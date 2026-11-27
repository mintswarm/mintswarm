#!/usr/bin/env python3
"""Record every floor.py and decision.py call of the plan-approval self-test through both sides.

    plan-approval-record.py SELFTEST BIN OUTPUT

Loads SELFTEST, .mintswarm/dogfood/plan-approval/selftest.py, without running
its main, and replaces the module's `subprocess` with a shim. Every call whose
argv is `[sys.executable, FLOOR or DECISION, ...]` runs twice from the same
starting state: first the Python, then `BIN tool plan-floor` or
`BIN tool plan-decision` with the same arguments, working directory and
environment. Between the two, and after the second, the recorder puts back
the state it captured, so the self-test goes on from the state the Python
left. The state is:

- `.mintswarm/job/outputs/floor.json` and `decision.json` in the working
  directory: their bytes, their absence, or an empty directory;
- the `.floor.*` and `.decision.*` files beside them;
- the `msw-floor-objects-` entries of the call's TMPDIR;
- the regular files directly in the parent of that TMPDIR, the git argument
  log among them.

A state the recorder cannot put back ends it with an error, never a pass.

The job repository, the git directory of the call's working directory (and
the one `GIT_DIR` names, when set), is never put back: its HEAD, refs,
packed-refs, config, shallow, info/grafts, index and the names under objects
are compared before and after each side, and a side that changes them
fails that call.

Every other call goes to the real `subprocess`. A `run`, `call`,
`check_call`, `check_output` or `Popen` that names FLOOR or DECISION in a
shape the recorder does not reproduce ends it with an error, so no call is
skipped unseen. The self-test's main then runs as usual.

Writes one JSON line per call to OUTPUT: the enclosing `check_*` function,
a sequence number, the tool, the argv after the script, whether `yq` is on
the call's PATH, and for each side the exit code, stdout and stderr in hex,
the two JSON files, the temporaries and stores left after it and whether it
changed the job repository. Then a
final line with the number of calls and the self-test's failure count.
Standard library only.
"""

import importlib.util
import json
import locale
import os
import shutil
import stat
import subprocess
import sys
from pathlib import Path

OUTPUTS = Path(".mintswarm/job/outputs")
FILES = ("floor.json", "decision.json")
TEMPORARIES = (".floor.", ".decision.")
STORE_PREFIX = "msw-floor-objects-"
SUBCOMMANDS = {"floor": "plan-floor", "decision": "plan-decision"}
SUPPORTED = {"cwd", "env", "capture_output", "text"}


class RecorderError(Exception):
    """The recorder cannot reproduce or put back a state."""


