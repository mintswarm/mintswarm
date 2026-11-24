#!/usr/bin/env python3
"""Self-test for the commit helper and its pre-commit hook, run by the commit-selftest gate.

    commit-selftest.py

Builds a fixture repository in a temporary directory, outside the source
tree, with a neutral git environment (no GIT_* variables, no global or
system config, HOME and black's cache inside the temporary directory) and
pinned identities. Its .mintswarm/config.yaml declares three formatters:

    python-format   black .                       paths **/*.py
    broken          exit 3                        paths fail/**
    web-stub        touches a marker file outside the repository
                                                  paths web/**

Each case starts from a fresh fixture and runs .mintswarm/dogfood/commit.py,
or `git commit` with core.hooksPath at .mintswarm/dogfood/hooks. Two more
cases read the source tree's own .mintswarm/config.yaml. Prints one line per
case; exits 0 only when every case passes and the source tree's
`git status --porcelain --ignored` is the same afterwards, 1 otherwise.
Needs git, yq and black on PATH.
"""

import importlib.util
import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
HELPER = HERE / "commit.py"
HOOKS = HERE / "hooks"
SOURCE = HERE.parent.parent
UNFORMATTED = "x  =  [1,2 ,\n 3]\n"
FIXTURE_CONFIG = """gates:
  run:
    - name: python-format
      command: "black --check ."
  format:
    - name: python-format
      command: "black ."
      paths: ["**/*.py"]
      gate: python-format
    - name: broken
      command: "exit 3"
      paths: ["fail/**"]
      gate: python-format
    - name: web-stub
      command: "touch '{marker}'"
      paths: ["web/**"]
      gate: python-format
"""


class Failed(Exception):
    """A case's expectation does not hold."""


def expect(condition: bool, message: str) -> None:
    if not condition:
        raise Failed(message)


class Fixture:
    def __init__(self, base: Path):
        self.base = base
        self.repo = base / "repo"
        self.marker = base / "web-stub-ran"
        home = base / "home"
        home.mkdir()
        env = {k: v for k, v in os.environ.items() if not k.startswith("GIT_")}
        env.update(
            HOME=str(home),
            XDG_CONFIG_HOME=str(home / ".config"),
            XDG_CACHE_HOME=str(home / ".cache"),
            BLACK_CACHE_DIR=str(base / "black-cache"),
            GIT_CONFIG_GLOBAL=os.devnull,
            GIT_CONFIG_NOSYSTEM="1",
            GIT_EDITOR="true",
            PYTHONDONTWRITEBYTECODE="1",
            GIT_AUTHOR_NAME="Self Test",
            GIT_AUTHOR_EMAIL="selftest@example.invalid",
            GIT_AUTHOR_DATE="2026-01-01T00:00:00Z",
            GIT_COMMITTER_NAME="Self Test",
            GIT_COMMITTER_EMAIL="selftest@example.invalid",
            GIT_COMMITTER_DATE="2026-01-01T00:00:00Z",
        )
        self.env = env
        self.repo.mkdir()
        self.git("init", "-q", "-b", "master")
        self.write(".mintswarm/config.yaml", FIXTURE_CONFIG.format(marker=self.marker))
        self.write("base.py", "base = 1\n")
        self.git("add", ".mintswarm/config.yaml", "base.py")
        self.git("commit", "-q", "-m", "fixture")

    def write(self, name: str, text: str) -> None:
        path = self.repo / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text)

    def read(self, name: str) -> str:
        return (self.repo / name).read_text()

    def run(self, argv, data=None) -> subprocess.CompletedProcess:
        return subprocess.run(
            argv, cwd=self.repo, env=self.env, input=data, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True
        )

    def git(self, *args) -> str:
        proc = self.run(["git", *args])
        if proc.returncode != 0:
            raise RuntimeError(f"git {' '.join(args)} failed: {proc.stderr}")
        return proc.stdout

    def helper(self, *args) -> subprocess.CompletedProcess:
        return self.run([sys.executable, str(HELPER), *args])

    def head(self) -> str:
        return self.git("rev-parse", "HEAD").strip()

    def black_check_committed(self, name: str) -> bool:
        text = self.git("show", f"HEAD:{name}")
        return self.run(["black", "--check", "-q", "-"], data=text).returncode == 0


def case_formats_python(f: Fixture) -> None:
    f.write("a.py", UNFORMATTED)
    proc = f.helper("-m", "add a", "--", "a.py")
    expect(proc.returncode == 0, f"exit {proc.returncode}: {proc.stderr}")
    committed = f.git("show", "HEAD:a.py")
    expect(committed != UNFORMATTED, "a.py was committed unformatted")
    expect(f.black_check_committed("a.py"), "black --check fails on the committed a.py")
    expect(f.git("status", "--porcelain") == "", "the work tree is not clean after the commit")


