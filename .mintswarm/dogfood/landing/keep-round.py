#!/usr/bin/env python3
"""Keep one gates or review round's results under its round number.

    keep-round.py gates N --head SHA --rounds DIR
    keep-round.py review N --head SHA --rounds DIR [--no-address]

Dogfooding stand-in for the runner's round history. The gates and review
stages overwrite their documents each round, and report-gates.py --clean
deletes the previous round's reports, so each round is copied here before
that happens. Paths other than DIR are relative to the working directory,
which is the worktree root. N is a positive integer without leading zeros;
gate and review rounds are numbered independently, each from 1. The options
come after `kind N`, in any order, each once.

A gates round copies the round's input, .mintswarm/job/inputs/gates.json,
as gates.input.json, so that collect-facts.py can apply every condition of
run-gates.py --verdict, including the gate count and names. It also copies
whichever of .mintswarm/job/outputs/gates.ctrf.json and
.mintswarm/job/outputs/gates.md exist. A round with neither holds only the
input, with a warning, so that a gates stage that failed still counts as a
round. It refuses when inputs/gates.json is missing, or when either output
is older than it (a report or document left from an earlier round), which
is run-gates.py --verdict's staleness condition, checked here because the
kept copies' own times say nothing.

A review round copies .mintswarm/job/outputs/review.md, which is required
and must be titled `# Review round N:`, and .mintswarm/job/outputs/address.md
when it exists and is titled `# Address round N:` (address round N answers
review round N's findings). An address document with any other title is
refused, unless --no-address is given, which ignores the file; the final
review round has no address and is kept with --no-address.

Where rounds are kept. DIR is the job's runner state, outside every
repository; the session passes
${XDG_STATE_HOME:-$HOME/.local/state}/mintswarm/jobs/<worktree name>/rounds/.
DIR must be absolute and not a symbolic link, and, resolved, must be
neither the resolved working directory nor under it, nor the main checkout
(the parent of `git rev-parse --path-format=absolute --git-common-dir`,
resolved) nor under it. There is no default inside the worktree. DIR is
created with mode 0700 when missing.

The head. SHA is the full hash of the commit the round judged, and
`git rev-parse --verify --quiet --end-of-options SHA^{commit}` must return
it unchanged. For a gates round it must be HEAD, and
`git status --porcelain=v1 -z --untracked-files=all --ignore-submodules=none`
must list no path outside .mintswarm/job/, so that the recorded commit is
exactly what the gates ran on. For a review round, `git merge-base
--is-ancestor SHA HEAD` must succeed: the round judged the head of the
diff.patch it was given, and address commits may follow it.

Trust boundary. On one account, keeping DIR outside the worktree and the
main checkout is a convention the stage rules hold, not an OS barrier: any
process of that account can write DIR. Mode 0444 and the sha256 values in
round.json detect only accidental change. The binding to a commit is what
lets the landing floor reject a stale round: a last gates or review round
whose head is not the change's head is not passed, however it was edited.

The round is kept in DIR/<kind>-<N>/ (gates-N or review-N). The files keep
their names and are made mode 0444; round.json records, in this order,
`tool` (`mintswarm-keep-round`), `version` (2), `kind`, `round`, `head`
(SHA), `kept_at` (UTC, YYYY-MM-DDTHH:MM:SSZ) and `files` ({`name`,
`sha256`} per copied file, sorted by name). Everything is written to
DIR/.tmp-<kind>-<N>-<random>/ first, the final name is checked to be
absent, and the temporary directory is renamed onto it, so a round is kept
completely or not at all. The temporary directory is removed on any
failure. A kept round is never changed: keeping the same kind and number
again fails.

Session sequence. report-gates.py's docstring numbers the runner's steps in
every gates round (1 --check, 2 --clean, 3 write inputs/gates.json, 4
dispatch gates and save gates.md, 5 run-gates.py --verdict, 6
report-gates.py). The dogfooding session adds:

- Gates round N is kept with `keep-round.py gates N --head SHA --rounds
  DIR` after step 6, and in any case before step 2 (report-gates.py
  --clean) of gates round N+1. SHA is the HEAD the session dispatched the
  gates at.
- Review round N is kept with `keep-round.py review N --head SHA --rounds
  DIR` after review.md, and address.md if the implementer answered the
  findings, are saved, and before review round N+1 is dispatched, that is,
  before the next review round overwrites review.md. SHA is the head of the
  diff.patch the session gave review round N.
- The last round of each kind is kept before collect-facts.py runs; the
  last review round with --no-address. The last gates round and the last
  review round must be at the final head, or the landing floor hits.
- Before the landing stage is dispatched, the session copies DIR read-only
  to .mintswarm/job/inputs/rounds/ of the landing job, as data for the
  judge only; collect-facts.py and the floor never read that copy.

run-gates.py and report-gates.py are unchanged by this: the verdict of a
round is still run-gates.py --verdict while the round runs, and the kept
copies are read later by collect-facts.py.

Exits 0 when the round is kept; 1 when DIR/<kind>-<N> already exists,
changing nothing; 2 on unusable input (bad arguments, a DIR in the wrong
place, a SHA that is not the head or not its ancestor, a dirty worktree for
a gates round, a missing or stale file, a wrong title), a failing git or an
I/O failure, leaving nothing at the final path. Standard library only,
apart from running git.
"""

