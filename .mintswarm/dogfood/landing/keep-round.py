#!/usr/bin/env python3
"""Keep one gates or review round's results under its round number.

    keep-round.py gates N
    keep-round.py review N [--no-address]

Dogfooding stand-in for the runner's round history. The gates and review
stages overwrite their documents each round, and report-gates.py --clean
deletes the previous round's reports, so each round is copied here before
that happens. Paths are relative to the working directory, which is the
worktree root. N is a positive integer without leading zeros; gate and
review rounds are numbered independently, each from 1.

A gates round copies whichever of .mintswarm/job/outputs/gates.ctrf.json and
.mintswarm/job/outputs/gates.md exist. A round with neither is kept empty,
with a warning, so that a gates stage that failed still counts as a round.
It refuses when .mintswarm/job/inputs/gates.json is missing, or when either
file is older than it (a report or document left from an earlier round).

A review round copies .mintswarm/job/outputs/review.md, which is required
and must be titled `# Review round N:`, and .mintswarm/job/outputs/address.md
when it exists and is titled `# Address round N:` (address round N answers
review round N's findings). An address document with any other title is
refused, unless --no-address is given, which ignores the file; the final
review round has no address and is kept with --no-address.

The round is kept in .mintswarm/job/rounds/<kind>-<N>/ (gates-N or
review-N), outside outputs/, so report-gates.py --clean and the runner's
publishing never touch it. The files keep their names and are made mode
0444; round.json records, in this order, `tool` (`mintswarm-keep-round`),
`version` (1), `kind`, `round`, `kept_at` (UTC, YYYY-MM-DDTHH:MM:SSZ) and
`files` ({`name`, `sha256`} per copied file, sorted by name). Everything is
written to .mintswarm/job/rounds/.tmp-<kind>-<N>-<random>/ first, the final
name is checked to be absent, and the temporary directory is renamed onto
it, so a round is kept completely or not at all. The temporary directory is
removed on any failure. A kept round is never changed: keeping the same
kind and number again fails.

Session sequence. report-gates.py's docstring numbers the runner's steps in
every gates round (1 --check, 2 --clean, 3 write inputs/gates.json, 4
dispatch gates and save gates.md, 5 run-gates.py --verdict, 6
report-gates.py). The dogfooding session adds:

- Gates round N is kept with `keep-round.py gates N` after step 6, and in
  any case before step 2 (report-gates.py --clean) of gates round N+1.
- Review round N is kept with `keep-round.py review N` after review.md, and
  address.md if the implementer answered the findings, are saved, and
  before review round N+1 is dispatched, that is, before the next review
  round overwrites review.md.
- The last round of each kind is kept before collect-facts.py runs; the
  last review round with --no-address.

run-gates.py and report-gates.py are unchanged by this: the verdict of a
round is still run-gates.py --verdict while the round runs, and the kept
copies are read later by collect-facts.py.

Exits 0 when the round is kept; 1 when .mintswarm/job/rounds/<kind>-<N>
already exists, changing nothing; 2 on unusable input (bad arguments, a
missing or stale file, a wrong title) or an I/O failure, leaving nothing at
the final path. Standard library only.
"""

import hashlib
import json
import os
import re
import shutil
import sys
import tempfile
from datetime import datetime, timezone
from pathlib import Path

JOB = Path(".mintswarm/job")
OUTPUTS = JOB / "outputs"
GATES_INPUT = JOB / "inputs" / "gates.json"
ROUNDS = JOB / "rounds"
TOOL = "mintswarm-keep-round"
VERSION = 1
GATES_FILES = ("gates.ctrf.json", "gates.md")
REVIEW = "review.md"
ADDRESS = "address.md"
ROUND_NUMBER = re.compile(r"[1-9][0-9]*")
USAGE = "usage: keep-round.py gates N | keep-round.py review N [--no-address]"


class InputError(Exception):
    """Unusable input or an I/O failure: exit 2."""


class AlreadyKept(Exception):
    """The round is already kept: exit 1."""


def parse_args(argv: list) -> tuple:
    if len(argv) < 2 or argv[0] not in ("gates", "review"):
        raise InputError(USAGE)
    kind, number, rest = argv[0], argv[1], argv[2:]
    if not ROUND_NUMBER.fullmatch(number):
        raise InputError(f"N must be a positive integer, got {number!r}")
    if kind == "gates" and rest:
        raise InputError(USAGE)
    if kind == "review" and rest not in ([], ["--no-address"]):
        raise InputError(USAGE)
    return kind, int(number), rest == ["--no-address"]


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
    if not GATES_INPUT.is_file():
        raise InputError(f"{GATES_INPUT} is missing: no gates round is under way")
    started = mtime_ns(GATES_INPUT)
    files = []
    for name in GATES_FILES:
        path = OUTPUTS / name
        if not is_file(path):
            continue
        if mtime_ns(path) < started:
            raise InputError(f"stale {path}: it is older than {GATES_INPUT}")
        files.append((name, read_bytes(path)))
    if not files:
        print(f"keep-round: warning: neither {' nor '.join(GATES_FILES)} exists; keeping an empty gates round",
              file=sys.stderr)
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


def keep(kind: str, number: int, files: list) -> Path:
    final = ROUNDS / f"{kind}-{number}"
    try:
        ROUNDS.mkdir(parents=True, exist_ok=True)
        tmp = Path(tempfile.mkdtemp(prefix=f".tmp-{kind}-{number}-", dir=ROUNDS))
    except OSError as exc:
        raise InputError(f"cannot create a temporary directory in {ROUNDS}: {exc}") from exc
    try:
        record = {
            "tool": TOOL,
            "version": VERSION,
            "kind": kind,
            "round": number,
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
    kind, number, no_address = parse_args(argv)
    final = ROUNDS / f"{kind}-{number}"
    if os.path.lexists(final):
        raise AlreadyKept(f"{final} already exists")
    files = gates_files() if kind == "gates" else review_files(number, no_address)
    final = keep(kind, number, files)
    names = ", ".join(name for name, _ in sorted(files)) or "no files"
    print(f"kept {final}: {names}")
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