def case_partial_staging(f: Fixture) -> None:
    f.write("p.py", "p = 1\n")
    f.git("add", "p.py")
    f.write("p.py", "p = 2\n")
    head = f.head()
    proc = f.helper("-m", "partial")
    expect(proc.returncode == 1, f"exit {proc.returncode}, not 1")
    expect("p.py" in proc.stderr, f"stderr does not name p.py: {proc.stderr}")
    expect(f.head() == head, "a commit was made")


def case_formatter_fails(f: Fixture) -> None:
    f.write("fail/x.txt", "x\n")
    head = f.head()
    proc = f.helper("-m", "fail", "--", "fail/x.txt")
    expect(proc.returncode == 1, f"exit {proc.returncode}, not 1")
    expect("broken" in proc.stderr, f"stderr does not name broken: {proc.stderr}")
    expect(f.head() == head, "a commit was made")


def case_selection(f: Fixture) -> None:
    f.write("a.py", "a = 1\n")
    proc = f.helper("-m", "python only", "--", "a.py")
    expect(proc.returncode == 0, f"exit {proc.returncode}: {proc.stderr}")
    expect(not f.marker.exists(), "web-stub ran for a commit with no web/** path")
    expect("broken" not in proc.stderr, "broken ran for a commit with no fail/** path")
    f.write("web/x.txt", "x\n")
    proc = f.helper("-m", "web", "--", "web/x.txt")
    expect(proc.returncode == 0, f"exit {proc.returncode}: {proc.stderr}")
    expect(f.marker.exists(), "web-stub did not run for a commit with a web/** path")


def case_restore_others(f: Fixture) -> None:
    f.write("other.py", UNFORMATTED)
    f.git("add", "other.py")
    f.git("commit", "-q", "--no-verify", "-m", "unformatted other.py")
    f.write("scratch.py", UNFORMATTED)
    f.write("a.py", UNFORMATTED)
    proc = f.helper("-m", "add a", "--", "a.py")
    expect(proc.returncode == 0, f"exit {proc.returncode}: {proc.stderr}")
    expect(f.read("other.py") == UNFORMATTED, "other.py was left changed")
    expect(f.read("scratch.py") == UNFORMATTED, "scratch.py was left changed")
    status = f.git("status", "--porcelain")
    expect(status == "?? scratch.py\n", f"git status is {status!r}")
    expect(f.git("show", "--name-only", "--format=", "HEAD") == "a.py\n", "the commit holds more than a.py")
    for name in ("other.py", "scratch.py"):
        expect(f"restored {name}" in proc.stderr, f"{name} is not reported: {proc.stderr}")


def case_merge(f: Fixture) -> None:
    f.write("m.py", "m = 1\n")
    f.git("add", "m.py")
    f.git("commit", "-q", "-m", "m")
    f.git("checkout", "-q", "-b", "job")
    f.write("m.py", "m = 2\n")
    f.git("commit", "-q", "-am", "job side")
    f.git("checkout", "-q", "master")
    f.write("m.py", "m = 3\n")
    f.git("commit", "-q", "-am", "master side")
    f.git("checkout", "-q", "job")
    expect(f.run(["git", "merge", "-q", "master"]).returncode != 0, "the merge did not conflict")
    f.write("m.py", "m  =  [2,\n3]\n")
    f.git("add", "m.py")
    proc = f.helper()
    expect(proc.returncode == 0, f"exit {proc.returncode}: {proc.stderr}")
    parents = f.git("rev-list", "--parents", "-n", "1", "HEAD").split()
    expect(len(parents) == 3, f"HEAD has {len(parents) - 1} parents, not 2")
    expect(f.black_check_committed("m.py"), "black --check fails on the committed m.py")


def case_hook_formats(f: Fixture) -> None:
    f.write("h.py", UNFORMATTED)
    f.git("add", "h.py")
    proc = f.run(["git", "-c", f"core.hooksPath={HOOKS}", "commit", "-q", "-m", "hook"])
    expect(proc.returncode == 0, f"exit {proc.returncode}: {proc.stderr}")
    expect(f.black_check_committed("h.py"), "black --check fails on the committed h.py")
    expect(f.git("status", "--porcelain") == "", "the work tree is not clean after the commit")


def case_hook_partial(f: Fixture) -> None:
    f.write("q.py", "q = 1\n")
    f.git("add", "q.py")
    f.write("q.py", "q = 2\n")
    head = f.head()
    proc = f.run(["git", "-c", f"core.hooksPath={HOOKS}", "commit", "-q", "-m", "hook"])
    expect(proc.returncode != 0, "git commit succeeded")
    expect("q.py" in proc.stderr, f"stderr does not name q.py: {proc.stderr}")
    expect(f.head() == head, "a commit was made")


