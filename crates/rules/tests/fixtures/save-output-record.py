#!/usr/bin/env python3
"""Record every save-output.py call that save-output-selftest.py makes, for the rules crate's parity test.

Usage: save-output-record.py OUT

Loads `.mintswarm/dogfood/save-output-selftest.py` as a module, without writing bytecode, wraps
`subprocess.run` and runs the whole suite in-process. For each call whose script argument is
`save-output.py` it writes one JSON line (ASCII only) to OUT while the test's temporary directory
still exists:

- `test`: the test, as `Class.method`;
- `mode`: `text` (with `--text`) or `transcript`;
- `input`: the input file's text, read with `newline=""`;
- `output`: the output argument as passed;
- `exit`, `stdout` and `stderr`: what the call gave;
- `document` and `record`: the saved document and its `.result.json` record, read with
  `newline=""`, on exit 0; otherwise `null`.

The last line is `{"unrecorded": [...]}`: the tests that made no save-output.py call. Exits 1 if
any test failed or errored. Reads `.mintswarm/dogfood/` without modifying it and writes only OUT
and the self-test's own temporary directories.

`tests/save_output.rs` runs it into a temporary file and replays every entry through
`mintswarm_rules::output`.
"""

import importlib.util
import json
import subprocess
import sys
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[4]
SELFTEST = ROOT / ".mintswarm" / "dogfood" / "save-output-selftest.py"


def load(path: Path):
    spec = importlib.util.spec_from_file_location("save_output_selftest", path)
    module = importlib.util.module_from_spec(spec)
    dont_write, sys.dont_write_bytecode = sys.dont_write_bytecode, True
    try:
        spec.loader.exec_module(module)
    finally:
        sys.dont_write_bytecode = dont_write
    return module


def read_text(path: Path):
    try:
        with path.open(encoding="utf-8", newline="") as f:
            return f.read()
    except OSError:
        return None


def record_path(output: Path) -> Path:
    stem = output.name[:-3] if output.name.endswith(".md") else output.name
    return output.with_name(stem + ".result.json")


def test_name(test) -> str:
    return ".".join(test.id().split(".")[-2:])


class Recorder:
    def __init__(self, out):
        self.out = out
        self.current = None
        self.recorded = set()
        self.real_run = subprocess.run

    def run(self, cmd, *args, **kwargs):
        proc = self.real_run(cmd, *args, **kwargs)
        if isinstance(cmd, (list, tuple)) and len(cmd) >= 3 and Path(str(cmd[2])).name == "save-output.py":
            self.write(cmd[3:], kwargs.get("cwd"), proc)
        return proc

    def write(self, argv, cwd, proc):
        base = Path(cwd) if cwd is not None else Path.cwd()
        text_mode = bool(argv) and argv[0] == "--text"
        source, output = str(argv[-2]), str(argv[-1])
        out_path = base / output
        saved = proc.returncode == 0
        entry = {
            "test": self.current,
            "mode": "text" if text_mode else "transcript",
            "input": read_text(base / source),
            "output": output,
            "exit": proc.returncode,
            "stdout": proc.stdout,
            "stderr": proc.stderr,
            "document": read_text(out_path) if saved else None,
            "record": read_text(record_path(out_path)) if saved else None,
        }
        self.out.write(json.dumps(entry, ensure_ascii=True) + "\n")
        self.recorded.add(self.current)


def tests_of(suite):
    for item in suite:
        if isinstance(item, unittest.TestSuite):
            yield from tests_of(item)
        else:
            yield item


def main(argv: list) -> int:
    if len(argv) != 1:
        print("usage: save-output-record.py OUT", file=sys.stderr)
        return 2
    module = load(SELFTEST)
    suite = unittest.defaultTestLoader.loadTestsFromModule(module)
    names = [test_name(t) for t in tests_of(suite)]
    with open(argv[0], "w", encoding="utf-8", newline="") as out:
        recorder = Recorder(out)

        class Result(unittest.TextTestResult):
            def startTest(self, test):
                recorder.current = test_name(test)
                super().startTest(test)

        subprocess.run = recorder.run
        try:
            result = unittest.TextTestRunner(stream=sys.stderr, resultclass=Result, verbosity=1).run(suite)
        finally:
            subprocess.run = recorder.real_run
        unrecorded = [name for name in names if name not in recorder.recorded]
        out.write(json.dumps({"unrecorded": unrecorded}, ensure_ascii=True) + "\n")
    return 0 if result.wasSuccessful() else 1


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
