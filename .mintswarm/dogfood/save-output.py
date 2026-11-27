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

The result block's content follows "Result block" and "Questions" in
.mintswarm/stages.md, read in a strict subset of YAML:

- `msw-result:` then keys indented by two spaces; blank and comment lines
  may come between them. `status`, `artefact`, `outputs`, `questions` and
  `reason` are required, `commit` and `pr_title` optional (absent or null
  is none). An unknown, duplicate or missing key is a violation.
- Other values are one line: a plain scalar, or a double-quoted one with
  JSON escapes only, with an optional trailing `# comment`. Single quotes,
  block scalars, a plain scalar holding ': ' or starting with an
  indicator, and a plain scalar that YAML reads as null, a boolean or a
  number (for the string keys) are violations. `status` is complete,
  needs_input or failed.
- `outputs` is a one-line flow list of plain scalars.
- `questions` is a JSON array opened on its line. The lines up to the
  first one indented by fewer than four spaces are decoded; if the array
  does not close there, it is decoded again up to the next key, so that a
  line indented by fewer than four spaces or by a tab is reported as such.
  Blank lines inside the array and text after its closing `]` other than
  a comment are violations.
- Each question, option, recommendation and reference has the keys and
  types of crates/domain/src/question.rs, null meaning absent for the
  optional ones; unknown and duplicate keys and strings holding an
  unpaired surrogate are violations, and so is a question that is not an
  object (an old string question). Sound questions then get the checks
  of Question::violations, emptiness meaning Rust's trim over the
  White_Space property, and when all are sound the id and status rules of
  StageOutcome::check, in its order and wording.

Every content violation is listed in one message under rule "Result
block", which quotes the last sentence of "Questions".

Writes the document to OUTPUT.md (creating its directory) and the record
to OUTPUT.result.json (the `.md` suffix replaced, or `.result.json`
appended): the StageOutcome in serde_json's pretty form, keys in serde
order, every key present, null for an absent value and [] for an absent
list. Both go through temporary files in the target directory, so either
both are written or neither. Then prints the result block, which
assistant message the document came from (transcript mode only) and the
saved size. Exits 0 when saved, 1 when the reply does not follow the rules
(nothing is written), 2 on a usage error or when INPUT cannot be read or
OUTPUT cannot be written (nothing is left behind), or on an unexpected
error. Read and write failures are reported in fixed phrases, those of
`mintswarm-runner tool save-output`, and an unexpected error as
`save-output: internal error: unexpected <class>`; no text of an operating
system, decoder or other library error is printed. The phrases and the base
error class come from errors.py beside this script, loaded by path.
Standard library only.

On exit 1, stderr names the message checked and the fault, then the rule
it breaks, quoted verbatim from .mintswarm/stages.md. The dispatching
session then sends the agent one corrective message that quotes that rule
and re-runs this script on the new reply; it never repairs the output by
hand. If the corrected reply lands in another transcript file, pass that
file. If the reply still fails, the stage is `failed`, with this script's
message as the reason.
"""

import importlib.util
import json
import os
import re
import sys
import tempfile
import unicodedata
from pathlib import Path


def load_errors():
    """errors.py from this script's directory, loaded by path without writing bytecode."""
    sys.dont_write_bytecode = True  # leave no __pycache__ next to errors.py
    try:
        spec = importlib.util.spec_from_file_location("msw_dogfood_errors", Path(__file__).with_name("errors.py"))
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
    except Exception:
        print("save-output: cannot load errors.py", file=sys.stderr)
        sys.exit(2)
    return module


errors = load_errors()

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


# Verbatim from "Questions" in .mintswarm/stages.md: the rule quoted when the result block's
# content breaks the subset or the checks.
CONTENT_RULE = (
    "The result block's keys and values follow the subset above, and its questions pass every check in this"
    " section; save-output.py lists every violation at once and saves nothing until all are fixed."
)


class ReplyError(errors.DomainError):
    """The reply does not follow a rule of .mintswarm/stages.md; exit 1."""

    def __init__(self, fault: str, rule: str, quote: str = ""):
        super().__init__(fault)
        self.fault = fault
        self.rule = rule
        self.quote = quote or RULES[rule]

    def explain(self) -> str:
        return f"{self.fault}\nbreaks {self.rule} of {STAGES}: {self.quote}"


