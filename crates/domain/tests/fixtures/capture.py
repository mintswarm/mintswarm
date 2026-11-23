#!/usr/bin/env python3
"""Capture the records the Python self-tests write, as fixtures for the domain crate's record types.

Usage: capture.py OUT [--all]

Runs the plan-approval and landing self-tests unchanged, in this process, with TMPDIR under the
git-ignored `build/fixture-capture/tmp`. After each call of a record tool (`floor.py`, `decision.py`,
`keep-round.py`, `collect-facts.py`) it collects every JSON file under TMPDIR whose bytes changed
during the call and whose `tool` and `version` name one of the five records. It keeps the first
record of each (tool, structural shape), or every record with `--all`, and writes one per line to OUT.

It reads `.mintswarm/dogfood/` and never modifies it. Run it with PYTHONDONTWRITEBYTECODE=1.
"""

import hashlib
import json
import os
import runpy
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[4]
DOGFOOD = ROOT / ".mintswarm" / "dogfood"
SELFTESTS = (
    DOGFOOD / "plan-approval" / "selftest.py",
    DOGFOOD / "landing" / "floor-selftest.py",
    DOGFOOD / "landing" / "facts-selftest.py",
    DOGFOOD / "landing" / "decision-selftest.py",
)
TOOLS = ("floor.py", "decision.py", "keep-round.py", "collect-facts.py")
RECORDS = {
    ("mintswarm-plan-floor", 2),
    ("mintswarm-approval-decision", 2),
    ("mintswarm-keep-round", 2),
    ("mintswarm-landing-facts", 3),
    ("mintswarm-landing-floor", 1),
}
SCRATCH = ROOT / "build" / "fixture-capture" / "tmp"


def signature(value):
    """The structural shape of a JSON value: scalars as their JSON type, lists as their elements' shapes."""
    if isinstance(value, dict):
        return "{" + ",".join(f"{json.dumps(k)}:{signature(v)}" for k, v in value.items()) + "}"
    if isinstance(value, list):
        return "[" + ",".join(sorted({signature(v) for v in value})) + "]"
    if value is None:
        return "null"
    if isinstance(value, bool):
        return "bool"
    if isinstance(value, int):
        return "int"
    if isinstance(value, float):
        return "float"
    return "str"


def scan(top: Path) -> dict:
    """sha256 of every JSON file under top, by path, not descending into `.git` directories."""
    found = {}
    for dirpath, dirnames, filenames in os.walk(top):
        dirnames[:] = [d for d in dirnames if d != ".git"]
        for name in filenames:
            if not name.endswith(".json"):
                continue
            path = os.path.join(dirpath, name)
            try:
                with open(path, "rb") as f:
                    found[path] = hashlib.sha256(f.read()).hexdigest()
            except OSError:
                continue
    return found


def record_of(path: str):
    """The record in path when it is one of the five, else None."""
    try:
        with open(path, "rb") as f:
            value = json.loads(f.read().decode("utf-8"))
    except (OSError, UnicodeDecodeError, ValueError):
        return None
    if not isinstance(value, dict) or (value.get("tool"), value.get("version")) not in RECORDS:
        return None
    return value


class Collector:
    def __init__(self, keep_all: bool):
        self.keep_all = keep_all
        self.seen = set()
        self.records = []
        self.tools = set()

    def add(self, record: dict) -> None:
        self.tools.add(record["tool"])
        key = (record["tool"], signature(record))
        if self.keep_all or key not in self.seen:
            self.seen.add(key)
            self.records.append(record)

    def wrap(self, run):
        def wrapped(*args, **kwargs):
            argv = args[0] if args else kwargs.get("args", [])
            names = [os.path.basename(str(a)) for a in argv] if isinstance(argv, (list, tuple)) else []
            if not any(name in TOOLS for name in names):
                return run(*args, **kwargs)
            before = scan(SCRATCH)
            try:
                return run(*args, **kwargs)
            finally:
                for path, digest in scan(SCRATCH).items():
                    if before.get(path) != digest:
                        record = record_of(path)
                        if record is not None:
                            self.add(record)

        return wrapped


def main(argv: list) -> int:
    if len(argv) not in (1, 2) or (len(argv) == 2 and argv[1] != "--all"):
        print("usage: capture.py OUT [--all]", file=sys.stderr)
        return 2
    out = Path(argv[0])
    shutil.rmtree(SCRATCH, ignore_errors=True)
    SCRATCH.mkdir(parents=True)
    os.environ["TMPDIR"] = str(SCRATCH)
    tempfile.tempdir = str(SCRATCH)
    collector = Collector(len(argv) == 2)
    subprocess.run = collector.wrap(subprocess.run)
    saved_argv = sys.argv
    for script in SELFTESTS:
        sys.argv = [str(script)]
        try:
            runpy.run_path(str(script), run_name="__main__")
            code = 0
        except SystemExit as exc:
            code = exc.code
        finally:
            sys.argv = saved_argv
        if code not in (0, None):
            print(f"capture.py: {script.relative_to(ROOT)} exited with {code}", file=sys.stderr)
            return 1
    missing = sorted(tool for tool, _ in RECORDS if tool not in collector.tools)
    if missing:
        print(f"capture.py: no record captured for {', '.join(missing)}", file=sys.stderr)
        return 1
    out.parent.mkdir(parents=True, exist_ok=True)
    with open(out, "w", encoding="utf-8") as f:
        for record in collector.records:
            f.write(json.dumps(record, ensure_ascii=False) + "\n")
    print(f"capture.py: {len(collector.records)} records written to {out}", file=sys.stderr)
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
