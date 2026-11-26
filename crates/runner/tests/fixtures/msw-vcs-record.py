#!/usr/bin/env python3
"""Record every case of the msw-vcs self-test through the shim and the Rust tool.

    msw-vcs-record.py SELFTEST BIN OUTPUT

Loads SELFTEST, .mintswarm/dogfood/msw-vcs-selftest.py, without running its
main, and builds its fixture in a temporary directory. For every entry of its
CASES, the error cases included, it arranges the case, runs the shim through
the fixture, runs `BIN tool msw-vcs` with the same filled argv in the same
working directory and environment, and restores the fixture. Writes one JSON
line per case to OUTPUT, with the case's name and, for each side, the hex of
its stdout and stderr and its exit code, then a final line with the number of
cases. Standard library only.
"""

import importlib.util
import json
import os
import subprocess
import sys
import tempfile
from pathlib import Path


def load(path: Path):
    spec = importlib.util.spec_from_file_location("msw_vcs_selftest", path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def side(out: bytes, err: bytes, code: int) -> dict:
    return {"stdout": out.hex(), "stderr": err.hex(), "exit": code}


def main(argv: list) -> int:
    if len(argv) != 3:
        print("usage: msw-vcs-record.py SELFTEST BIN OUTPUT", file=sys.stderr)
        return 2
    selftest, binary, output = Path(argv[0]), argv[1], Path(argv[2])
    module = load(selftest)
    lines = []
    with tempfile.TemporaryDirectory(prefix="msw-vcs-record.") as tmp:
        fixture = module.Fixture(Path(tmp))
        for case in module.CASES:
            vcs_json = case.get("vcs_json", module.DEFAULT_VCS_JSON)
            fixture.arrange(case)
            try:
                shim = fixture.tool(case["argv"], vcs_json)
                # fixture.tool left vcs.json as the case needs it
                proc = subprocess.run(
                    [binary, "tool", "msw-vcs", *[os.fsencode(fixture.fill(arg)) for arg in case["argv"]]],
                    cwd=fixture.repo,
                    env=dict(fixture.env, PYTHONDONTWRITEBYTECODE="1"),
                    stdin=subprocess.DEVNULL,
                    stdout=subprocess.PIPE,
                    stderr=subprocess.PIPE,
                )
            finally:
                fixture.restore(case)
            lines.append(
                {
                    "name": case["name"],
                    "shim": side(*shim),
                    "rust": side(proc.stdout, proc.stderr, proc.returncode),
                }
            )
    with open(output, "w", encoding="utf-8") as handle:
        for line in lines:
            handle.write(json.dumps(line) + "\n")
        handle.write(json.dumps({"cases": len(lines)}) + "\n")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
