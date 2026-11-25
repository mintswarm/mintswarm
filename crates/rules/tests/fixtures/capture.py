#!/usr/bin/env python3
"""Capture what the Python helpers give, as fixtures for the rules crate's Python-parity blocks.

Usage: capture.py OUT

Loads `.mintswarm/dogfood/plan-approval/floor.py`, `.mintswarm/dogfood/landing/collect-facts.py` and
`.mintswarm/dogfood/landing/facts-selftest.py` from their paths, without running their `main`, and
writes one JSON object per line to OUT:

- `dumps`: each line of `crates/domain/tests/fixtures/records.jsonl` re-dumped by `json.dumps` in
  three modes: `disk` (`indent=2` and a final newline, with `ensure_ascii` at the tool's own
  setting), `ascii` (`indent=2`, `ensure_ascii=True` and a final newline) and `canonical`
  (`sort_keys=True, ensure_ascii=False`, as `floor.py`'s `canonical`);
- `float`: number texts and Python's `json.dumps(float(text))`;
- `markdown`: a document with `floor.py`'s `headings`, `section` results and `issue_body` length, and
  `collect-facts.py`'s `scan_fences` result. The documents are the plan-approval fixtures
  `plan-*.md`, which the plan-approval and landing floor self-tests read, the landing facts
  self-test's `REVIEW_1` and `REVIEW_2`, and the edge documents in EDGES.

It reads `.mintswarm/dogfood/` and never modifies it. Run it with PYTHONDONTWRITEBYTECODE=1. Two runs
write the same bytes.

`tests/fixture.rs` runs it into a temporary file and fails when the committed fixture differs.
Refresh the fixture whenever a change touches one of those helpers, the record fixtures or the edge
documents:

    PYTHONDONTWRITEBYTECODE=1 python3 crates/rules/tests/fixtures/capture.py \
        crates/rules/tests/fixtures/python.jsonl
    cargo test -p mintswarm-rules
"""

import importlib.util
import json
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[4]
DOGFOOD = ROOT / ".mintswarm" / "dogfood"
RECORDS = ROOT / "crates" / "domain" / "tests" / "fixtures" / "records.jsonl"
PLANS = DOGFOOD / "plan-approval" / "fixtures"
# the tools that write their record with json.dumps's default ensure_ascii=True
ASCII_TOOLS = {"mintswarm-keep-round"}
WHAT = "the document"
ABSENT = "No such section"
ENDS = ((1, 2), (2,))
FLOATS = (
    "0.0",
    "-0.0",
    "0.1",
    "1.5",
    "100.0",
    "1234.5678",
    "0.30000000000000004",
    "1e15",
    "1e16",
    "123456789012345680000",
    "9007199254740993",
    "1e22",
    "1e+100",
    "1.7976931348623157e308",
    "0.0001",
    "1e-4",
    "0.000123",
    "1e-05",
    "2.5e-7",
    "123e-20",
    "5e-324",
    "-1.5e-10",
)
EDGES = (
    ("indented-four", "# T\n\n    ```\n## Inside\n    ```\n## After\nbody\n"),
    ("tilde", "# T\n~~~\n## Inside\n~~~\n## After\nx\n"),
    ("longer-close", "# T\n````\n## Inside\n```\n## Still\n``````\n## After\n"),
    ("trailing-text-close", "# T\n```\n## Inside\n``` x\n## Still\n```\n## After\n"),
    ("backtick-info", "# T\n```a`b\n## Outside\n```\n## Also\n"),
    ("info-string", "# T\n```yaml extra\nfindings: []\n```\n## After\n"),
    ("crlf", "# T\r\n\r\n## One\r\nbody\r\n```\r\n## In\r\n```\r\n## Two \r\nx\r\n"),
    ("heading-x1c", "# T\n## Title\x1c\nbody\n## Other\x1f \nx\n"),
    ("unclosed", "# T\n## Before\n```text\n## Inside\nx\n"),
    ("no-space", "#x\n##y\n# T\n## A\n###  Three\n## B\n"),
    ("repeated", "# T\n## Same\na\n## Same\nb\n# Top\nc\n"),
    ("comments", "# T\n## Body\nx\n## Comments\n## Later\ny\n## Comments\n"),
    ("tabs-and-mixed", "# T\n\t```\n## In\n  ~~~\n## Still\n\t```  \n## Out\n   ```\n## Fenced\n   ```\n"),
)


def load(name: str, path: Path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def dumps_lines() -> list:
    out = []
    for number, line in enumerate(RECORDS.read_text(encoding="utf-8").split("\n"), 1):
        if not line:
            continue
        record = json.loads(line)
        tool = record["tool"]
        out.append(
            {
                "kind": "dumps",
                "line": number,
                "tool": tool,
                "disk": json.dumps(record, indent=2, ensure_ascii=tool in ASCII_TOOLS) + "\n",
                "ascii": json.dumps(record, indent=2) + "\n",
                "canonical": json.dumps(record, sort_keys=True, ensure_ascii=False),
            }
        )
    return out


def float_lines() -> list:
    return [{"kind": "float", "text": text, "dumps": json.dumps(float(text))} for text in FLOATS]


def markdown_line(floor, facts, name: str, text: str) -> dict:
    lines = floor.document_lines(text)
    marks = floor.headings(lines)
    titles = []
    for i, level in marks:
        title = lines[i].rstrip()[3:]
        if level == 2 and title not in titles:
            titles.append(title)
    sections = []
    for title in titles + [ABSENT]:
        for ends in ENDS:
            entry = {"title": title, "ends": list(ends)}
            try:
                start, body = floor.section(lines, title, WHAT, ends)
                entry.update(start=start, length=len(body))
            except floor.DocumentError as exc:
                entry["error"] = str(exc)
            sections.append(entry)
    outside, blocks = facts.scan_fences(text.replace("\r\n", "\n").split("\n"))
    return {
        "kind": "markdown",
        "name": name,
        "text": text,
        "what": WHAT,
        "headings": [list(mark) for mark in marks],
        "sections": sections,
        "issue_body": len(floor.issue_body(text)),
        "fences": {
            "outside": [index for index, _ in outside],
            "blocks": [{"info": info, "content": content} for info, content in blocks],
        },
    }


def markdown_lines(floor, facts, selftest) -> list:
    documents = [(path.name, path.read_bytes().decode("utf-8")) for path in sorted(PLANS.glob("plan-*.md"))]
    documents += [("REVIEW_1", selftest.REVIEW_1), ("REVIEW_2", selftest.REVIEW_2)]
    documents += list(EDGES)
    return [markdown_line(floor, facts, name, text) for name, text in documents]


def main(argv: list) -> int:
    if len(argv) != 2:
        print("usage: capture.py OUT", file=sys.stderr)
        return 2
    floor = load("msw_capture_floor", DOGFOOD / "plan-approval" / "floor.py")
    facts = load("msw_capture_collect_facts", DOGFOOD / "landing" / "collect-facts.py")
    selftest = load("msw_capture_facts_selftest", DOGFOOD / "landing" / "facts-selftest.py")
    entries = dumps_lines() + float_lines() + markdown_lines(floor, facts, selftest)
    text = "".join(json.dumps(entry) + "\n" for entry in entries)
    Path(argv[1]).write_bytes(text.encode("utf-8"))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
