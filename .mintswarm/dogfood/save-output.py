#!/usr/bin/env python3
"""Save the document a stage returned in its final reply.

Dogfooding stand-in for the runner's save step.

    save-output.py [--text] INPUT OUTPUT.md

By default INPUT is a Claude Code subagent transcript (JSONL); the reply is
its last assistant text, including a SubagentHandback tool input. With
--text, INPUT is the reply itself as plain text.

The reply must follow "Returning the output" in .mintswarm/stages.md:

1. The opening line is N backticks immediately followed by `markdown`, at
   the start of the line with no indentation; N is at least 4. Trailing
   whitespace is allowed, nothing else.
2. The closing line is exactly N backticks at the start of the line,
   optionally followed by whitespace.
3. N is greater than the longest run of backticks that starts any line of
   the document, ignoring leading spaces.
4. The runner saves the lines strictly between the opening and closing
   lines, unchanged, with a final newline. The first of them is the
   document's `# ` title.
5. The reply contains exactly one such block. Lines inside it are part of
   the document, never the start of another block.
6. After the closing line come only blank lines, then exactly one result
   block (a ```yaml fence whose first line is `msw-result:`), then only
   blank lines. A few lines of text may come before the document block.

Closing on exactly N backticks is stricter than CommonMark's "at least N";
a reply that follows rule 3 renders the same in CommonMark.

Writes the document to OUTPUT.md (creating its directory), then prints the
result block and the saved size. Exits 0 when saved, 1 when the reply does
not follow the rules (nothing is written), 2 on a usage error or when INPUT
cannot be read or OUTPUT cannot be written. Standard library only.
"""

import json
import re
import sys
from pathlib import Path

USAGE = "usage: save-output.py [--text] INPUT OUTPUT.md"
OPENER = re.compile(r"(`{4,})markdown[ \t]*")
RESULT_OPENER = re.compile(r"```yaml[ \t]*")
RESULT_CLOSER = re.compile(r"```[ \t]*")


class ReplyError(Exception):
    pass


def last_assistant_text(path: str):
    text = None
    with open(path, encoding="utf-8") as f:
        for line in f:
            try:
                rec = json.loads(line)
            except json.JSONDecodeError:
                continue
            if not isinstance(rec, dict):
                continue
            msg = rec.get("message")
            if not isinstance(msg, dict) or msg.get("role") != "assistant":
                continue
            content = msg.get("content")
            if isinstance(content, str):
                parts = [content]
            else:
                parts = []
                for c in content if isinstance(content, list) else []:
                    if not isinstance(c, dict):
                        continue
                    if c.get("type") == "text":
                        parts.append(c.get("text") or "")
                    elif c.get("type") == "tool_use" and c.get("name") == "SubagentHandback":
                        parts.append((c.get("input") or {}).get("message") or "")
            joined = "\n".join(p for p in parts if p)
            if joined.strip():
                text = joined
    return text


def leading_backticks(line: str) -> int:
    stripped = line.lstrip(" ")
    return len(stripped) - len(stripped.lstrip("`"))


def find_document(lines: list) -> tuple:
    """Return (first, last, fence): the document is lines[first:last]."""
    blocks = []
    i = 0
    while i < len(lines):
        m = OPENER.fullmatch(lines[i])
        if not m:
            i += 1
            continue
        fence = m.group(1)
        closer = re.compile(re.escape(fence) + r"[ \t]*")
        for j in range(i + 1, len(lines)):
            if closer.fullmatch(lines[j]):
                break
        else:
            raise ReplyError(f"line {i + 1}: document block opened with {len(fence)} backticks is never closed")
        blocks.append((i + 1, j, fence))
        i = j + 1
    if not blocks:
        raise ReplyError("no document block (a line of 4 or more backticks followed by 'markdown')")
    if len(blocks) > 1:
        starts = ", ".join(str(first) for first, _, _ in blocks)
        raise ReplyError(f"{len(blocks)} document blocks, opened on lines {starts}; expected exactly one")
    return blocks[0]


def check_document(lines: list, first: int, last: int, fence: str) -> None:
    body = lines[first:last]
    if not body or not body[0].startswith("# "):
        raise ReplyError(f"line {first + 1}: the document does not start with a '# ' title")
    for k, line in enumerate(body, start=first):
        if leading_backticks(line) >= len(fence):
            raise ReplyError(
                f"line {k + 1}: a run of {leading_backticks(line)} backticks inside a document"
                f" opened with {len(fence)}; open it with more backticks"
            )


def find_result(lines: list, after: int) -> str:
    i = after
    while i < len(lines) and not lines[i].strip():
        i += 1
    if i == len(lines):
        raise ReplyError("no msw-result block after the document block")
    if not (RESULT_OPENER.fullmatch(lines[i]) and i + 1 < len(lines) and lines[i + 1].startswith("msw-result:")):
        raise ReplyError(f"line {i + 1}: only blank lines may come between the document block and the msw-result block")
    for j in range(i + 2, len(lines)):
        if RESULT_CLOSER.fullmatch(lines[j]):
            break
    else:
        raise ReplyError(f"line {i + 1}: result block is never closed")
    for k in range(j + 1, len(lines)):
        if lines[k].strip():
            raise ReplyError(f"line {k + 1}: text after the msw-result block")
    return "\n".join(lines[i + 1:j])


def extract(text: str) -> tuple:
    lines = [line[:-1] if line.endswith("\r") else line for line in text.split("\n")]
    raw = text.split("\n")
    first, last, fence = find_document(lines)
    check_document(lines, first, last, fence)
    result = find_result(lines, last + 1)
    return "\n".join(raw[first:last]) + "\n", result


def main(argv: list) -> int:
    text_mode = bool(argv) and argv[0] == "--text"
    args = argv[1:] if text_mode else argv
    if len(args) != 2 or any(a.startswith("--") for a in args):
        print(USAGE, file=sys.stderr)
        return 2
    source, output = args
    try:
        if text_mode:
            with open(source, encoding="utf-8", newline="") as f:
                text = f.read()
        else:
            text = last_assistant_text(source)
    except (OSError, UnicodeDecodeError) as exc:
        print(f"save-output: cannot read {source}: {exc}", file=sys.stderr)
        return 2
    try:
        if not text or not text.strip():
            raise ReplyError("no reply text" if text_mode else "no assistant text in the transcript")
        document, result = extract(text)
    except ReplyError as exc:
        print(f"save-output: {exc}", file=sys.stderr)
        return 1
    path = Path(output)
    try:
        path.parent.mkdir(parents=True, exist_ok=True)
        with path.open("w", encoding="utf-8", newline="") as f:
            f.write(document)
    except OSError as exc:
        print(f"save-output: cannot write {output}: {exc}", file=sys.stderr)
        return 2
    print(result.rstrip())
    print(f"saved {len(document)} characters to {output}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