class InputError(errors.DomainError):
    """INPUT cannot be read; exit 2."""


class OutputError(errors.DomainError):
    """OUTPUT or its record cannot be written; exit 2."""


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


def find_result(lines: list, after: int) -> tuple:
    """Return (block, at): the result block's lines from `msw-result:` on, which is reply line at."""
    i = after
    while i < len(lines) and not lines[i].strip():
        i += 1
    if i == len(lines):
        raise ReplyError("no msw-result block after the document block", "rule 6")
    fence = RESULT_FENCE.fullmatch(lines[i])
    if not fence:
        raise ReplyError(
            f"line {i + 1}: text between the document block and the result block: {rust_debug(lines[i].strip())}",
            "rule 6",
        )
    if not RESULT_OPENER.fullmatch(lines[i]):
        tag = fence.group(1).strip()
        found = f"tagged {rust_debug(tag)}" if tag else "untagged"
        raise ReplyError(f"line {i + 1}: the result block's fence is {found}; it must be ```yaml", "Result block")
    if i + 1 == len(lines) or not lines[i + 1].startswith("msw-result:"):
        found = rust_debug(lines[i + 1]) if i + 1 < len(lines) else "nothing"
        raise ReplyError(
            f"line {i + 2}: the result block's first line is {found}; it must be 'msw-result:'", "Result block"
        )
    for j in range(i + 2, len(lines)):
        if RESULT_CLOSER.fullmatch(lines[j]):
            break
    else:
        raise ReplyError(f"line {i + 1}: result block is never closed", "Result block")
    return lines[i + 1 : j], i + 2


# The result block's content: a strict subset of YAML that yaml-rust2 reads the same way.

STATUSES = ("complete", "needs_input", "failed")
KINDS = ("must_decide", "recommended", "assumption")
SHAPES = ("one", "several", "yes_no", "open")
REFERENCE_KINDS = ("file", "artefact", "ticket", "design", "answer", "url")
REQUIRED_KEYS = ("status", "artefact", "outputs", "questions", "reason")
OPTIONAL_KEYS = ("commit", "pr_title")
KEY_LINE = re.compile(r"  ([A-Za-z_][A-Za-z0-9_-]*):(?:[ \t]+(.*)|[ \t]*)")
TAIL = re.compile(r"(?:[ \t]+#.*)?[ \t]*")
COMMENT = re.compile(r"[ \t]#")
INDICATORS = set("[]{},#&*!|>'\"%@`")
YAML_NULL = re.compile(r"~|null|Null|NULL")
YAML_BOOL = re.compile(r"true|True|TRUE|false|False|FALSE")
YAML_NUMBER = re.compile(
    r"[-+]?[0-9]+|0o[0-7]+|0x[0-9a-fA-F]+|[-+]?(?:\.[0-9]+|[0-9]+(?:\.[0-9]*)?)(?:[eE][-+]?[0-9]+)?"
    r"|[-+]?\.(?:inf|Inf|INF)|\.(?:nan|NaN|NAN)"
)
CONTINUATION = "    "

# The objects in `questions`, as (key, spec) pairs in serde order. A spec is "string" (required),
# "string?" (a string or null), ("enum", values), ("list", item spec) (null or absent is []),
# ("object", fields) (required) or ("object?", fields) (null or absent is None).
OPTION_FIELDS = (("id", "string"), ("label", "string"), ("consequence", "string"), ("trade_off", "string?"))
RECOMMENDATION_FIELDS = (("choice", ("list", "string")), ("answer", "string?"), ("reason", "string"))
REFERENCE_FIELDS = (("kind", ("enum", REFERENCE_KINDS)), ("target", "string"), ("lines", "string?"))
QUESTION_FIELDS = (
    ("id", "string"),
    ("kind", ("enum", KINDS)),
    ("question", "string"),
    ("why", "string"),
    ("shape", ("enum", SHAPES)),
    ("options", ("list", ("object", OPTION_FIELDS))),
    ("recommendation", ("object?", RECOMMENDATION_FIELDS)),
    ("references", ("list", ("object", REFERENCE_FIELDS))),
)


