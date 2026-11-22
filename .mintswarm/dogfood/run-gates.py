#!/usr/bin/env python3
"""Run a job's gates and record the result as a CTRF report.

Dogfooding stand-in for the runner's GateRunner. Paths are relative to the
working directory, which is the worktree root. The input is the gates the
runner prepared in .mintswarm/job/inputs/gates.json:

    {"gates": [{"name": "whitespace", "command": "git diff --check origin/master...HEAD",
                "timeout": 600}]}

`timeout` is in seconds and optional (default 1800).

    run-gates.py              run every gate with `bash -c` and write the report
    run-gates.py --verdict    the runner's verdict, read from the report; runs nothing
    run-gates.py --failures   the failed gates as plain text, for the gates agent

Running the gates first deletes any stale report, then writes
.mintswarm/job/outputs/gates.ctrf.json, a CTRF 0.3.0 report
(https://ctrf.io): `results.tool.name` is `mintswarm-gates` and each gate is
one test, `passed` or `failed`. A failed gate carries `message` (`exit code N`
or `timed out after N s`) and `trace` (the last 200 lines of its combined
stdout and stderr, left out when empty); a timeout also has `rawStatus`
`timeout`. Every test has the command, exit code (null on timeout), timeout
flag and timeout in `extra["mintswarm.gate"]`. One line per gate is printed.
Exits 0 when every gate passed, 1 when any failed or timed out, 2 when the
input is unusable.

--verdict exits 0 only when the report exists and parses, is CTRF, has no
`results.errors`, counts as many tests as the input has gates (at least one)
and as `results.tests` holds, and every test passed with none failed or other.
Otherwise it prints the first unmet condition and exits 1. It exits 2 when the
report exists but the input is unusable. The verdict is this, never a summary
written by an agent.

--failures prints, for each failed gate, `== <name>: <message>` and then its
trace verbatim, or `no failed gates`. Exits 0 when printed, 2 when the report
is missing or unreadable.

Other report formats are written by converters that the runner runs
afterwards; see report-gates.py. Standard library only.
"""

import json
import subprocess
import sys
import time
import uuid
from datetime import datetime, timezone
from pathlib import Path

JOB = Path(".mintswarm/job")
INPUT = JOB / "inputs" / "gates.json"
REPORT = JOB / "outputs" / "gates.ctrf.json"
DEFAULT_TIMEOUT_S = 1800
TAIL_LINES = 200
TOOL = "mintswarm-gates"
EXTRA_KEY = "mintswarm.gate"
USAGE = "usage: run-gates.py [--verdict | --failures]"


class InputError(Exception):
    pass


def now_ms() -> int:
    return int(time.time() * 1000)


def tail(text: str) -> str:
    lines = text.splitlines()
    return "\n".join(lines[-TAIL_LINES:])


def seconds(value) -> str:
    return f"{value:g}"


def load_gates() -> list:
    try:
        spec = json.loads(INPUT.read_text(encoding="utf-8"))
        gates = spec["gates"]
        if not isinstance(gates, list) or not gates:
            raise InputError("no gates defined")
        for gate in gates:
            if not isinstance(gate, dict) or not gate.get("name") or not gate.get("command"):
                raise InputError(f"gate without name or command: {gate!r}")
    except (OSError, ValueError, KeyError, TypeError, InputError) as exc:
        raise InputError(f"unusable {INPUT}: {exc}") from exc
    return gates


def run_gate(gate: dict) -> dict:
    name, command = gate["name"], gate["command"]
    timeout = gate.get("timeout", DEFAULT_TIMEOUT_S)
    start = now_ms()
    started = time.monotonic()
    try:
        proc = subprocess.run(
            ["bash", "-c", command],
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            timeout=timeout,
        )
        exit_code, raw, timed_out = proc.returncode, proc.stdout, False
    except subprocess.TimeoutExpired as exc:
        exit_code, raw, timed_out = None, exc.stdout, True
    duration = int((time.monotonic() - started) * 1000)
    stop = now_ms()
    output = (raw or b"").decode("utf-8", errors="replace")

    test = {
        "name": name,
        "status": "passed" if exit_code == 0 else "failed",
        "duration": duration,
        "start": start,
        "stop": stop,
    }
    if test["status"] == "failed":
        test["message"] = f"timed out after {seconds(timeout)} s" if timed_out else f"exit code {exit_code}"
        trace = tail(output)
        if trace:
            test["trace"] = trace
        if timed_out:
            test["rawStatus"] = "timeout"
    test["extra"] = {
        EXTRA_KEY: {
            "command": command,
            "exitCode": exit_code,
            "timedOut": timed_out,
            "timeoutSeconds": timeout,
        }
    }
    return test


