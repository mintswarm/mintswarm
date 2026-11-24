#!/usr/bin/env python3
"""Format the staged files with the declared formatters, then commit them.

    commit.py [-m MSG]... [--config PATH] [--hook] [-- PATHS...]

The commit helper of the dogfooding session, and, in --hook mode, the body
of the pre-commit hook in .mintswarm/dogfood/hooks/. Run from anywhere in a
work tree; every git command inherits the environment, so under a hook it
uses the index git chose (GIT_INDEX_FILE). In --hook mode it refuses a
commit with paths (`git commit <paths>`, `--only`), whose temporary index
would take the formatted files while the index kept them unformatted.

1. Stages PATHS with `git add -A -- PATHS`, relative to the working
   directory and read literally, not as globs (not in --hook mode). The
   staged set is then every path the index adds or modifies against HEAD
   (against the empty tree before the first commit); deletions are not
   part of it.
2. Refuses, naming each path and committing nothing, an unmerged path and a
   staged path that also has unstaged changes.
3. Reads the formatters from `gates.format` in HEAD:.mintswarm/config.yaml,
   or from PATH with --config, through `yq -p=yaml -o=json`. Without HEAD,
   without the config or without `gates.format` no formatter runs. Each
   entry has a `name`, a whole-tree `command` and the `paths` globs it
   covers (the plan floor's glob semantics, from
   plan-approval/floor.py). Only the formatters with a glob matching a
   staged path run.
4. Before they run, records every non-ignored file of the work tree outside
   the staged set. Each selected command then runs with the shell at the
   top of the work tree, in declaration order, its output on stderr. When
   one exits non-zero it is named, the recorded files are put back and
   nothing is committed.
5. Re-stages the staged set only, puts back every recorded file a formatter
   changed or deleted (its recorded bytes and mode, or `git checkout` for a
   file that matched the index), removes every file a formatter created,
   and reports each such path on stderr.
6. Commits with `git commit --no-verify` and the -m messages, or, during a
   merge (MERGE_HEAD exists) without -m, with git's merge message. In
   --hook mode it stops after step 5: git makes the commit.

Exits 0 on success, 1 on a refusal or a failing formatter, 2 on a usage
error or when git or yq fails. Needs git and yq; standard library only.
"""

import hashlib
import importlib.util
import json
import os
import stat
import subprocess
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
FLOOR = HERE / "plan-approval" / "floor.py"
CONFIG = ".mintswarm/config.yaml"
EMPTY_TREE = "4b825dc642cb6eb9a060e54bf8d69288fbee4904"
USAGE = "usage: commit.py [-m MSG]... [--config PATH] [--hook] [-- PATHS...]"
PROG = "commit.py"


class UsageError(Exception):
    """The command line is unusable."""


class GitError(Exception):
    """git or yq failed or is missing."""


class Refusal(Exception):
    """Nothing is committed; the message says why."""


def say(message: str) -> None:
    print(f"{PROG}: {message}", file=sys.stderr)