# Rust's White_Space property, which str::trim strips: Python's str.strip also strips U+001C-U+001F.
RUST_WHITE_SPACE = frozenset("\t\n\v\f\r \x85\xa0\u1680\u2028\u2029\u202f\u205f\u3000") | frozenset(
    chr(c) for c in range(0x2000, 0x200B)
)
SURROGATE = re.compile("[\ud800-\udfff]")


def rust_blank(value: str) -> bool:
    """Whether Rust's value.trim().is_empty() holds."""
    return all(c in RUST_WHITE_SPACE for c in value)


def unpaired(value) -> bool:
    """Whether a decoded string holds a lone surrogate, which serde_json rejects and UTF-8 cannot hold."""
    return isinstance(value, str) and SURROGATE.search(value) is not None


class JsonObject(dict):
    """A decoded JSON object that keeps the first value of a repeated key and lists the repeats."""

    def __init__(self, pairs: list):
        super().__init__()
        self.duplicates = []
        for key, value in pairs:
            if key not in self:
                self[key] = value
            elif key not in self.duplicates:
                self.duplicates.append(key)


def rust_debug(value: str, quote: str = '"') -> str:
    """A string as Rust's `{:?}` writes it, as the Rust check's messages quote ids.

    With quote="'", a single character as Rust's `{:?}` writes a char: in single quotes, with
    `'` escaped and `"` left as it is.

    An approximation: Rust's char::escape_debug escapes by its is_printable table and
    Grapheme_Extend, approximated here by general categories. The two agree for ASCII and the
    printable letters, digits, punctuation and symbols ids realistically hold, and may differ for
    unassigned code points, some format characters and spacing marks.
    """
    out = []
    for c in value:
        if c in quote + "\\":
            out.append("\\" + c)
        elif c in "\n\r\t\0":
            out.append({"\n": "\\n", "\r": "\\r", "\t": "\\t", "\0": "\\0"}[c])
        elif unicodedata.category(c) in ("Cc", "Cf", "Cs", "Co", "Cn", "Zl", "Zp", "Mn", "Me"):
            out.append(f"\\u{{{ord(c):x}}}")
        else:
            out.append(c)
    return quote + "".join(out) + quote


def json_type(value) -> str:
    if value is None:
        return "null"
    if isinstance(value, bool):
        return "a boolean"
    if isinstance(value, (int, float)):
        return "a number"
    if isinstance(value, str):
        return "a string"
    if isinstance(value, list):
        return "an array"
    return "an object"


def plain_problem(text: str) -> str:
    """Why text cannot be a plain scalar here, or '' when it can."""
    if not text:
        return ""
    if text[0] in INDICATORS or (text[0] in "-?:" and (len(text) == 1 or text[1] in " \t")):
        first = rust_debug(text[0], quote="'")
        return f"starts with {first}; quote it with double quotes"
    if ": " in text or text.endswith(":") or ":\t" in text:
        return "holds ': '; quote it with double quotes"
    return ""


def resolve_plain(text: str) -> tuple:
    """A plain scalar as YAML resolves it: ('null', None), ('string', text) or ('other', what)."""
    if not text or YAML_NULL.fullmatch(text):
        return "null", None
    if YAML_BOOL.fullmatch(text):
        return "other", "a boolean"
    if YAML_NUMBER.fullmatch(text):
        return "other", "a number"
    return "string", text


def read_scalar(raw: str) -> tuple:
    """Read a scalar value with an optional trailing comment.

    Returns ((kind, value), problem): kind is 'null', 'string' or 'other', and problem is '' when
    the value is in the subset.
    """
    raw = raw.strip(" \t")
    if raw.startswith('"'):
        i = 1
        while i < len(raw) and raw[i] != '"':
            i += 2 if raw[i] == "\\" else 1
        if i >= len(raw):
            return None, "a double-quoted value is not closed on its line"
        try:
            value = json.loads(raw[: i + 1])
        except json.JSONDecodeError:
            return None, "a double-quoted value may use JSON escapes only"
        if unpaired(value):
            return None, "holds an unpaired surrogate"
        if not TAIL.fullmatch(raw[i + 1 :]):
            return None, f"text after the closing quote: {rust_debug(raw[i + 1 :].strip())}"
        return ("string", value), ""
    if raw.startswith("'"):
        return None, "single-quoted values are not supported; use double quotes"
    if raw[:1] in ("|", ">"):
        return None, "block scalars are not supported; use one double-quoted line"
    cut = COMMENT.search(raw)
    text = (raw[: cut.start()] if cut else ("" if raw.startswith("#") else raw)).rstrip(" \t")
    problem = plain_problem(text) or ("holds an unpaired surrogate" if unpaired(text) else "")
    if problem:
        return None, problem
    return resolve_plain(text), ""


