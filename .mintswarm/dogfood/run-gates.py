#!/usr/bin/env python3
"""Run a job's gates and write a machine-readable verdict.

Dogfooding stand-in for the runner's GateRunner. Reads the gates the runner
prepared in .mintswarm/job/inputs/gates.json:

    {"gates": [{"name": "whitespace", "command": "git diff --check origin/master...HEAD",
                "timeout": 600}]}

runs each one with bash from the worktree root, and writes
.mintswarm/job/outputs/gates.json:

    {"passed": false, "started_at": "...", "gates": [{"name": ..., "command": ...,
     "exit_code": 1, "passed": false, "timed_out": false, "duration_s": 0.4,
     "output_tail": "..."}]}

Exits 0 when every gate passed, 1 when any failed, 2 when the input is unusable.
The verdict is this file, never a summary written by an agent.
Standard library only.
"""

import json
import subprocess
import sys
import time
from datetime import datetime, timezone
from pathlib import Path

JOB = Path(".mintswarm/job")
INPUT = JOB / "inputs" / "gates.json"
OUTPUT = JOB / "outputs" / "gates.json"
DEFAULT_TIMEOUT_S = 1800
TAIL_LINES = 200


def tail(text: str) -> str:
    lines = text.splitlines()
    return "\n".join(lines[-TAIL_LINES:])


def run_gate(gate: dict) -> dict:
    name, command = gate["name"], gate["command"]
    timeout = gate.get("timeout", DEFAULT_TIMEOUT_S)
    started = time.monotonic()
    try:
        proc = subprocess.run(
            ["bash", "-c", command],
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
            timeout=timeout,
        )
        exit_code, output, timed_out = proc.returncode, proc.stdout, False
    except subprocess.TimeoutExpired as exc:
        out = exc.stdout or ""
        if isinstance(out, bytes):
            out = out.decode(errors="replace")
        exit_code, output, timed_out = None, out, True
    return {
        "name": name,
        "command": command,
        "exit_code": exit_code,
        "passed": exit_code == 0,
        "timed_out": timed_out,
        "duration_s": round(time.monotonic() - started, 1),
        "output_tail": tail(output),
    }


def main() -> int:
    try:
        spec = json.loads(INPUT.read_text())
        gates = spec["gates"]
        if not gates:
            raise ValueError("no gates defined")
        for gate in gates:
            if not gate.get("name") or not gate.get("command"):
                raise ValueError(f"gate without name or command: {gate!r}")
    except (OSError, ValueError, KeyError, TypeError) as exc:
        print(f"run-gates: unusable {INPUT}: {exc}", file=sys.stderr)
        return 2

    started_at = datetime.now(timezone.utc).isoformat(timespec="seconds")
    results = [run_gate(gate) for gate in gates]
    verdict = {
        "passed": all(r["passed"] for r in results),
        "started_at": started_at,
        "gates": results,
    }
    OUTPUT.parent.mkdir(parents=True, exist_ok=True)
    OUTPUT.write_text(json.dumps(verdict, indent=2) + "\n")

    for r in results:
        state = "pass" if r["passed"] else ("timeout" if r["timed_out"] else "FAIL")
        print(f"{state:7} {r['name']} ({r['duration_s']}s)")
    return 0 if verdict["passed"] else 1


if __name__ == "__main__":
    sys.exit(main())