def case_hook_pathspec(f: Fixture) -> None:
    f.write("r.py", UNFORMATTED)
    f.git("add", "r.py")
    head = f.head()
    proc = f.run(["git", "-c", f"core.hooksPath={HOOKS}", "commit", "-q", "-m", "hook", "--", "r.py"])
    expect(proc.returncode != 0, "git commit with paths succeeded")
    expect("git commit <paths>" in proc.stderr, f"stderr does not explain the refusal: {proc.stderr}")
    expect(f.head() == head, "a commit was made")
    expect(f.git("status", "--porcelain") == "A  r.py\n", "the index or the work tree changed")


def case_literal_paths(f: Fixture) -> None:
    # git reads an existing file's name literally even as a glob; a deleted one it does not
    f.write("app/[id].txt", "named\n")
    f.git("add", "app/[id].txt")
    f.git("commit", "-q", "--no-verify", "-m", "bracketed")
    (f.repo / "app" / "[id].txt").unlink()
    f.write("app/i.txt", "glob match\n")
    f.write("app/d.txt", "glob match\n")
    proc = f.helper("-m", "remove bracketed", "--", "app/[id].txt")
    expect(proc.returncode == 0, f"exit {proc.returncode}: {proc.stderr}")
    committed = f.git("show", "--name-only", "--format=", "HEAD")
    expect(committed == "app/[id].txt\n", f"the commit holds {committed!r}")
    status = f.git("status", "--porcelain", "--untracked-files=all")
    expect(status == "?? app/d.txt\n?? app/i.txt\n", f"git status is {status!r}")


def load_helper():
    sys.dont_write_bytecode = True  # leave no __pycache__ next to the helpers
    spec = importlib.util.spec_from_file_location("msw_commit_helper", HELPER)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def live_formatters():
    helper = load_helper()
    return helper, helper.load_formatters(SOURCE, SOURCE / ".mintswarm" / "config.yaml")


def case_live_config(_: Fixture) -> None:
    helper, formatters = live_formatters()
    names = [f["name"] for f in formatters]
    expect(sorted(names) == ["python-format", "rust-format", "typescript-format"], f"gates.format names {names}")
    config = helper.yaml_to_json((SOURCE / ".mintswarm" / "config.yaml").read_bytes(), "the source config")
    gates = {gate.get("name") for gate in config["gates"]["run"]}
    for entry in config["gates"]["format"]:
        expect(entry.get("gate") in gates, f"{entry.get('name')} names no gates.run entry")


def case_live_selection(_: Fixture) -> None:
    helper, formatters = live_formatters()
    chosen = helper.select_formatters(formatters, ["x/a.py"])
    expect([f["name"] for f in chosen] == ["python-format"], f"x/a.py selects {[f['name'] for f in chosen]}")
    for f in chosen:
        expect("web-install.sh" not in f["command"] and "prettier" not in f["command"], f"{f['name']} runs web tools")
    chosen = helper.select_formatters(formatters, ["web/apps/ui/src/a.ts"])
    expect([f["name"] for f in chosen] == ["typescript-format"], "a web/** path does not select typescript-format")


CASES = [
    ("formats-python", case_formats_python),
    ("partial-staging", case_partial_staging),
    ("formatter-fails", case_formatter_fails),
    ("selection", case_selection),
    ("restore-others", case_restore_others),
    ("merge", case_merge),
    ("hook-formats", case_hook_formats),
    ("hook-partial", case_hook_partial),
    ("hook-pathspec", case_hook_pathspec),
    ("literal-paths", case_literal_paths),
    ("live-config", case_live_config),
    ("live-selection", case_live_selection),
]


def source_status() -> bytes:
    return subprocess.run(
        ["git", "status", "--porcelain", "--ignored", "-z"], cwd=SOURCE, stdout=subprocess.PIPE, check=True
    ).stdout


def main() -> int:
    for tool in ("git", "yq", "black"):
        if shutil.which(tool) is None:
            print(f"commit-selftest: {tool} is not on PATH", file=sys.stderr)
            return 1
    before = source_status()
    failures = 0
    for name, case in CASES:
        with tempfile.TemporaryDirectory(prefix="msw-commit-selftest.") as tmp:
            try:
                case(Fixture(Path(tmp).resolve()))
                print(f"ok   {name}")
            except (Failed, RuntimeError) as exc:
                failures += 1
                print(f"FAIL {name}: {exc}")
    if source_status() != before:
        failures += 1
        print("FAIL source-tree: git status --porcelain --ignored changed while the self-test ran")
    print(f"{failures} failure(s) in {len(CASES)} cases")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
