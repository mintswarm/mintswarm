#!/usr/bin/env python3
"""Convert a CTRF report to JUnit XML: the built-in `junit` report converter.

    ctrf-to-junit.py CTRF_IN OUT_PATH

Follows the converter contract in .mintswarm/dogfood/report-gates.py: reads
the CTRF report CTRF_IN and writes only OUT_PATH. The report must have
`reportFormat` `CTRF` and a `results.tests` list whose entries have a `name`,
a `status` and an integer `duration`.

Writes one <testsuites> holding one <testsuite>, both named after
`results.tool.name`, and one <testcase> per test, with times in seconds:

    passed                         no child
    failed, rawStatus "timeout"    <error type="timeout" message=...>trace</error>
    failed                         <failure type="failed" message=...>trace</failure>
    other                          <error type="other" message=...>trace</error>
    skipped, pending               <skipped message=.../>

<system-out> holds the test's `stdout` lines, or else its `trace`, and is left
out when both are empty. Characters XML 1.0 does not allow (control characters
other than tab, LF and CR, surrogates, U+FFFE and U+FFFF) become U+FFFD, so
colour codes show as U+FFFD followed by `[31m`.

Exits 0 when written, 2 on unusable input or when OUT_PATH cannot be written.
Standard library only.
"""

import json
import re
import sys
import xml.etree.ElementTree as ET
from datetime import datetime, timezone

USAGE = "usage: ctrf-to-junit.py CTRF_IN OUT_PATH"
STATUSES = ("passed", "failed", "skipped", "pending", "other")
NOT_XML_CHAR = re.compile("[^\t\n\r\x20-\ud7ff\ue000-\ufffd\U00010000-\U0010ffff]")


class InputError(Exception):
    pass


def clean(value) -> str:
    return NOT_XML_CHAR.sub("\ufffd", str(value))


def seconds(ms) -> str:
    return f"{max(ms, 0) / 1000:.3f}"


def is_int(value) -> bool:
    return isinstance(value, int) and not isinstance(value, bool)


def load(path: str) -> dict:
    try:
        with open(path, encoding="utf-8") as f:
            report = json.load(f)
    except (OSError, ValueError) as exc:
        raise InputError(f"cannot read {path}: {exc}") from exc
    if not isinstance(report, dict) or report.get("reportFormat") != "CTRF":
        raise InputError(f"{path}: reportFormat is not CTRF")
    results = report.get("results")
    tests = results.get("tests") if isinstance(results, dict) else None
    if not isinstance(tests, list):
        raise InputError(f"{path}: no results.tests list")
    for index, test in enumerate(tests):
        if not isinstance(test, dict):
            raise InputError(f"{path}: results.tests[{index}] is not an object")
        if not isinstance(test.get("name"), str) or not test["name"]:
            raise InputError(f"{path}: results.tests[{index}] has no name")
        if test.get("status") not in STATUSES:
            raise InputError(f"{path}: results.tests[{index}] has no valid status")
        if not is_int(test.get("duration")):
            raise InputError(f"{path}: results.tests[{index}] has no integer duration")
    return report


def timestamp(report: dict, summary: dict) -> str:
    if isinstance(report.get("timestamp"), str) and report["timestamp"]:
        return report["timestamp"]
    if is_int(summary.get("start")):
        start = datetime.fromtimestamp(summary["start"] / 1000, timezone.utc)
        return start.isoformat(timespec="seconds")
    return ""


def total_ms(summary: dict, tests: list) -> int:
    if is_int(summary.get("duration")):
        return summary["duration"]
    if is_int(summary.get("start")) and is_int(summary.get("stop")):
        return summary["stop"] - summary["start"]
    return sum(t["duration"] for t in tests)


def testcase(parent: ET.Element, test: dict, tool: str) -> None:
    case = ET.SubElement(
        parent, "testcase", name=clean(test["name"]), classname=clean(tool), time=seconds(test["duration"])
    )
    status = test["status"]
    message = test.get("message")
    trace = test.get("trace") if isinstance(test.get("trace"), str) else ""
    if status == "failed" and test.get("rawStatus") == "timeout":
        child = ET.SubElement(case, "error", type="timeout")
    elif status == "failed":
        child = ET.SubElement(case, "failure", type="failed")
    elif status == "other":
        child = ET.SubElement(case, "error", type="other")
    elif status in ("skipped", "pending"):
        child = ET.SubElement(case, "skipped")
        trace = None
    else:
        child = None
    if child is not None:
        if isinstance(message, str):
            child.set("message", clean(message))
        if trace:
            child.text = clean(trace)
    stdout = test.get("stdout")
    if isinstance(stdout, list) and stdout:
        out = "\n".join(str(line) for line in stdout)
    else:
        out = test.get("trace") if isinstance(test.get("trace"), str) else ""
    if out:
        ET.SubElement(case, "system-out").text = clean(out)


def convert(report: dict) -> ET.ElementTree:
    results = report["results"]
    tests = results["tests"]
    tool_info = results.get("tool")
    tool = tool_info.get("name") if isinstance(tool_info, dict) else None
    tool = tool if isinstance(tool, str) and tool else "ctrf"
    summary = results.get("summary") if isinstance(results.get("summary"), dict) else {}

    failures = sum(1 for t in tests if t["status"] == "failed" and t.get("rawStatus") != "timeout")
    errors = sum(
        1 for t in tests if t["status"] == "other" or (t["status"] == "failed" and t.get("rawStatus") == "timeout")
    )
    skipped = sum(1 for t in tests if t["status"] in ("skipped", "pending"))
    time = seconds(total_ms(summary, tests))
    counts = {"tests": str(len(tests)), "failures": str(failures), "errors": str(errors)}

    root = ET.Element("testsuites", name=clean(tool), **counts, time=time)
    suite = ET.SubElement(root, "testsuite", name=clean(tool), **counts, skipped=str(skipped), time=time)
    stamp = timestamp(report, summary)
    if stamp:
        suite.set("timestamp", clean(stamp))
    for test in tests:
        testcase(suite, test, tool)
    tree = ET.ElementTree(root)
    ET.indent(tree)
    return tree


def main(argv: list) -> int:
    if len(argv) != 2:
        print(USAGE, file=sys.stderr)
        return 2
    source, output = argv
    try:
        tree = convert(load(source))
    except InputError as exc:
        print(f"ctrf-to-junit: {exc}", file=sys.stderr)
        return 2
    try:
        tree.write(output, encoding="UTF-8", xml_declaration=True)
        with open(output, "a", encoding="utf-8") as f:
            f.write("\n")
    except OSError as exc:
        print(f"ctrf-to-junit: cannot write {output}: {exc}", file=sys.stderr)
        return 2
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
