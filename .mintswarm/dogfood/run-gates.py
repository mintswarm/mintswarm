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
one test, `passed` or `failed`. Every test has `stdout`, the last 200 lines
of the gate's combined stdout and stderr (left out when empty), and the
command, exit code (null on timeout), timeout flag and timeout in
`extra["mintswarm.gate"]`. A failed gate also carries `message` (`exit code N`
or `timed out after N s`) and `trace` (the same tail as one string); a
timeout also has `rawStatus` `timeout`. Each gate runs in its own session,
and on timeout its whole process group is killed. One line per gate is
printed. Then, as the runner's own check of the tree, the tracked files that
differ from HEAD (`git status --porcelain --untracked-files=no`) are printed
after a `modified tracked files:` line, one path per line, when there are
any; a failure to list them is printed as `modified tracked files: unknown
(<reason>)`. Exits 0 when every gate passed, 1 when any failed or timed out,
2 when the input is unusable, whatever the tree holds.

--verdict exits 0 only when the report exists and parses, is CTRF, has no
`results.errors`, is no older than inputs/gates.json, counts as many tests as
the input has gates (at least one) and as `results.tests` holds, names the
input's gates in order, and every test passed with none failed or other, in
the summary and in each test. Otherwise it prints the first unmet condition
and exits 1. It exits 2 when the report exists but the input is unusable.
The verdict is this, never a summary written by an agent. The runner deletes
the report before each gates stage (see report-gates.py --clean), so a stage
that never ran the gates leaves no report and fails.

--failures prints, for each failed gate, `== <name>: <message>` and then its
trace verbatim, or `no failed gates`. Exits 0 when printed, 2 when the report
is missing or unreadable.

Other report formats are written by converters that the runner runs
afterwards; see report-gates.py. Standard library only.
"""

import json
import os
import signal
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


def tail(text: str) -> list:
    return text.splitlines()[-TAIL_LINES:]


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


def kill_group(proc: subprocess.Popen) -> None:
    try:
        os.killpg(proc.pid, signal.SIGKILL)
    except (ProcessLookupError, PermissionError):
        pass


def run_in_session(argv: list, timeout, **kwargs) -> tuple:
    """Run argv in a new session; return (returncode, stdout, timed_out).

    On timeout the whole process group is killed, not only the direct child.
    """
    proc = subprocess.Popen(argv, start_new_session=True, **kwargs)
    try:
        out, _ = proc.communicate(timeout=timeout)
        return proc.returncode, out, False
    except subprocess.TimeoutExpired:
        kill_group(proc)
        try:
            out, _ = proc.communicate(timeout=5)
        except subprocess.TimeoutExpired as exc:
            out = exc.stdout
            proc.kill()
            proc.wait()
        return None, out, True
    except BaseException:
        kill_group(proc)
        proc.wait()
        raise


def run_gate(gate: dict) -> dict:
    name, command = gate["name"], gate["command"]
    timeout = gate.get("timeout", DEFAULT_TIMEOUT_S)
    start = now_ms()
    started = time.monotonic()
    exit_code, raw, timed_out = run_in_session(
        ["bash", "-c", command], timeout, stdout=subprocess.PIPE, stderr=subprocess.STDOUT
    )
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
    lines = tail(output)
    if lines:
        test["stdout"] = lines
    if test["status"] == "failed":
        test["message"] = f"timed out after {seconds(timeout)} s" if timed_out else f"exit code {exit_code}"
        if lines:
            test["trace"] = "\n".join(lines)
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
    report_modified_tracked_files()
    return 0 if passed == len(tests) else 1


def modified_tracked_files() -> list:
    """The tracked files that differ from HEAD; raises OSError or ValueError when git cannot list them."""
    proc = subprocess.run(
        ["git", "status", "--porcelain", "--untracked-files=no", "-z"],
        stdin=subprocess.DEVNULL,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    if proc.returncode != 0:
        raise ValueError(f"git status exited {proc.returncode}")
    entries = proc.stdout.split(b"\0")
    paths = []
    at = 0
    while at < len(entries):
        entry = entries[at]
        at += 1
        if len(entry) < 4:
            continue
        paths.append(entry[3:].decode("utf-8", errors="replace"))
        if entry[:1] in (b"R", b"C"):
            at += 1  # -z gives a rename's or copy's original path as the next entry
    return paths


def report_modified_tracked_files() -> None:
    try:
        paths = modified_tracked_files()
    except (OSError, ValueError) as exc:
        print(f"modified tracked files: unknown ({exc})")
        return
    if paths:
        print("modified tracked files:")
        for path in paths:
            print(path)


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
    try:
        stale = REPORT.stat().st_mtime_ns < INPUT.stat().st_mtime_ns
    except OSError as exc:
        raise InputError(f"cannot stat {INPUT}: {exc}") from exc
    if stale:
        return f"stale report: {REPORT} is older than {INPUT}"
    if counts["tests"] != len(gates) or counts["tests"] <= 0:
        return f"summary.tests is {counts['tests']}, the input has {len(gates)} gates"
    if counts["tests"] != len(tests):
        return f"summary.tests is {counts['tests']}, results.tests holds {len(tests)}"
    names = [t.get("name") if isinstance(t, dict) else None for t in tests]
    expected = [g["name"] for g in gates]
    if names != expected:
        return f"results.tests names {names}, the input has {expected}"
    if counts["passed"] != counts["tests"]:
        return f"summary.passed is {counts['passed']} of {counts['tests']}"
    if counts["failed"] != 0 or counts["other"] != 0:
        return f"summary.failed is {counts['failed']}, summary.other is {counts['other']}"
    for test in tests:
        if test.get("status") != "passed":
            return f"test {test['name']} has status {test.get('status')!r}"
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