def load(path: Path):
    spec = importlib.util.spec_from_file_location("msw_plan_approval_selftest", path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def file_state(path: Path):
    if path.is_symlink():
        raise RecorderError(f"{path} is a symlink")
    if path.is_dir():
        if any(path.iterdir()):
            raise RecorderError(f"{path} is a directory that is not empty")
        return ("dir",)
    if path.exists():
        return ("file", path.read_bytes())
    return ("absent",)


def snapshot(cwd: Path, tmpdir: Path) -> dict:
    outputs = cwd / OUTPUTS
    state = {"outputs": outputs.is_dir(), "files": {}, "temporaries": {}, "stores": [], "parent": {}}
    for name in FILES:
        state["files"][name] = file_state(outputs / name)
    if outputs.is_dir():
        for entry in sorted(outputs.iterdir()):
            if entry.name.startswith(TEMPORARIES):
                state["temporaries"][entry.name] = file_state(entry)
    if tmpdir.is_dir():
        state["stores"] = sorted(p.name for p in tmpdir.iterdir() if p.name.startswith(STORE_PREFIX))
    parent = tmpdir.parent
    for entry in sorted(parent.iterdir()):
        if entry.is_file() and not entry.is_symlink():
            state["parent"][entry.name] = entry.read_bytes()
    return state


REPOSITORY_FILES = ("HEAD", "config", "shallow", "info/grafts", "index", "packed-refs")


def git_dirs(cwd: Path, tmpdir: Path, env: dict) -> list:
    """The job repository's git directory, found from cwd up to the sandbox, and GIT_DIR's."""
    dirs = []
    bound = tmpdir.parent.resolve()
    here = cwd.resolve()
    while True:
        dot = here / ".git"
        if dot.is_dir():
            dirs.append(dot)
            break
        if dot.is_file():
            text = dot.read_text(encoding="utf-8").strip()
            if not text.startswith("gitdir: "):
                raise RecorderError(f"{dot} is not a gitdir file")
            dirs.append((here / text[len("gitdir: "):]).resolve())
            break
        if here == bound or here.parent == here or bound not in here.parents:
            break
        here = here.parent
    if env.get("GIT_DIR"):
        named = Path(env["GIT_DIR"])
        if not named.is_absolute():
            named = cwd / named
        if named.resolve() not in [d.resolve() for d in dirs]:
            dirs.append(named)
    return dirs


def repository_state(dirs: list) -> dict:
    """The parts of each git directory a read-only tool must leave as they are."""
    state = {}
    for gitdir in dirs:
        entry = {}
        for name in REPOSITORY_FILES:
            path = gitdir / name
            entry[name] = path.read_bytes() if path.is_file() else None
        for tree in ("refs", "objects"):
            root = gitdir / tree
            names = sorted(str(p.relative_to(root)) for p in root.rglob("*")) if root.is_dir() else []
            if tree == "refs":
                entry[tree] = {name: (root / name).read_bytes() for name in names if (root / name).is_file()}
            else:
                entry[tree] = names
        state[str(gitdir)] = entry
    return state


class Writable:
    """Make a directory writable for the owner while the state is put back, then give its mode back."""

    def __init__(self, path: Path):
        self.path = path
        self.mode = None

    def __enter__(self):
        if self.path.is_dir() and not os.access(self.path, os.W_OK):
            self.mode = stat.S_IMODE(self.path.stat().st_mode)
            self.path.chmod(self.mode | stat.S_IWUSR | stat.S_IXUSR)
        return self

    def __exit__(self, *exc):
        if self.mode is not None:
            self.path.chmod(self.mode)
        return False


def put(path: Path, want, have) -> None:
    if want == have:
        return
    if have[0] == "dir":
        path.rmdir()
    elif have[0] == "file":
        path.unlink()
    if want[0] == "dir":
        path.mkdir()
    elif want[0] == "file":
        path.write_bytes(want[1])


def restore(cwd: Path, tmpdir: Path, want: dict) -> None:
    have = snapshot(cwd, tmpdir)
    if have == want:
        return
    outputs = cwd / OUTPUTS
    if want["outputs"] and not outputs.is_dir():
        outputs.mkdir(parents=True)
    if outputs.is_dir():
        with Writable(outputs):
            for name in FILES:
                put(outputs / name, want["files"][name], have["files"][name])
            for name in set(have["temporaries"]) | set(want["temporaries"]):
                put(
                    outputs / name,
                    want["temporaries"].get(name, ("absent",)),
                    have["temporaries"].get(name, ("absent",)),
                )
        if not want["outputs"]:
            outputs.rmdir()
    for name in have["stores"]:
        if name not in want["stores"]:
            shutil.rmtree(tmpdir / name)
    parent = tmpdir.parent
    for name in set(have["parent"]) | set(want["parent"]):
        if name not in want["parent"]:
            (parent / name).unlink()
        elif have["parent"].get(name) != want["parent"][name]:
            (parent / name).write_bytes(want["parent"][name])
    if snapshot(cwd, tmpdir) != want:
        raise RecorderError(f"the state in {cwd} cannot be put back")


def side_state(before: dict, after: dict) -> dict:
    def shown(state):
        if state[0] == "file":
            return {"state": "file", "hex": state[1].hex()}
        return {"state": state[0]}

    return {
        "files": {name: shown(after["files"][name]) for name in FILES},
        "temporaries": sorted(after["temporaries"]),
        "stores": [name for name in after["stores"] if name not in before["stores"]],
    }


def check_name() -> str:
    frame = sys._getframe(1)
    while frame is not None:
        if frame.f_code.co_name.startswith("check_"):
            return frame.f_code.co_name
        frame = frame.f_back
    return ""


def as_text(data: bytes) -> str:
    """What `subprocess.run(..., text=True)` gives for data."""
    text = data.decode(locale.getpreferredencoding(False))
    return text.replace("\r\n", "\n").replace("\r", "\n")


class Recorder:
    def __init__(self, module, binary: str):
        self.module = module
        self.binary = binary
        self.lines = []
        self.real = subprocess

    def tool_of(self, argv):
        if not isinstance(argv, (list, tuple)) or len(argv) < 2 or argv[0] != sys.executable:
            return None
        script = str(argv[1])
        if script == str(self.module.FLOOR):
            return "floor"
        if script == str(self.module.DECISION):
            return "decision"
        return None

    def side(self, argv, cwd, env):
        proc = self.real.run(argv, cwd=cwd, env=env, stdin=self.real.DEVNULL, capture_output=True)
        return proc.returncode, proc.stdout, proc.stderr

    def names_a_tool(self, argv) -> bool:
        scripts = (str(self.module.FLOOR), str(self.module.DECISION))
        if isinstance(argv, (str, bytes, os.PathLike)):
            text = os.fsdecode(argv)
            return any(script in text for script in scripts)
        if isinstance(argv, (list, tuple)):
            return any(os.fsdecode(arg) in scripts for arg in argv if isinstance(arg, (str, bytes, os.PathLike)))
        return False

    def call(self, tool: str, argv, kwargs: dict):
        unknown = set(kwargs) - SUPPORTED
        if unknown or not kwargs.get("capture_output"):
            raise RecorderError(f"a {tool} call the recorder does not reproduce: {sorted(kwargs)}")
        cwd = Path(kwargs.get("cwd") or os.getcwd())
        env = kwargs.get("env") or dict(os.environ)
        tmpdir = Path(env["TMPDIR"])
        args = [str(arg) for arg in argv[2:]]

        dirs = git_dirs(cwd, tmpdir, env)
        repository = repository_state(dirs)
        before = snapshot(cwd, tmpdir)
        python = self.side([sys.executable, str(argv[1]), *args], cwd, env)
        python_state = snapshot(cwd, tmpdir)
        python_changed = repository_state(dirs) != repository
        restore(cwd, tmpdir, before)
        rust = self.side([self.binary, "tool", SUBCOMMANDS[tool], *args], cwd, env)
        rust_state = snapshot(cwd, tmpdir)
        rust_changed = repository_state(dirs) != repository
        restore(cwd, tmpdir, python_state)

        def shown(result, state, changed):
            code, out, err = result
            return {
                "exit": code,
                "stdout": out.hex(),
                "stderr": err.hex(),
                "repository_changed": changed,
                **side_state(before, state),
            }

        self.lines.append(
            {
                "check": check_name(),
                "seq": len(self.lines),
                "tool": tool,
                "argv": args,
                "yq": shutil.which("yq", path=env.get("PATH", "")) is not None,
                "python": shown(python, python_state, python_changed),
                "rust": shown(rust, rust_state, rust_changed),
            }
        )
        code, out, err = python
        if kwargs.get("text"):
            out, err = as_text(out), as_text(err)
        return self.real.CompletedProcess(list(argv), code, out, err)


class Shim:
    """The module's `subprocess`: floor.py and decision.py calls go through the recorder."""

    def __init__(self, recorder: Recorder):
        self._recorder = recorder

    def __getattr__(self, name):
        return getattr(subprocess, name)

    def _unseen(self, how: str, argv) -> None:
        if self._recorder.names_a_tool(argv):
            raise RecorderError(f"a {how} of floor.py or decision.py the recorder does not reproduce: {argv!r}")

    def run(self, argv, *args, **kwargs):
        tool = self._recorder.tool_of(argv)
        if tool is None or args:
            self._unseen("run", argv)
            return subprocess.run(argv, *args, **kwargs)
        return self._recorder.call(tool, argv, kwargs)

    def call(self, argv, *args, **kwargs):
        self._unseen("call", argv)
        return subprocess.call(argv, *args, **kwargs)

    def check_call(self, argv, *args, **kwargs):
        self._unseen("check_call", argv)
        return subprocess.check_call(argv, *args, **kwargs)

    def check_output(self, argv, *args, **kwargs):
        self._unseen("check_output", argv)
        return subprocess.check_output(argv, *args, **kwargs)

    def Popen(self, argv, *args, **kwargs):  # noqa: N802 - subprocess's own name
        self._unseen("Popen", argv)
        return subprocess.Popen(argv, *args, **kwargs)


def main(argv: list) -> int:
    if len(argv) != 3:
        print("usage: plan-approval-record.py SELFTEST BIN OUTPUT", file=sys.stderr)
        return 2
    selftest, binary, output = Path(argv[0]), argv[1], Path(argv[2])
    module = load(selftest)
    recorder = Recorder(module, binary)
    module.subprocess = Shim(recorder)
    module.main()
    with open(output, "w", encoding="utf-8") as handle:
        for line in recorder.lines:
            handle.write(json.dumps(line) + "\n")
        handle.write(json.dumps({"calls": len(recorder.lines), "failed": module.T.failed}) + "\n")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
