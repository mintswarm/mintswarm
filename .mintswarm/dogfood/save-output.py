#!/usr/bin/env python3
"""Save the document a stage returned in its final reply.

Dogfooding stand-in for the runner's save step.

    save-output.py [--text] INPUT OUTPUT.md

By default INPUT is a Claude Code subagent transcript (JSONL). The script
reads its assistant messages:

- only records with `type: assistant` whose `message.role` is `assistant`
  count; `api-request-blob` and other replayed copies are skipped;
- records that share a `message.id` are one message, in order of first
  appearance; a record without an id is a message of its own;
- a message's text is its `text` parts and the `message` input of its
  SubagentHandback tool calls, joined in record order; thinking and other
  tool calls are ignored, and messages without text are not counted.

The reply is the last of these messages that has a document opening line
(rule 1). It is the last message instead when that message has no opening
line but its first non-blank line is a `# ` title (a revised document left
unfenced), or when no message has an opening line; its fault is then
reported. Only that message is checked, so a final message that has an
opening line, or that starts with a `# ` title, is never replaced by an
earlier draft. An earlier message is used only when every later one has
neither, for example a final hand-back that holds only a summary. With
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
5. The reply contains exactly one such block before the result block.
   Lines inside it are part of the document, never the start of another
   block.
6. After the closing line come only blank lines, then exactly one result
   block (a ```yaml fence whose first line is `msw-result:`). Text after
   the result block is ignored. A few lines of text may come before the
   document block.

Closing on exactly N backticks is stricter than CommonMark's "at least N";
a reply that follows rule 3 renders the same in CommonMark. Lines are split
at LF only; a trailing CR is ignored when matching and kept in the saved
document.

Writes the document to OUTPUT.md (creating its directory), then prints the
result block, which assistant message the document came from (transcript
mode only) and the saved size. Exits 0 when saved, 1 when the reply does
not follow the rules (nothing is written), 2 on a usage error or when INPUT
cannot be read or OUTPUT cannot be written. Standard library only.

On exit 1, stderr names the message checked and the fault, then the rule
it breaks, quoted verbatim from .mintswarm/stages.md. The dispatching
session then sends the agent one corrective message that quotes that rule
and re-runs this script on the new reply; it never repairs the output by
hand. If the corrected reply lands in another transcript file, pass that
file. If the reply still fails, the stage is `failed`, with this script's
message as the reason.
"""

import json
import re
import sys
from pathlib import Path

USAGE = "usage: save-output.py [--text] INPUT OUTPUT.md"
STAGES = ".mintswarm/stages.md"
OPENER = re.compile(r"(`{4,})markdown[ \t]*")
RESULT_FENCE = re.compile(r"```(?!`)(.*)")
RESULT_OPENER = re.compile(r"```yaml[ \t]*")
RESULT_CLOSER = re.compile(r"```[ \t]*")

# Verbatim from "Returning the output" and "Result block" in
# .mintswarm/stages.md; save-output-selftest.py checks that they match.
RULES = {
    "rule 1": (
        "The opening line is N backticks immediately followed by `markdown`, at the start of the line"
        " with no indentation; N is at least 4. Trailing whitespace is allowed, nothing else."
    ),
    "rule 2": "The closing line is exactly N backticks at the start of the line, optionally followed by whitespace.",
    "rule 3": (
        "N is greater than the longest run of backticks that starts any line of the document, ignoring"
        " leading spaces. That is 4 for most documents, and 5 when the document contains a four-backtick"
        " block, for example one quoting the decompose or review template."
    ),
    "rule 4": (
        "The runner saves the lines strictly between the opening and closing lines, unchanged, with a"
        " final newline. The first of them is the document's `# ` title."
    ),
    "rule 5": (
        "The reply contains exactly one such block before the result block. Lines inside it are part of"
        " the document, never the start of another block."
    ),
    "rule 6": (
        "After the closing line come only blank lines, then exactly one result block (below). Text after"
        " the result block is ignored. A few lines of text may come before the document block."
    ),
    "Result block": (
        "End your final reply with exactly one block of this form; anything after it is ignored:\n"
        "\n"
        "```yaml\n"
        "msw-result:"
    ),
}


class ReplyError(Exception):
    def __init__(self, fault: str, rule: str):
        super().__init__(fault)
        self.fault = fault
        self.rule = rule

    def explain(self) -> str:
        return f"{self.fault}\nbreaks {self.rule} of {STAGES}: {RULES[self.rule]}"


def message_text(content) -> list:
    if isinstance(content, str):
        return [content]
    parts = []
    for c in content if isinstance(content, list) else []:
        if not isinstance(c, dict):
            continue
        if c.get("type") == "text":
            parts.append(c.get("text") or "")
        elif c.get("type") == "tool_use" and c.get("name") == "SubagentHandback":
            parts.append((c.get("input") or {}).get("message") or "")
    return parts


def assistant_messages(path: str) -> list:
    """Return the transcript's assistant messages as (id, text), in order."""
    messages = []
    by_id = {}
    with open(path, encoding="utf-8") as f:
        for line in f:
            try:
                rec = json.loads(line)
            except json.JSONDecodeError:
                continue
            if not isinstance(rec, dict) or rec.get("type") != "assistant":
                continue
            msg = rec.get("message")
            if not isinstance(msg, dict) or msg.get("role") != "assistant":
                continue
            parts = [p for p in message_text(msg.get("content")) if p]
            mid = msg.get("id")
            if isinstance(mid, str) and mid:
                if mid not in by_id:
                    by_id[mid] = len(messages)
                    messages.append((mid, []))
                messages[by_id[mid]][1].extend(parts)
            else:
                messages.append((None, parts))
    joined = [(mid, "\n".join(parts)) for mid, parts in messages]
    return [(mid, text) for mid, text in joined if text.strip()]