def read_outputs(raw: str) -> tuple:
    """Read a one-line flow list of plain scalars. Returns (paths, problem)."""
    raw = raw.strip(" \t")
    end = raw.find("]")
    if not raw.startswith("[") or end < 0:
        return None, "outputs must be a flow list on one line, such as [.mintswarm/job/outputs/plan.md]"
    if not TAIL.fullmatch(raw[end + 1 :]):
        return None, f"text after outputs' closing ]: {rust_debug(raw[end + 1 :].strip())}"
    inner = raw[1:end].strip(" \t")
    if not inner:
        return [], ""
    paths = []
    for item in inner.split(","):
        item = item.strip(" \t")
        problem = plain_problem(item) or ("holds ' #'" if COMMENT.search(item) else "")
        problem = problem or ("holds an unpaired surrogate" if unpaired(item) else "")
        kind, value = resolve_plain(item)
        if problem or kind != "string":
            reason = problem or (f"reads as {value}" if kind == "other" else "is empty or null")
            return None, f"outputs item {rust_debug(item)} is not a plain string: {reason}"
        paths.append(value)
    return paths, ""


def indent_problem(line: str) -> str:
    """Why a line inside the questions array breaks the four-space rule, or ''."""
    if not line.strip():
        return "a blank line inside the questions array"
    if line.startswith(CONTINUATION):
        return ""
    if "\t" in line[: len(line) - len(line.lstrip(" \t"))]:
        return "a continuation line of questions is indented with a tab; use four spaces"
    return "a continuation line of questions is indented by fewer than four spaces"


def decode_array(texts: list) -> tuple:
    """Decode the JSON array that starts the joined texts. Returns (text, value, end, error)."""
    text = "\n".join(texts)
    try:
        value, end = json.JSONDecoder(object_pairs_hook=JsonObject).raw_decode(text)
    except json.JSONDecodeError as exc:
        return text, None, None, exc
    return text, value, end, None


def read_questions(block: list, k: int, raw: str, first: int) -> tuple:
    """Read the JSON array opened on block[k], the `questions:` line, whose value is raw.

    Collects the lines up to the first one indented by fewer than four spaces. If the array does
    not close there, decodes it again up to the next top-level key, so that under-indented lines
    are reported as such. Returns (questions, next index, violations).
    """
    raw = raw.strip(" \t")
    if not raw.startswith("["):
        return None, k + 1, [f"line {first + k}: questions must be a JSON array opened on its line, such as []"]
    end = k + 1
    while end < len(block) and block[end].startswith(CONTINUATION):
        end += 1
    stop = end
    text, value, close, error = decode_array([raw] + block[k + 1 : end])
    if error is not None:
        stop = end
        while stop < len(block) and not KEY_LINE.fullmatch(block[stop]):
            stop += 1
        if stop > end:
            text, value, close, error = decode_array([raw] + block[k + 1 : stop])
    if error is not None:
        if not text[error.pos :].strip():
            return None, stop, [f"line {first + k}: the questions array is not closed by ]"]
        return (
            None,
            stop,
            [f"line {first + k}: the questions array is not valid JSON; write it as a JSON array of question objects"],
        )
    violations = []
    last = text.count("\n", 0, close - 1)
    for j in range(1, last + 1):
        problem = indent_problem(block[k + j])
        if problem:
            violations.append(f"line {first + k + j}: {problem}")
    tail = text[close:].split("\n")
    if not TAIL.fullmatch(tail[0]):
        violations.append(
            f"line {first + k + last}: text after the questions array's closing ]: {rust_debug(tail[0].strip())}"
        )
    for j, line in enumerate(tail[1:], start=last + 1):
        if line.strip() and not line.strip().startswith("#"):
            violations.append(
                f"line {first + k + j}: text after the questions array's closing ]: {rust_debug(line.strip())}"
            )
    return value, stop, violations


