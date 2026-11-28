#!/usr/bin/env python3
"""Read the project setup: the factory's branches, starting with the work branch.

    project-setup.py work-branch    print the work branch
    project-setup.py diff-check     git diff --check origin/<work branch>...HEAD
    project-setup.py packs-check    packs/pack.py check --base origin/<work branch>

The setup is project-setup.yaml beside this script, read from the checkout
and never at a ref, with `yq -o=json . FILE`. It is a mapping with one key,
`branches`, itself a mapping with one key, `work`: the work branch, the branch
the factory integrates into. Any other key is refused, and so is a value that
is not a string. A name is valid when it does not start with `-`, is not
`HEAD`, and `git check-ref-format refs/heads/<name>` accepts it.

This is the one reader of the file outside Rust, where crates/config's
load_project_setup is its schema; the case table
crates/config/tests/conformance/project-setup.json holds both to the same
rules, and .mintswarm/dogfood/github/selftest.sh runs it against this script.

The whitespace and packs-check gates run diff-check and packs-check, which
read the work branch and then run the check against origin/<work branch> from
the current directory, the repository root; common.sh wraps work-branch as
msw_work_branch; and the Python tools load this script by path with
importlib.util, without writing bytecode, and call work_branch(), which raises
SetupError naming the key path on any failure.

work-branch exits 0 with the name on stdout. diff-check and packs-check exit
with the check's status, or 1 when it cannot be started. All three exit 2 on a
usage error or when the setup cannot be read, with
`project-setup: <path>: <what>` on stderr, and then run nothing.
"""

import json
import subprocess
import sys
from pathlib import Path

SETUP = Path(__file__).with_name("project-setup.yaml")
USAGE = "usage: project-setup.py work-branch|diff-check|packs-check"
PACK = Path(__file__).with_name("packs") / "pack.py"


class SetupError(Exception):
    """The setup cannot be read; `path` is the key path, empty for the file as a whole."""

    def __init__(self, path: str, message: str):
        super().__init__(f"{path}: {message}" if path else message)
        self.path = path
        self.message = message


def valid_branch(name: str) -> bool:
    """Whether NAME is a valid branch name, by git's own rule and the two extra ones."""
    if name.startswith("-") or name == "HEAD":
        return False
    try:
        done = subprocess.run(
            ["git", "check-ref-format", f"refs/heads/{name}"],
            stdin=subprocess.DEVNULL,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
    except (OSError, ValueError):
        # ValueError: a name with a NUL, which no argv can carry.
        return False
    return done.returncode == 0


def load(path: Path = SETUP) -> dict:
    """The setup file at PATH, as yq reads it."""
    try:
        done = subprocess.run(
            ["yq", "-o=json", ".", str(path)],
            stdin=subprocess.DEVNULL,
            capture_output=True,
            text=True,
        )
    except OSError:
        raise SetupError("", f"cannot run yq on {path.name}") from None
    if done.returncode != 0:
        raise SetupError("", f"cannot read {path.name}")
    try:
        return json.loads(done.stdout or "null")
    except ValueError:
        raise SetupError("", f"cannot read {path.name}") from None


def only_keys(mapping: dict, prefix: str, keys: tuple) -> None:
    for key in mapping:
        if key not in keys:
            raise SetupError(f"{prefix}{key}", "unknown key")


def work_branch(path: Path = SETUP) -> str:
    """The work branch the setup at PATH names; SetupError when it cannot be read."""
    setup = load(path)
    if not isinstance(setup, dict):
        raise SetupError("", "the project setup must be a mapping")
    only_keys(setup, "", ("branches",))
    if "branches" not in setup:
        raise SetupError("branches", "is missing")
    branches = setup["branches"]
    if not isinstance(branches, dict):
        raise SetupError("branches", "must be a mapping")
    only_keys(branches, "branches.", ("work",))
    if "work" not in branches:
        raise SetupError("branches.work", "is missing")
    name = branches["work"]
    if not isinstance(name, str):
        raise SetupError("branches.work", "must be a string")
    if not valid_branch(name):
        raise SetupError("branches.work", "is not a valid branch name")
    return name


def check_command(action: str, name: str) -> list:
    """The argv of the check ACTION names, against origin/NAME."""
    if action == "diff-check":
        return ["git", "diff", "--check", f"origin/{name}...HEAD"]
    return [sys.executable, str(PACK), "check", "--base", f"origin/{name}"]


def main(argv: list) -> int:
    if len(argv) != 1 or argv[0] not in ("work-branch", "diff-check", "packs-check"):
        sys.stderr.write(USAGE + "\n")
        return 2
    try:
        name = work_branch()
    except SetupError as e:
        sys.stderr.write(f"project-setup: {e}\n")
        return 2
    if argv[0] == "work-branch":
        sys.stdout.write(name + "\n")
        return 0
    sys.stdout.flush()
    try:
        return subprocess.run(check_command(argv[0], name)).returncode
    except OSError:
        sys.stderr.write(f"project-setup: cannot run the {argv[0]} check\n")
        return 1


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