def run() -> int:
    REPORT.unlink(missing_ok=True)
    try:
        gates = load_gates()
    except InputError as exc:
        print(f"run-gates: {exc}", file=sys.stderr)
        return 2

    timestamp = datetime.now(timezone.utc).isoformat(timespec="seconds")
    start = now_ms()
    started = time.monotonic()
    tests = [run_gate(gate) for gate in gates]
    duration = int((time.monotonic() - started) * 1000)
    stop = now_ms()
    passed = sum(1 for t in tests if t["status"] == "passed")
    report = {
        "reportFormat": "CTRF",
        "specVersion": "0.3.0",
        "reportId": str(uuid.uuid4()),
        "timestamp": timestamp,
        "generatedBy": "mintswarm run-gates.py",
        "results": {
            "tool": {"name": TOOL},
            "summary": {
                "tests": len(tests),
                "passed": passed,
                "failed": len(tests) - passed,
                "skipped": 0,
                "pending": 0,
                "other": 0,
                "start": start,
                "stop": stop,
                "duration": duration,
            },
            "tests": tests,
        },
    }
    REPORT.parent.mkdir(parents=True, exist_ok=True)
    REPORT.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")

    for t in tests:
        if t["status"] == "passed":
            state = "pass"
        elif t.get("rawStatus") == "timeout":
            state = "timeout"
        else:
            state = "FAIL"
        print(f"{state:7} {t['name']} ({t['duration'] / 1000:.1f}s)")
    return 0 if passed == len(tests) else 1


def load_report():
    """Return the parsed report, or None when it is missing; raises ValueError when unreadable."""
    try:
        text = REPORT.read_text(encoding="utf-8")
    except FileNotFoundError:
        return None
    except (OSError, UnicodeDecodeError) as exc:
        raise ValueError(f"cannot read {REPORT}: {exc}") from exc
    try:
        return json.loads(text)
    except ValueError as exc:
        raise ValueError(f"{REPORT} does not parse: {exc}") from exc


def unmet_condition():
    """Return the first unmet verdict condition, or None when every gate passed."""
    try:
        report = load_report()
    except ValueError as exc:
        return str(exc)
    if report is None:
        return f"no report: {REPORT} does not exist"
    if not isinstance(report, dict) or report.get("reportFormat") != "CTRF":
        return "reportFormat is not CTRF"
    results = report.get("results")
    if not isinstance(results, dict):
        return "the report has no results"
    if "errors" in results:
        return "results.errors is present"
    summary = results.get("summary")
    tests = results.get("tests")
    if not isinstance(summary, dict) or not isinstance(tests, list):
        return "the report has no results.summary or results.tests"
    counts = {}
    for key in ("tests", "passed", "failed", "other"):
        value = summary.get(key)
        if not isinstance(value, int) or isinstance(value, bool):
            return f"summary.{key} is not an integer"
        counts[key] = value
    gates = load_gates()
    if counts["tests"] != len(gates) or counts["tests"] <= 0:
        return f"summary.tests is {counts['tests']}, the input has {len(gates)} gates"
    if counts["tests"] != len(tests):
        return f"summary.tests is {counts['tests']}, results.tests holds {len(tests)}"
    if counts["passed"] != counts["tests"]:
        return f"summary.passed is {counts['passed']} of {counts['tests']}"
    if counts["failed"] != 0 or counts["other"] != 0:
        return f"summary.failed is {counts['failed']}, summary.other is {counts['other']}"
    return None


def verdict() -> int:
    try:
        unmet = unmet_condition()
    except InputError as exc:
        print(f"run-gates: {exc}", file=sys.stderr)
        return 2
    if unmet:
        print(f"gates failed: {unmet}")
        return 1
    print("gates passed")
    return 0


def failures() -> int:
    try:
        report = load_report()
        if report is None:
            raise ValueError(f"no report: {REPORT} does not exist")
        tests = report["results"]["tests"]
        if not isinstance(tests, list):
            raise TypeError("results.tests is not a list")
        failed = [t for t in tests if t.get("status") in ("failed", "other")]
    except (ValueError, KeyError, TypeError, AttributeError) as exc:
        print(f"run-gates: unreadable report: {exc}", file=sys.stderr)
        return 2
    if not failed:
        print("no failed gates")
        return 0
    for t in failed:
        print(f"== {t.get('name')}: {t.get('message', t.get('status'))}")
        if t.get("trace"):
            print(t["trace"])
    return 0


def main(argv: list) -> int:
    if argv == []:
        return run()
    if argv == ["--verdict"]:
        return verdict()
    if argv == ["--failures"]:
        return failures()
    print(USAGE, file=sys.stderr)
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
