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
Every other call goes to the real `subprocess`. The self-test's main then
runs as usual.

Writes one JSON line per call to OUTPUT: the enclosing `check_*` function,
a sequence number, the tool, the argv after the script, whether `yq` is on
the call's PATH, and for each side the exit code, stdout and stderr in hex,
the two JSON files and the temporaries and stores left after it. Then a
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

    def call(self, tool: str, argv, kwargs: dict):
        unknown = set(kwargs) - SUPPORTED
        if unknown or not kwargs.get("capture_output"):
            raise RecorderError(f"a {tool} call the recorder does not reproduce: {sorted(kwargs)}")
        cwd = Path(kwargs.get("cwd") or os.getcwd())
        env = kwargs.get("env") or dict(os.environ)
        tmpdir = Path(env["TMPDIR"])
        args = [str(arg) for arg in argv[2:]]

        before = snapshot(cwd, tmpdir)
        python = self.side([sys.executable, str(argv[1]), *args], cwd, env)
        python_state = snapshot(cwd, tmpdir)
        restore(cwd, tmpdir, before)
        rust = self.side([self.binary, "tool", SUBCOMMANDS[tool], *args], cwd, env)
        rust_state = snapshot(cwd, tmpdir)
        restore(cwd, tmpdir, python_state)

        def shown(result, state):
            code, out, err = result
            return {"exit": code, "stdout": out.hex(), "stderr": err.hex(), **side_state(before, state)}

        self.lines.append(
            {
                "check": check_name(),
                "seq": len(self.lines),
                "tool": tool,
                "argv": args,
                "yq": shutil.which("yq", path=env.get("PATH", "")) is not None,
                "python": shown(python, python_state),
                "rust": shown(rust, rust_state),
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

    def run(self, argv, *args, **kwargs):
        tool = self._recorder.tool_of(argv)
        if tool is None or args:
            return subprocess.run(argv, *args, **kwargs)
        return self._recorder.call(tool, argv, kwargs)


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
