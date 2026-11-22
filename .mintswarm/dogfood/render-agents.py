#!/usr/bin/env python3
"""Render the Claude Code stage agents from the harness-neutral definitions.

Dogfooding stand-in for the runner's harness adapter. Reads every stage
definition in .mintswarm/stages/<stage>.md (README.md excepted):

    ---
    name: plan
    description: mintswarm plan stage. ...
    capabilities: [read, search, shell]
    tier: strong
    ---
    <instructions>

and writes .claude/agents/msw-<stage>.md, mapping capabilities to Claude tool
names and the tier to a Claude model. The body is copied byte for byte.
The frontmatter is a YAML subset: flat `key: value` lines with literal values,
and an inline `capabilities: [a, b]` list; blank lines and `#` comments are
ignored. `description` is written out as a double-quoted YAML scalar. The
vocabulary is documented in .mintswarm/stages/README.md.

The renderer owns .claude/agents/msw-*.md and nothing else in .claude/agents/.
Without arguments it writes every rendered file and removes msw-*.md files
that have no source. With --check it writes nothing and reports drift.

Exits 0 on success (or no drift), 1 on drift (--check only), 2 when the
sources are unusable. Standard library only.
"""

import difflib
import sys
import unicodedata
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
SOURCES = ROOT / ".mintswarm" / "stages"
TARGET = ROOT / ".claude" / "agents"
SCRIPT = ".mintswarm/dogfood/render-agents.py"

CAPABILITIES = {
    "read": ["Read"],
    "search": ["Grep", "Glob"],
    "shell": ["Bash"],
    "edit": ["Edit", "Write"],
}
TOOL_ORDER = ["Read", "Grep", "Glob", "Bash", "Edit", "Write"]
TIERS = {
    "strong": "opus",
    "standard": "sonnet",
    "light": "haiku",
}
KEYS = ("name", "description", "capabilities", "tier")


class SourceError(Exception):
    pass


def read_text(path: Path) -> str:
    with path.open(encoding="utf-8", newline="") as f:
        return f.read()


def write_text(path: Path, text: str) -> None:
    with path.open("w", encoding="utf-8", newline="") as f:
        f.write(text)


def rel(path: Path) -> str:
    return str(path.relative_to(ROOT))


def parse(path: Path) -> dict:
    where = rel(path)
    text = read_text(path)
    # Split on LF only: str.splitlines() also breaks at U+2028, U+0085 and
    # the like, which must be reported as characters, not as line ends.
    pieces = text.split("\n")
    if pieces[0].removesuffix("\r") != "---":
        raise SourceError(f"{where}: must start with a '---' line")
    fields = {}
    for index, piece in enumerate(pieces[1:], start=1):
        stripped = piece.removesuffix("\r")
        if stripped == "---":
            body = "\n".join(pieces[index + 1 :])
            break
        if not stripped.strip() or stripped.lstrip().startswith("#"):
            continue
        key, sep, value = stripped.partition(": ")
        if not sep:
            raise SourceError(f"{where}:{index + 1}: expected 'key: value'")
        if key not in KEYS:
            raise SourceError(f"{where}:{index + 1}: unknown key {key!r}")
        if key in fields:
            raise SourceError(f"{where}:{index + 1}: duplicate key {key!r}")
        if key == "capabilities" and value.startswith("[") and value.endswith("]"):
            items = [item.strip() for item in value[1:-1].split(",")]
            value = [item for item in items if item]
        fields[key] = value
    else:
        raise SourceError(f"{where}: frontmatter has no closing '---' line")

    missing = [key for key in KEYS if key not in fields]
    if missing:
        raise SourceError(f"{where}: missing keys: {', '.join(missing)}")
    if fields["name"] != path.stem:
        raise SourceError(f"{where}: name {fields['name']!r} does not match the file name")
    # YAML allows neither control characters (tab apart) nor U+FFFE, U+FFFF;
    # line and paragraph separators (U+2028, U+2029) would break the line.
    control = [
        c
        for c in fields["description"]
        if c != "\t" and (unicodedata.category(c) == "Cc" or c in "\ufffe\uffff\u2028\u2029")
    ]
    if control:
        raise SourceError(
            f"{where}: description contains characters that are not allowed: "
            + ", ".join(f"U+{ord(c):04X}" for c in control)
        )
    if not isinstance(fields["capabilities"], list):
        raise SourceError(f"{where}: capabilities must be a [ ] list")
    if not fields["capabilities"]:
        raise SourceError(f"{where}: capabilities must not be empty")
    unknown = [c for c in fields["capabilities"] if c not in CAPABILITIES]
    if unknown:
        raise SourceError(f"{where}: unknown capabilities: {', '.join(unknown)}")
    if fields["tier"] not in TIERS:
        raise SourceError(f"{where}: unknown tier {fields['tier']!r}")
    fields["body"] = body
    return fields


def quote(value: str) -> str:
    """Return value as a YAML double-quoted scalar."""
    return '"' + value.replace("\\", "\\\\").replace('"', '\\"') + '"'


def render(stage: dict) -> str:
    tools = {tool for cap in stage["capabilities"] for tool in CAPABILITIES[cap]}
    ordered = [tool for tool in TOOL_ORDER if tool in tools]
    name = stage["name"]
    return (
        "---\n"
        f"# Generated from .mintswarm/stages/{name}.md by {SCRIPT}; do not edit.\n"
        f"name: msw-{name}\n"
        f"description: {quote(stage['description'])}\n"
        f"tools: {', '.join(ordered)}\n"
        f"model: {TIERS[stage['tier']]}\n"
        "---\n"
        f"{stage['body']}"
    )


def load() -> dict:
    sources = sorted(p for p in SOURCES.glob("*.md") if p.name != "README.md")
    if not sources:
        raise SourceError(f"no stage definitions in {rel(SOURCES)}")
    return {TARGET / f"msw-{p.stem}.md": render(parse(p)) for p in sources}


def check(rendered: dict) -> int:
    drift = False
    for path, text in rendered.items():
        if not path.exists():
            print(f"missing: {rel(path)}")
            drift = True
            continue
        current = read_text(path)
        if current != text:
            print(f"differs: {rel(path)}")
            sys.stdout.writelines(
                difflib.unified_diff(
                    current.splitlines(keepends=True),
                    text.splitlines(keepends=True),
                    fromfile=f"{rel(path)} (on disk)",
                    tofile=f"{rel(path)} (rendered)",
                )
            )
            drift = True
    for path in sorted(TARGET.glob("msw-*.md")):
        if path not in rendered:
            print(f"orphan: {rel(path)}")
            drift = True
    if drift:
        print(f"run python3 {SCRIPT} to regenerate")
        return 1
    return 0


def write(rendered: dict) -> int:
    TARGET.mkdir(parents=True, exist_ok=True)
    for path, text in rendered.items():
        write_text(path, text)
        print(f"wrote {rel(path)}")
    for path in sorted(TARGET.glob("msw-*.md")):
        if path not in rendered:
            path.unlink()
            print(f"removed {rel(path)}")
    return 0


def main(argv: list) -> int:
    if argv not in ([], ["--check"]):
        print(f"usage: {SCRIPT} [--check]", file=sys.stderr)
        return 2
    try:
        rendered = load()
    except (OSError, SourceError) as exc:
        print(f"render-agents: {exc}", file=sys.stderr)
        return 2
    return check(rendered) if argv else write(rendered)


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