def check_value(value, present: bool, spec, label: str, path: str, out: list) -> tuple:
    """Check one JSON value against its spec. Returns (sound, value with defaults applied)."""
    if spec == "string":
        if not present:
            out.append(f"{label}: {path} is missing")
        elif not isinstance(value, str):
            out.append(f"{label}: {path} must be a string, not {json_type(value)}")
        elif unpaired(value):
            out.append(f"{label}: {path} holds an unpaired surrogate")
        else:
            return True, value
        return False, None
    if spec == "string?":
        if unpaired(value):
            out.append(f"{label}: {path} holds an unpaired surrogate")
        elif value is None or isinstance(value, str):
            return True, value
        else:
            out.append(f"{label}: {path} must be a string or null, not {json_type(value)}")
        return False, None
    what = spec[0]
    if what == "enum":
        if not present:
            out.append(f"{label}: {path} is missing")
        elif not isinstance(value, str) or value not in spec[1]:
            found = rust_debug(value) if isinstance(value, str) else json_type(value)
            out.append(f"{label}: {path} must be one of {', '.join(spec[1])}, not {found}")
        else:
            return True, value
        return False, None
    if what == "list":
        if value is None:
            return True, []
        if not isinstance(value, list):
            out.append(f"{label}: {path} must be an array or null, not {json_type(value)}")
            return False, None
        sound, items = True, []
        for i, item in enumerate(value):
            ok, item = check_value(item, True, spec[1], label, f"{path}[{i}]", out)
            sound = sound and ok
            items.append(item)
        return sound, items
    if value is None and (what == "object?" or not present):
        if what == "object?":
            return True, None
        out.append(f"{label}: {path} is missing")
        return False, None
    if not isinstance(value, dict):
        out.append(f"{label}: {path} must be an object, not {json_type(value)}")
        return False, None
    return check_fields(value, spec[1], label, path, out)


def check_fields(value: dict, fields: tuple, label: str, path: str, out: list) -> tuple:
    """Check an object's keys. Returns (sound, the object in serde order with defaults applied)."""
    sound, result = True, {}
    where = f"{path}: " if path else ""
    names = [name for name, _ in fields]
    for key in getattr(value, "duplicates", ()):
        out.append(f"{label}: {where}duplicate key {rust_debug(key)}")
        sound = False
    for key in value:
        if key not in names:
            out.append(f"{label}: {where}unknown key {rust_debug(key)}")
            sound = False
    for name, spec in fields:
        ok, item = check_value(value.get(name), name in value, spec, label, f"{path}.{name}" if path else name, out)
        sound = sound and ok
        result[name] = item
    return sound, result


def repeated(ids: list) -> list:
    """The ids that occur more than once, each once, in order of first use."""
    seen, out = [], []
    for i in ids:
        if i not in seen:
            seen.append(i)
        elif i not in out:
            out.append(i)
    return out


def question_violations(q: dict) -> list:
    """Mirror of Question::violations in crates/domain/src/question.rs: same order, same wording."""
    qid, kind, shape, options, rec = q["id"], q["kind"], q["shape"], q["options"], q["recommendation"]
    choice = rec["choice"] if rec else []
    if shape == "open":
        valid = []
    elif shape == "yes_no" and not options:
        valid = ["yes", "no"]
    else:
        valid = [o["id"] for o in options]
    out = []
    if kind != "must_decide" and rec is None:
        out.append(f"{qid}: a recommended or assumption question needs a recommendation")
    if kind == "assumption":
        if shape == "open":
            out.append(f"{qid}: an assumption cannot have shape open")
        elif all(v in choice for v in valid):
            out.append(f"{qid}: an assumption needs an option besides those it chooses")
    if rust_blank(q["question"]):
        out.append(f"{qid}: question is empty")
    if rust_blank(q["why"]):
        out.append(f"{qid}: why is empty")
    if shape in ("one", "several") and len(options) < 2:
        out.append(f"{qid}: shapes one and several need at least two options")
    elif shape == "yes_no" and options:
        if len(options) != 2 or {o["id"] for o in options} != {"yes", "no"}:
            out.append(f"{qid}: yes_no options must be exactly yes and no")
    elif shape == "open" and options:
        out.append(f"{qid}: an open question has no options")
    for option in repeated([o["id"] for o in options]):
        out.append(f"{qid}: option id {rust_debug(option)} is not unique")
    if rec is not None:
        count = len(choice)
        fits = {"one": count == 1, "yes_no": count == 1, "several": count >= 1, "open": count == 0}[shape]
        if not fits:
            out.append(f"{qid}: {count} choices do not fit shape {shape}")
        if shape != "open":
            for c in choice:
                if c not in valid:
                    out.append(f"{qid}: choice {rust_debug(c)} is not an option id")
        for c in repeated(choice):
            out.append(f"{qid}: choice {rust_debug(c)} is chosen more than once")
        if rec["answer"] is not None and shape != "open":
            out.append(f"{qid}: only an open question's recommendation has an answer")
    return out