import hashlib
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
from datetime import datetime, timezone
from pathlib import Path

JOB = Path(".mintswarm/job")
OUTPUTS = JOB / "outputs"
GATES_INPUT = JOB / "inputs" / "gates.json"
TOOL = "mintswarm-keep-round"
VERSION = 2
GATES_FILES = ("gates.ctrf.json", "gates.md")
GATES_INPUT_COPY = "gates.input.json"
REVIEW = "review.md"
ADDRESS = "address.md"
ROUND_NUMBER = re.compile(r"[1-9][0-9]*")
FULL_HASH = re.compile(r"[0-9a-f]{40}|[0-9a-f]{64}")
USAGE = ("usage: keep-round.py gates N --head SHA --rounds DIR | "
         "keep-round.py review N --head SHA --rounds DIR [--no-address]")


class InputError(Exception):
    """Unusable input or an I/O failure: exit 2."""


class AlreadyKept(Exception):
    """The round is already kept: exit 1."""


def parse_args(argv: list) -> tuple:
    """Return (kind, number, head, rounds, no_address)."""
    if len(argv) < 2 or argv[0] not in ("gates", "review"):
        raise InputError(USAGE)
    kind, number, rest = argv[0], argv[1], argv[2:]
    if not ROUND_NUMBER.fullmatch(number):
        raise InputError(f"N must be a positive integer, got {number!r}")
    values, i = {}, 0
    while i < len(rest):
        name = rest[i]
        if name in values or name not in ("--head", "--rounds", "--no-address") \
                or (name == "--no-address" and kind != "review"):
            raise InputError(USAGE)
        if name == "--no-address":
            values[name], i = True, i + 1
            continue
        if i + 1 >= len(rest):
            raise InputError(f"{name} needs a value; {USAGE}")
        values[name], i = rest[i + 1], i + 2
    missing = [name for name in ("--head", "--rounds") if name not in values]
    if missing:
        raise InputError(f"missing {', '.join(missing)}; {USAGE}")
    return kind, int(number), values["--head"], values["--rounds"], "--no-address" in values


def git(*args: str) -> tuple:
    """Run git in the working directory: (exit code, stdout as text)."""
    try:
        proc = subprocess.run(["git", *args], stdin=subprocess.DEVNULL, capture_output=True)
    except OSError as exc:
        raise InputError(f"cannot run git: {exc}") from exc
    return proc.returncode, proc.stdout.decode("utf-8", errors="replace")


def rounds_dir(value: str) -> Path:
    """Check DIR's location: absolute, not a link, outside the worktree and the main checkout."""
    path = Path(value)
    if not path.is_absolute():
        raise InputError(f"--rounds must be an absolute path, got {value!r}")
    if path.is_symlink():
        raise InputError(f"--rounds {value} is a symbolic link")
    resolved = path.resolve()
    code, out = git("rev-parse", "--path-format=absolute", "--git-common-dir")
    if code != 0 or not out.strip():
        raise InputError("git rev-parse --git-common-dir failed: the working directory is not a git checkout")
    for name, place in (("the worktree", Path.cwd().resolve()),
                        ("the main checkout", Path(out.strip()).resolve().parent)):
        if resolved == place or place in resolved.parents:
            raise InputError(f"--rounds {value} is inside {name} {place}; kept rounds live in runner state")
    return path


def check_head(kind: str, head: str) -> None:
    """Check that SHA is the commit a round of this kind can have judged."""
    if not FULL_HASH.fullmatch(head):
        raise InputError(f"--head must be a full commit hash, got {head!r}")
    code, out = git("rev-parse", "--verify", "--quiet", "--end-of-options", f"{head}^{{commit}}")
    if code != 0 or out.strip() != head:
        raise InputError(f"--head {head} is not a commit of this repository")
    if kind == "review":
        code, _ = git("merge-base", "--is-ancestor", head, "HEAD")
        if code != 0:
            raise InputError(f"--head {head} is not HEAD or an ancestor of it")
        return
    code, out = git("rev-parse", "--verify", "HEAD")
    if code != 0 or out.strip() != head:
        raise InputError(f"--head {head} is not HEAD {out.strip()}: a gates round is kept at the commit it ran on")
    code, out = git("status", "--porcelain=v1", "-z", "--untracked-files=all", "--ignore-submodules=none")
    if code != 0:
        raise InputError("git status failed")
    records, dirty = out.split("\0"), []
    i = 0
    while i < len(records):
        record = records[i]
        i += 1
        if not record:
            continue
        paths = [record[3:]]
        if "R" in record[:2] or "C" in record[:2]:  # a rename or copy is followed by its source path
            paths.append(records[i] if i < len(records) else "")
            i += 1
        dirty += [path for path in paths if not path.startswith(".mintswarm/job/")]
    if dirty:
        raise InputError(f"the worktree has changes outside .mintswarm/job/, so HEAD is not what the gates ran on: "
                         f"{', '.join(sorted(dirty)[:10])}")