def has_opener(text: str) -> bool:
    return any(OPENER.fullmatch(line.removesuffix("\r")) for line in text.split("\n"))


def unfenced_document(text: str) -> bool:
    """Whether text has no document opener but starts with a '# ' title."""
    if has_opener(text):
        return False
    for line in text.split("\n"):
        if line.strip():
            return line.startswith("# ")
    return False


def pick_reply(messages: list) -> int:
    """Index of the reply: the last message if it is an unfenced document,
    else the last message with a document opener, else the last message."""
    if unfenced_document(messages[-1][1]):
        return len(messages) - 1
    for k in range(len(messages) - 1, -1, -1):
        if has_opener(messages[k][1]):
            return k
    return len(messages) - 1


def describe(messages: list, k: int) -> str:
    mid = messages[k][0] or "no id"
    return f"assistant message {k + 1} of {len(messages)} ({mid})"


def leading_backticks(line: str) -> int:
    stripped = line.lstrip(" ")
    return len(stripped) - len(stripped.lstrip("`"))


def result_follows(lines: list, after: int) -> bool:
    """Whether lines[after:] hold only blank lines, then a three-backtick fence line.

    Any such fence is taken as the attempted result block; find_result()
    reports what is wrong with it.
    """
    i = after
    while i < len(lines) and not lines[i].strip():
        i += 1
    return i < len(lines) and bool(RESULT_FENCE.fullmatch(lines[i]))


def find_document(lines: list) -> tuple:
    """Return (first, last, fence): the document is lines[first:last].

    Scanning stops at the first closed document block that a result block
    (or an attempt at one) follows, so text after it is never read.
    """
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
            raise ReplyError(
                f"line {i + 1}: document block opened with {len(fence)} backticks is never closed", "rule 2"
            )
        blocks.append((i + 1, j, fence))
        if result_follows(lines, j + 1):
            break
        i = j + 1
    if not blocks:
        raise ReplyError("no document block (a line of 4 or more backticks followed by 'markdown')", "rule 1")
    if len(blocks) > 1:
        starts = ", ".join(str(first) for first, _, _ in blocks)
        raise ReplyError(f"{len(blocks)} document blocks, opened on lines {starts}; expected exactly one", "rule 5")
    return blocks[0]


def check_document(lines: list, first: int, last: int, fence: str) -> None:
    body = lines[first:last]
    if not body or not body[0].startswith("# "):
        raise ReplyError(f"line {first + 1}: the document does not start with a '# ' title", "rule 4")
    for k, line in enumerate(body, start=first):
        if leading_backticks(line) >= len(fence):
            raise ReplyError(
                f"line {k + 1}: a run of {leading_backticks(line)} backticks inside a document"
                f" opened with {len(fence)}; open it with more backticks",
                "rule 3",
            )


def find_result(lines: list, after: int) -> str:
    i = after
    while i < len(lines) and not lines[i].strip():
        i += 1
    if i == len(lines):
        raise ReplyError("no msw-result block after the document block", "rule 6")
    fence = RESULT_FENCE.fullmatch(lines[i])
    if not fence:
        raise ReplyError(
            f"line {i + 1}: text between the document block and the result block: {lines[i].strip()!r}", "rule 6"
        )
    if not RESULT_OPENER.fullmatch(lines[i]):
        tag = fence.group(1).strip()
        found = f"tagged {tag!r}" if tag else "untagged"
        raise ReplyError(f"line {i + 1}: the result block's fence is {found}; it must be ```yaml", "Result block")
    if i + 1 == len(lines) or not lines[i + 1].startswith("msw-result:"):
        found = repr(lines[i + 1]) if i + 1 < len(lines) else "nothing"
        raise ReplyError(
            f"line {i + 2}: the result block's first line is {found}; it must be 'msw-result:'", "Result block"
        )
    for j in range(i + 2, len(lines)):
        if RESULT_CLOSER.fullmatch(lines[j]):
            break
    else:
        raise ReplyError(f"line {i + 1}: result block is never closed", "Result block")
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
    where = None
    unfenced = False
    try:
        if text_mode:
            with open(source, encoding="utf-8", newline="") as f:
                text = f.read()
        else:
            messages = assistant_messages(source)
            text = None
            if messages:
                k = pick_reply(messages)
                text = messages[k][1]
                where = describe(messages, k)
                unfenced = unfenced_document(text)
    except (OSError, UnicodeDecodeError) as exc:
        print(f"save-output: cannot read {source}: {exc}", file=sys.stderr)
        return 2
    try:
        if not text or not text.strip():
            raise ReplyError("no reply text" if text_mode else "no assistant text in the transcript", "rule 1")
        if unfenced:
            raise ReplyError(
                "no document block: the final message starts with a '# ' title but is not in a block"
                " opened by a line of 4 or more backticks followed by 'markdown'; no earlier block is used",
                "rule 1",
            )
        document, result = extract(text)
    except ReplyError as exc:
        prefix = f"{where}: " if where else ""
        print(f"save-output: {prefix}{exc.explain()}", file=sys.stderr)
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
    if where:
        print(f"document from {where}")
    print(f"saved {len(document)} characters to {output}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