def run(argv, cwd=None, data=None, env=None, check=True) -> subprocess.CompletedProcess:
    try:
        proc = subprocess.run(argv, cwd=cwd, input=data, env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    except OSError as exc:
        raise GitError(f"cannot run {argv[0]}: {exc}") from exc
    if check and proc.returncode != 0:
        detail = proc.stderr.decode(errors="replace").strip()
        raise GitError(f"{' '.join(argv[:3])} failed: {detail}")
    return proc


def literal_env() -> dict:
    env = dict(os.environ)
    env["GIT_LITERAL_PATHSPECS"] = "1"
    return env


def nul_list(data: bytes) -> list:
    return [p.decode("utf-8", "surrogateescape") for p in data.split(b"\0") if p]


def nul_join(paths) -> bytes:
    return b"".join(p.encode("utf-8", "surrogateescape") + b"\0" for p in paths)


# --- arguments ---------------------------------------------------------


def parse_args(argv: list) -> dict:
    args = {"messages": [], "config": None, "hook": False, "paths": []}
    i = 0
    while i < len(argv):
        arg = argv[i]
        if arg == "--":
            args["paths"] = argv[i + 1 :]
            break
        if arg in ("-m", "--config"):
            if i + 1 >= len(argv):
                raise UsageError(f"{arg} needs a value; {USAGE}")
            if arg == "-m":
                args["messages"].append(argv[i + 1])
            elif args["config"] is not None:
                raise UsageError(f"--config is given twice; {USAGE}")
            else:
                args["config"] = argv[i + 1]
            i += 2
        elif arg == "--hook":
            args["hook"] = True
            i += 1
        else:
            raise UsageError(f"unknown argument {arg!r}; {USAGE}")
    if args["hook"] and (args["messages"] or args["paths"]):
        raise UsageError(f"--hook takes no -m and no PATHS; {USAGE}")
    if args["paths"] and not all(args["paths"]):
        raise UsageError(f"an empty path; {USAGE}")
    return args


# --- formatters --------------------------------------------------------


def load_floor():
    sys.dont_write_bytecode = True  # leave no __pycache__ next to floor.py
    try:
        spec = importlib.util.spec_from_file_location("msw_commit_plan_floor", FLOOR)
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
    except Exception as exc:
        raise GitError(f"cannot load {FLOOR}: {type(exc).__name__}: {exc}") from exc
    return module


def parse_formatters(config: dict) -> list:
    """The `gates.format` entries of a config, as dicts with name, command and paths."""
    gates = config.get("gates") if isinstance(config, dict) else None
    entries = gates.get("format") if isinstance(gates, dict) else None
    if entries is None:
        return []
    if not isinstance(entries, list):
        raise GitError("gates.format is not a list")
    formatters = []
    for index, entry in enumerate(entries):
        ok = (
            isinstance(entry, dict)
            and isinstance(entry.get("name"), str)
            and isinstance(entry.get("command"), str)
            and isinstance(entry.get("paths"), list)
            and all(isinstance(p, str) for p in entry["paths"])
        )
        if not ok:
            raise GitError(f"gates.format[{index}] needs a string name and command and a list of paths")
        formatters.append({"name": entry["name"], "command": entry["command"], "paths": entry["paths"]})
    return formatters


def yaml_to_json(text: bytes, what: str) -> dict:
    proc = run(["yq", "-p=yaml", "-o=json", "."], data=text, check=False)
    if proc.returncode != 0:
        raise GitError(f"yq cannot read {what}: {proc.stderr.decode(errors='replace').strip()}")
    try:
        return json.loads(proc.stdout or b"null") or {}
    except ValueError as exc:
        raise GitError(f"yq gave no JSON for {what}: {exc}") from exc


def load_formatters(root: Path, config_path=None) -> list:
    if config_path is not None:
        try:
            text = Path(config_path).read_bytes()
        except OSError as exc:
            raise GitError(f"cannot read {config_path}: {exc}") from exc
        return parse_formatters(yaml_to_json(text, config_path))
    if run(["git", "rev-parse", "--verify", "-q", "HEAD^{commit}"], cwd=root, check=False).returncode != 0:
        return []
    if run(["git", "cat-file", "-e", f"HEAD:{CONFIG}"], cwd=root, check=False).returncode != 0:
        return []
    text = run(["git", "cat-file", "blob", f"HEAD:{CONFIG}"], cwd=root).stdout
    return parse_formatters(yaml_to_json(text, f"HEAD:{CONFIG}"))


def select_formatters(formatters: list, paths, floor=None) -> list:
    """The formatters with a `paths` glob that matches one of paths, in declaration order."""
    floor = floor or load_floor()
    paths = list(paths)
    chosen = []
    for formatter in formatters:
        regexes = [floor.glob_to_regex(glob) for glob in formatter["paths"]]
        if any(regex.fullmatch(path) for regex in regexes for path in paths):
            chosen.append(formatter)
    return chosen


# --- the work tree -----------------------------------------------------


def staged_paths(root: Path) -> list:
    has_head = run(["git", "rev-parse", "--verify", "-q", "HEAD^{commit}"], cwd=root, check=False).returncode == 0
    against = "HEAD" if has_head else EMPTY_TREE
    argv = ["git", "diff", "--cached", "--name-only", "-z", "--no-renames", "--no-ext-diff", "--diff-filter=d"]
    return nul_list(run(argv + [against, "--"], cwd=root).stdout)


def unstaged_paths(root: Path) -> set:
    argv = ["git", "diff", "--name-only", "-z", "--no-renames", "--no-ext-diff", "--"]
    return set(nul_list(run(argv, cwd=root).stdout))


def unmerged_paths(root: Path) -> list:
    lines = nul_list(run(["git", "ls-files", "-u", "-z"], cwd=root).stdout)
    return sorted({line.split("\t", 1)[1] for line in lines if "\t" in line})


def file_state(path: Path):
    """What restoring needs to compare: None when missing, else (kind, digest, executable)."""
    try:
        info = path.lstat()
    except FileNotFoundError:
        return None
    if stat.S_ISLNK(info.st_mode):
        return ("link", os.readlink(path), False)
    if not stat.S_ISREG(info.st_mode):
        return ("other", None, False)
    return ("file", hashlib.sha256(path.read_bytes()).hexdigest(), bool(info.st_mode & stat.S_IXUSR))


def snapshot(root: Path, staged) -> dict:
    """Every non-ignored work-tree path outside staged: its state, and its bytes unless it matches the index."""
    staged = set(staged)
    listed = nul_list(run(["git", "ls-files", "-co", "--exclude-standard", "-z"], cwd=root).stdout)
    tracked = set(nul_list(run(["git", "ls-files", "-z"], cwd=root).stdout))
    dirty = unstaged_paths(root)
    shot = {}
    for name in listed:
        if name in staged or name in shot:
            continue
        path = root / name
        state = file_state(path)
        keep = None
        if state is not None and state[0] == "file" and (name not in tracked or name in dirty):
            keep = (path.read_bytes(), path.lstat().st_mode)
        shot[name] = (state, keep)
    return shot


def restore(root: Path, staged, shot: dict) -> list:
    """Put back what the formatters changed outside staged; the paths, sorted."""
    staged = set(staged)
    touched = []
    checkout = []
    for name, (before, keep) in shot.items():
        path = root / name
        if before is not None and before[0] == "other":
            continue
        if file_state(path) == before:
            continue
        touched.append(name)
        if before is None:
            path.unlink()
        elif keep is not None:
            if path.is_symlink() or path.is_dir():
                raise GitError(f"cannot restore {name}: a formatter made it a link or a directory")
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(keep[0])
            os.chmod(path, stat.S_IMODE(keep[1]))
        else:
            checkout.append(name)
    if checkout:
        run(
            ["git", "checkout", "--pathspec-from-file=-", "--pathspec-file-nul"],
            root,
            nul_join(checkout),
            literal_env(),
        )
    listed = nul_list(run(["git", "ls-files", "-co", "--exclude-standard", "-z"], cwd=root).stdout)
    for name in listed:
        if name in staged or name in shot:
            continue
        path = root / name
        if path.is_symlink() or path.is_file():
            path.unlink()
            touched.append(name)
    return sorted(set(touched))


def run_formatters(root: Path, chosen: list) -> None:
    for formatter in chosen:
        say(f"running {formatter['name']}: {formatter['command']}")
        sys.stderr.flush()
        try:
            code = subprocess.run(formatter["command"], shell=True, cwd=root, stdout=sys.stderr).returncode
        except OSError as exc:
            raise Refusal(f"formatter {formatter['name']} cannot run: {exc}") from exc
        if code != 0:
            raise Refusal(f"formatter {formatter['name']} exited {code}; nothing is committed")


# --- main --------------------------------------------------------------


def git_path(root: Path, name: str) -> Path:
    return root / run(["git", "rev-parse", "--git-path", name], cwd=root).stdout.decode().strip()


def temporary_index(root: Path) -> bool:
    """Whether git runs the hook on a temporary index: `git commit <paths>`.

    A plain commit uses the index and `git commit -a` or `-i` its lock, both
    of which become the index; a commit with pathspecs uses a separate
    next-index-*.lock and leaves the paths unformatted in the index.
    """
    chosen = os.environ.get("GIT_INDEX_FILE")
    if not chosen:
        return False
    env = {k: v for k, v in os.environ.items() if k != "GIT_INDEX_FILE"}
    real = run(["git", "rev-parse", "--git-path", "index"], cwd=root, env=env).stdout.decode().strip()
    real = os.path.realpath(root / real)
    return os.path.realpath(root / chosen) not in (real, real + ".lock")


def helper(args: dict) -> int:
    root = Path(run(["git", "rev-parse", "--show-toplevel"]).stdout.decode().strip())
    if args["hook"] and temporary_index(root):
        raise Refusal("cannot format a commit with paths (git commit <paths>); stage the paths and run git commit")
    merging = git_path(root, "MERGE_HEAD").exists()
    if not args["hook"] and not args["messages"] and not merging:
        raise UsageError(f"-m is required outside a merge; {USAGE}")
    if args["paths"]:
        run(["git", "add", "-A", "--"] + args["paths"], env=literal_env())
    unmerged = unmerged_paths(root)
    if unmerged:
        raise Refusal("resolve and stage these unmerged paths first: " + ", ".join(unmerged))
    staged = staged_paths(root)
    partial = sorted(set(staged) & unstaged_paths(root))
    if partial:
        raise Refusal(
            "these staged paths also have unstaged changes; stage them whole or stash the rest: " + ", ".join(partial)
        )
    chosen = select_formatters(load_formatters(root, args["config"]), staged) if staged else []
    if chosen:
        shot = snapshot(root, staged)
        try:
            run_formatters(root, chosen)
        finally:
            for name in restore(root, staged, shot):
                say(f"restored {name}: a formatter changed or created it, but it is not staged")
        run(
            ["git", "add", "-A", "--pathspec-from-file=-", "--pathspec-file-nul"], root, nul_join(staged), literal_env()
        )
    if args["hook"]:
        return 0
    argv = ["git", "commit", "--no-verify"]
    if args["messages"]:
        for message in args["messages"]:
            argv += ["-m", message]
    else:
        argv.append("--no-edit")
    proc = subprocess.run(argv, cwd=root)
    if proc.returncode != 0:
        raise GitError(f"git commit exited {proc.returncode}")
    return 0


def main(argv: list) -> int:
    try:
        return helper(parse_args(argv))
    except Refusal as exc:
        say(str(exc))
        return 1
    except (UsageError, GitError) as exc:
        say(str(exc))
        return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