def check_questions(questions: list, status) -> list:
    """Mirror of StageOutcome::check, after the structural checks serde would make and more.

    The semantic checks run on each structurally sound question; the id and status rules run only
    when every question is sound and, for the status rule, the status is valid. Replaces each
    question in place with its serde form.
    """
    out = []
    all_sound = True
    for i, q in enumerate(questions):
        if not isinstance(q, dict):
            out.append(f"questions[{i}] must be a question object, not {json_type(q)}")
            all_sound = False
            continue
        qid = q.get("id")
        label = qid if isinstance(qid, str) and not rust_blank(qid) and not unpaired(qid) else f"questions[{i}]"
        sound, normalised = check_fields(q, QUESTION_FIELDS, label, "", out)
        if sound:
            questions[i] = normalised
            out.extend(question_violations(normalised))
        all_sound = all_sound and sound
    if not all_sound:
        return out
    for qid in repeated([q["id"] for q in questions]):
        out.append(f"{qid}: the id is not unique")
    blocking = [q for q in questions if q["kind"] != "assumption"]
    if status == "needs_input" and not blocking:
        out.append("needs_input needs a must_decide or recommended question")
    elif status in ("complete", "failed"):
        out.extend(f"{q['id']}: a {status} outcome may hold assumptions only" for q in blocking)
    return out


def parse_result(block: list, first: int) -> tuple:
    """Parse and check the result block's lines, block[0] being `msw-result:` on reply line first.

    Returns (outcome, violations): outcome is the StageOutcome's serde form, with keys in serde
    order, and is meaningful only when violations is empty.
    """
    violations = []
    values = {}
    if not TAIL.fullmatch(block[0][len("msw-result:") :]):
        violations.append(f"line {first}: text after 'msw-result:'; its keys go on the lines below")
    k = 1
    while k < len(block):
        line = block[k]
        here = first + k
        if not line.strip() or line.lstrip(" \t").startswith("#"):
            k += 1
            continue
        m = KEY_LINE.fullmatch(line)
        if not m:
            violations.append(f"line {here}: expected a key indented by two spaces, found {rust_debug(line.strip())}")
            k += 1
            continue
        key, raw = m.group(1), m.group(2) or ""
        k += 1
        if key not in REQUIRED_KEYS + OPTIONAL_KEYS:
            violations.append(f"line {here}: unknown key {rust_debug(key)}")
            while k < len(block) and block[k].startswith("   "):
                k += 1
            continue
        if key in values:
            violations.append(f"line {here}: duplicate key {rust_debug(key)}")
        if key == "questions":
            value, k, found = read_questions(block, k - 1, raw, first)
            violations.extend(found)
        elif key == "outputs":
            value, problem = read_outputs(raw)
            if problem:
                violations.append(f"line {here}: {problem}")
        else:
            scalar, problem = read_scalar(raw)
            value = None
            if problem:
                violations.append(f"line {here}: {key}: {problem}")
            elif scalar[0] == "string":
                value = scalar[1]
            elif scalar[0] == "null" and key in OPTIONAL_KEYS:
                value = None
            else:
                what = "empty or null" if scalar[0] == "null" else f"reads as {scalar[1]}"
                violations.append(f'line {here}: {key} must be a string but {what}; quote it, for example ""')
            if key == "status" and value is not None and value not in STATUSES:
                violations.append(f"line {here}: status must be one of {', '.join(STATUSES)}, not {rust_debug(value)}")
        values.setdefault(key, value)
    for key in REQUIRED_KEYS:
        if key not in values:
            violations.append(f"missing key {rust_debug(key)}")
    questions = values.get("questions")
    if isinstance(questions, list):
        status = values.get("status")
        violations.extend(check_questions(questions, status if status in STATUSES else None))
    outcome = {key: values.get(key) for key in REQUIRED_KEYS + OPTIONAL_KEYS}
    return outcome, violations