def mtime_ns(path: Path) -> int:
    try:
        return path.stat().st_mtime_ns
    except OSError as exc:
        raise InputError(f"cannot stat {path}: {exc}") from exc


def read_bytes(path: Path) -> bytes:
    try:
        return path.read_bytes()
    except OSError as exc:
        raise InputError(f"cannot read {path}: {exc}") from exc


def is_file(path: Path) -> bool:
    if path.is_symlink():
        raise InputError(f"{path} is a symbolic link")
    if path.exists() and not path.is_file():
        raise InputError(f"{path} is not a regular file")
    return path.is_file()


def title(data: bytes, path: Path) -> str:
    try:
        text = data.decode("utf-8")
    except UnicodeDecodeError as exc:
        raise InputError(f"{path} is not UTF-8: {exc}") from exc
    return text.split("\n", 1)[0]


def gates_files() -> list:
    if not is_file(GATES_INPUT):
        raise InputError(f"{GATES_INPUT} is missing: no gates round is under way")
    started = mtime_ns(GATES_INPUT)
    files = [(GATES_INPUT_COPY, read_bytes(GATES_INPUT))]
    for name in GATES_FILES:
        path = OUTPUTS / name
        if not is_file(path):
            continue
        if mtime_ns(path) < started:
            raise InputError(f"stale {path}: it is older than {GATES_INPUT}")
        files.append((name, read_bytes(path)))
    if len(files) == 1:
        print(f"keep-round: warning: neither {' nor '.join(GATES_FILES)} exists; "
              "keeping a gates round with only its input", file=sys.stderr)
    return files


def review_files(number: int, no_address: bool) -> list:
    review = OUTPUTS / REVIEW
    if not is_file(review):
        raise InputError(f"{review} is missing")
    data = read_bytes(review)
    if not title(data, review).startswith(f"# Review round {number}:"):
        raise InputError(f"{review} is not titled '# Review round {number}:'")
    files = [(REVIEW, data)]
    address = OUTPUTS / ADDRESS
    if not no_address and is_file(address):
        data = read_bytes(address)
        if not title(data, address).startswith(f"# Address round {number}:"):
            raise InputError(
                f"{address} is not titled '# Address round {number}:'; "
                "give --no-address when this review round has no address"
            )
        files.append((ADDRESS, data))
    return files


def write_file(path: Path, data: bytes) -> None:
    with open(path, "xb") as out:
        out.write(data)
        out.flush()
        os.fsync(out.fileno())
    os.chmod(path, 0o444)


def keep(rounds: Path, kind: str, number: int, head: str, files: list) -> Path:
    final = rounds / f"{kind}-{number}"
    try:
        os.makedirs(rounds, mode=0o700, exist_ok=True)
        tmp = Path(tempfile.mkdtemp(prefix=f".tmp-{kind}-{number}-", dir=rounds))
    except OSError as exc:
        raise InputError(f"cannot create a temporary directory in {rounds}: {exc}") from exc
    try:
        record = {
            "tool": TOOL,
            "version": VERSION,
            "kind": kind,
            "round": number,
            "head": head,
            "kept_at": datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
            "files": [
                {"name": name, "sha256": hashlib.sha256(data).hexdigest()}
                for name, data in sorted(files)
            ],
        }
        for name, data in files:
            write_file(tmp / name, data)
        write_file(tmp / "round.json", (json.dumps(record, indent=2) + "\n").encode("utf-8"))
        os.chmod(tmp, 0o755)
        if os.path.lexists(final):
            raise AlreadyKept(f"{final} already exists")
        try:
            os.rename(tmp, final)
        except OSError as exc:
            if os.path.lexists(final):
                raise AlreadyKept(f"{final} already exists") from exc
            raise
    except BaseException as exc:
        shutil.rmtree(tmp, ignore_errors=True)
        if isinstance(exc, OSError):
            raise InputError(f"cannot keep {final}: {exc}") from exc
        raise
    return final


def keep_round(argv: list) -> int:
    kind, number, head, value, no_address = parse_args(argv)
    rounds = rounds_dir(value)
    check_head(kind, head)
    final = rounds / f"{kind}-{number}"
    if os.path.lexists(final):
        raise AlreadyKept(f"{final} already exists")
    files = gates_files() if kind == "gates" else review_files(number, no_address)
    final = keep(rounds, kind, number, head, files)
    names = ", ".join(name for name, _ in sorted(files)) or "no files"
    print(f"kept {final} at {head}: {names}")
    return 0


def main(argv: list) -> int:
    try:
        return keep_round(argv)
    except AlreadyKept as exc:
        print(f"keep-round: {exc}; a kept round is never changed", file=sys.stderr)
        return 1
    except InputError as exc:
        print(f"keep-round: {exc}", file=sys.stderr)
        return 2
    except Exception as exc:  # a crash must never read as kept or already kept
        print(f"keep-round: unexpected error: {type(exc).__name__}: {exc}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