def record_path(output: Path):
    """Where the result record goes: OUTPUT's `.md` suffix replaced, or `.result.json` appended.
    None when OUTPUT names no file, as `/`, `.` and `..` do."""
    if output.name in ("", ".."):
        return None
    stem = output.name[:-3] if output.name.endswith(".md") else output.name
    return output.with_name(stem + ".result.json")


def write_both(writes: list) -> None:
    """Write each (path, text), all or none: temporary files in the target directory, then
    os.replace. On any exception, removes what it wrote and raises."""
    temps, done = [], []
    try:
        for path, text in writes:
            path.parent.mkdir(parents=True, exist_ok=True)
            fd, temp = tempfile.mkstemp(prefix=f".{path.name}.", dir=path.parent)
            temps.append(temp)
            with os.fdopen(fd, "w", encoding="utf-8", newline="") as f:
                f.write(text)
        for (path, _), temp in zip(writes, temps):
            os.replace(temp, path)
            done.append(path)
    except BaseException:
        for path in temps + done:
            try:
                os.unlink(path)
            except OSError:
                pass
        raise


def extract(text: str) -> tuple:
    lines = [line[:-1] if line.endswith("\r") else line for line in text.split("\n")]
    raw = text.split("\n")
    first, last, fence = find_document(lines)
    check_document(lines, first, last, fence)
    block, at = find_result(lines, last + 1)
    outcome, violations = parse_result(block, at)
    if violations:
        count = f"{len(violations)} violation{'s' if len(violations) > 1 else ''}"
        listed = "\n".join(f"- {v}" for v in violations)
        raise ReplyError(f"the result block has {count}:\n{listed}", "Result block", CONTENT_RULE)
    return "\n".join(raw[first:last]) + "\n", "\n".join(block), outcome


def main(argv: list) -> int:
    """Run the tool; an exception it did not expect gives the fixed crash line and exit 2."""
    try:
        return run(argv)
    except Exception as exc:
        print(errors.crash_line("save-output", exc), file=sys.stderr)
        return 2


def read_reply(source: str, text_mode: bool) -> tuple:
    """The reply text, the message it came from and whether it is an unfenced document."""
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
        raise InputError(f"cannot read {source}: {errors.read_phrase(exc)}") from exc
    return text, where, unfenced


def run(argv: list) -> int:
    text_mode = bool(argv) and argv[0] == "--text"
    args = argv[1:] if text_mode else argv
    if len(args) != 2 or any(a.startswith("--") for a in args):
        print(USAGE, file=sys.stderr)
        return 2
    source, output = args
    where = None
    try:
        text, where, unfenced = read_reply(source, text_mode)
        if not text or not text.strip():
            raise ReplyError("no reply text" if text_mode else "no assistant text in the transcript", "rule 1")
        if unfenced:
            raise ReplyError(
                "no document block: the final message starts with a '# ' title but is not in a block"
                " opened by a line of 4 or more backticks followed by 'markdown'; no earlier block is used",
                "rule 1",
            )
        document, result, outcome = extract(text)
    except ReplyError as exc:
        prefix = f"{where}: " if where else ""
        print(f"save-output: {prefix}{exc.explain()}", file=sys.stderr)
        return 1
    except InputError as exc:
        print(f"save-output: {exc}", file=sys.stderr)
        return 2
    path = Path(output)
    record = json.dumps(outcome, indent=2, ensure_ascii=False) + "\n"
    try:
        saved = record_path(path)
        if saved is None:
            raise OutputError(f"cannot write {output}: it names no file")
        try:
            write_both([(path, document), (saved, record)])
        except (OSError, UnicodeEncodeError) as exc:
            raise OutputError(f"cannot write {output}: {errors.write_phrase(exc)}") from exc
    except OutputError as exc:
        print(f"save-output: {exc}", file=sys.stderr)
        return 2
    print(result.rstrip())
    if where:
        print(f"document from {where}")
    print(f"saved {len(document)} characters to {output}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
