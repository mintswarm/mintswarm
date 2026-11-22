#!/usr/bin/env python3
"""Check the guideline packs against defaults/packs/README.md, and render one.

    pack.py check [--root DIR] [--base REF]
    pack.py render PACK [--root DIR] [name=value ...]

Dogfooding stand-in for the pack validation and rendering that the setup session and the readiness
check rely on: it enforces every rule of defaults/packs/README.md that a machine can check, and renders
a pack by the README's single-pass rule. Other tools may load this file by path and call load_pack,
check_pack and render_pack; importing it has no side effects.

Inputs:

- DIR, the pack root, default `defaults/packs` relative to the working directory. Every directory in
  it is a pack, named by its id; other entries are ignored, except a symlink to a directory, which is
  reported. Files are seen by walking the filesystem with lstat, so untracked files count.
- pack.yaml is read with mikefarah yq: `yq -p=yaml -o=json .` for the values, loaded as JSON, and one
  query for each node's tag, style, kind, anchor, line, column and raw text, each value's path and
  each mapping's keys. A raw-text scan covers what yq hides: document markers, `<<` keys, quoted
  strings that leave their line and double-quoted escapes. yq numbers the first document's lines as if
  its leading blank, comment, directive and marker lines were absent, so they are added back; a quoted
  scalar whose line does not hold its quote at yq's column is a yaml-parse violation. Before any pack
  is read, a probe runs yq on a fixed text and checks that it answers each of these constructs as
  expected.
- REF, given to check: the version rule compares each pack with `git merge-base REF HEAD`. A pack is
  changed when `git diff --name-only --no-renames` against that commit or `git ls-files --others
  --exclude-standard` lists anything in its directory. A changed pack whose pack.yaml exists at the
  merge base has its version plus one; any other changed pack is new and has version 1.
- render: PACK, the id of one pack under DIR, and typed values: an integer fully matches
  `0|-?[1-9][0-9]*` within +-2147483647, a boolean is `true` or `false`, a string is taken verbatim.
  Parameters not given take their defaults. A derived pack is rendered alone, without its base.

Rules, in README order:

  layout-files               pack.yaml or conventions.md missing, or an extra file or directory, a
                             symlink, a non-regular file or an empty directory under settings/
  layout-settings-dir        settings/ present without settings entries, or missing with them
  layout-unreferenced        a file under settings/ that no entry's template names
  encoding                   a file that is not UTF-8, starts with a BOM or holds a CR byte
  conventions-title          the first line of conventions.md does not start with `# `
  yaml-parse                 yq cannot read pack.yaml, its top level is not a mapping, or yq places a
                             quoted string where the text has no quote
  yaml-duplicate-key         a key repeated in one mapping
  yaml-marker                a `---` or `...` line, or more than one document
  yaml-anchor-alias          an anchor or an alias
  yaml-merge-key             a `<<` key
  yaml-tag                   an explicit tag
  yaml-unquoted-string       a string value that is not quoted, a timestamp
  yaml-block-scalar          a literal or folded block scalar
  yaml-multiline-string      a quoted string that does not end on its line
  yaml-escape                a double-quoted escape other than `\\\\` and `\\"`
  yaml-integer               an integer that is not plain decimal, or outside +-2147483647
  yaml-boolean               a boolean written other than `true` or `false`
  yaml-float-null            a float or a null
  field-unknown              a key not in the README's tables
  field-missing              a required field left out
  field-type                 a value of the wrong type (`type(v) is int`, `type(v) is bool`)
  field-enum                 a parameter `type`, settings `kind` or `mode`, or gate `kind` outside
                             its values
  id                         `id` does not fully match `[a-z][a-z0-9-]*` or differs from the
                             directory name
  version-min                `version` is below 1
  detect-empty               `detect` lists no entry
  param-name                 a parameter name not fully matching `[a-z][a-z0-9_]*`
  param-unique               a parameter name repeated
  param-default              a `default` not of the parameter's type
  param-choices              `choices` on a boolean, empty, with a value not of the type, a repeat,
                             or without `default`; at render, a value outside `choices`
  param-pattern              `pattern` on a parameter that is not a string, or not a valid Python
                             regular expression
  value-string               a string value with a character of category Cc or Cf, U+2028 or
                             U+2029, not fully matching `pattern`, or, for a parameter in a target,
                             alternative or command, not fully matching `[A-Za-z0-9._/+-]+` or
                             starting with `-`
  section                    `section` missing with `fragment`, or given with `file`
  path-placeholder-segment   a placeholder in `target` or `alternatives` that is not a whole
                             segment followed by `/`
  path-target                the rendered target, with `.` segments dropped, is empty, absolute,
                             has an empty, `..` or `.git` segment (any case), a `\\`, `*`, `?`, `[`
                             or `]`
  path-alternatives          a rendered alternative breaks the same rules, glob characters apart
  path-template              `template` breaks them, has a `.` segment, does not start with
                             `settings/` or names no file
  fragment-header            a rendered fragment that does not start with a header line, a header
                             not named `<section>` or `<section>.<rest>`, a repeated header or a
                             `[[...]]` header
  settings-unique            a rendered (target, section) pair repeated, or a `file` entry sharing
                             its target; for a derived pack over the base's entries and its own
  gate-name                  a gate name not fully matching `<id>-[a-z0-9][a-z0-9-]*`
  gate-unique                a gate name repeated
  gate-timeout               `timeout` below 1
  gate-command               a rendered command that shlex.split refuses or splits into no word
  placeholder-undeclared     a placeholder naming no parameter of the pack
  placeholder-unused         a declared parameter in no placeholder
  placeholder-not-allowed    a placeholder in pack.yaml outside a target, an alternative or a command
  base                       `base` names the pack itself, no pack, or a pack with a base of its own
  version-bump               with --base: a changed pack whose version is not the merge base's plus
                             one, or a new pack not at 1

Path, fragment, uniqueness and command rules are checked rendered with defaults; render checks them
again, and the value rules, with the values it is given. The combined settings of a derived pack are
checked only when its base has no violations of its own.

Not checked (review rules): that a string value is placed only where every allowed value is safe, and
that a string parameter inside a quoted settings string has a `pattern` excluding the quote, `\\`, `$`
and the backtick; that a derived pack does not repeat its base's conventions and is adopted with its
base; that a base template takes in the derived file; that a fragment's target is a TOML,
EditorConfig or INI-style file; that PyYAML loads the file the same way, so a construct the two
loaders read differently beyond those the yaml-* rules forbid passes.

Limits: render checks the uniqueness of a derived pack's own settings only; the combined base and
derived list with the values chosen at adoption is the setup session's to check. yq and git are found
on PATH, and the probe catches only a yq that answers its fixed text wrongly, not one that lies about
pack.yaml alone. A parameter `pattern` runs with no time bound, so a catastrophic pattern stalls check
until the gate's timeout, which fails closed.

Output: check prints one line per violation, `<pack id>: <rule>: <detail>`, the detail naming the
file and, for pack.yaml, `line:column` where yq gives one; without --base it then prints `check:
version-bump not evaluated (no --base)`, and last `check: <P> packs, <V> violations`. render prints
one JSON object: `pack`, `version`, `values` (every parameter, in order), `conventions`, `settings`
(each `template`, `target`, `kind`, `mode`, `section`, null for `file`, `optional`, `alternatives`
and `text`, rendered, with `.` segments dropped from paths) and `gates` (each `name`, `command` and
`timeout` if given). It prints nothing to stdout unless it renders, and its violations go to stderr.

Exits 0 when every pack complies (a root with no pack directories does) or the pack renders; 1 on any
violation, including a given value that breaks a value, path, fragment or uniqueness rule; 2 on
unusable input: unknown or repeated arguments, a missing root, a missing yq or git, a failed probe,
an unresolvable REF, a root outside a git work tree with --base, a merge-base pack.yaml whose version
cannot be read, an unknown PACK, an argument without `=`, a repeated or undeclared name or an
ill-typed value, or any unexpected error. Standard library only, apart from running yq and git.
"""

import json
import os
import re
import shlex
import stat
import subprocess
import sys
import unicodedata
from pathlib import Path

DEFAULT_ROOT = "defaults/packs"
USAGE = "usage: pack.py check [--root DIR] [--base REF] | pack.py render PACK [--root DIR] [name=value ...]"
PLACEHOLDER = re.compile(r"\{\{ *([a-z][a-z0-9_]*) *\}\}")
HEADER_LINE = re.compile(r"\[ *([^\[\]#]+?) *\] *(#.*)?")
DOUBLE_HEADER = re.compile(r"\[\[ *([^\[\]#]+?) *\]\] *(#.*)?")
PACK_ID = re.compile(r"[a-z][a-z0-9-]*")
PARAM_NAME = re.compile(r"[a-z][a-z0-9_]*")
INTEGER = re.compile(r"0|-?[1-9][0-9]*")
INT_LIMIT = 2147483647
PATH_VALUE = re.compile(r"[A-Za-z0-9._/+-]+")
LINE_BREAK = re.compile("\r\n|[\n\r\x85\u2028\u2029]")  # what yq counts as a line break
MARKER = re.compile(r"(---|\.\.\.)([ \t].*)?")
MERGE = re.compile(r"(?:^|[{,\[])\s*(?:-\s+)*<<\s*:")
GLOB_CHARS = set("*?[]")
BOM = b"\xef\xbb\xbf"


def fields(spec: str) -> tuple:
    """A field table, (name, kind, required), from `name:kind` words; `!` marks a required field."""
    return tuple((w.split(":")[0], w.split(":")[1].rstrip("!"), w.endswith("!")) for w in spec.split())


TOP_FIELDS = fields(
    "id:string! version:integer! name:string! description:string base:string detect:mapping!"
    " parameters:entries settings:entries gates:entries"
)
DETECT_FIELDS = fields("files:strings shebangs:strings")
PARAMETER_FIELDS = fields("name:string! type:string! default:value! description:string! choices:list pattern:string")
SETTING_FIELDS = fields(
    "template:string! target:string! kind:string! mode:string section:string optional:boolean alternatives:strings"
)
GATE_FIELDS = fields("name:string! command:string! kind:string! timeout:integer optional:boolean")
KIND_NAMES = {"integer": "an integer", "entries": "a list of mappings", "strings": "a list of strings"}
PARAMETER_TYPES = ("string", "integer", "boolean")
SETTING_KINDS = ("formatter", "linter", "compiler", "toolchain", "editor")
SETTING_MODES = ("file", "fragment")
GATE_KINDS = ("format", "lint", "typecheck", "test")

META_QUERY = (
    '{"nodes": [... | [tag, style, kind, anchor, line, column, (select(kind == "scalar") | tostring)]],'
    ' "paths": [.. | [path, line, column]],'
    ' "keys": [.. | select(kind == "map") | [keys | .[] | [line, column, tostring]]]}'
)
PROBE_META = '# probe\n\na: 010\nb: True\nc: !!str "a"\nd: !!int 3\ne: &x "v"\nf: *x\ng: {<<: {z: 1}}\n'
PROBE_VALUES = "d: 1\nd: 2\n---\ne: 3\n"


class InputError(Exception):
    pass


# --- processes and JSON ----------------------------------------------------


def run(argv: list, data: bytes = None, cwd=None) -> subprocess.CompletedProcess:
    try:
        return subprocess.run(argv, input=data, capture_output=True, cwd=cwd)
    except OSError as exc:
        raise InputError(f"cannot run {argv[0]}: {exc}") from exc


def last_line(raw: bytes) -> str:
    lines = raw.decode("utf-8", errors="replace").strip().splitlines()
    return lines[-1] if lines else "no message"


def yq(query: str, data: bytes) -> subprocess.CompletedProcess:
    return run(["yq", "-p=yaml", "-o=json", "-I0", query, "-"], data)


def _no_duplicates(pairs: list) -> dict:
    result = {}
    for key, value in pairs:
        if key in result:
            raise ValueError(f"duplicate key {key!r}")
        result[key] = value
    return result


def _no_constant(name: str):
    raise ValueError(f"{name} is not allowed")


def strict_json(text: str):
    return json.loads(text, object_pairs_hook=_no_duplicates, parse_constant=_no_constant)


def json_stream(text: str, hook=_no_duplicates) -> list:
    """Every JSON value in text, in order; yq prints one per document."""
    decoder = json.JSONDecoder(object_pairs_hook=hook, parse_constant=_no_constant)
    values, i = [], 0
    while True:
        while i < len(text) and text[i] in " \t\r\n":
            i += 1
        if i >= len(text):
            return values
        value, i = decoder.raw_decode(text, i)
        values.append(value)


def probe() -> None:
    """Fail closed when yq answers a construct this script relies on differently."""
    meta = yq(META_QUERY, PROBE_META.encode())
    values = yq(".", PROBE_VALUES.encode())
    if meta.returncode != 0 or values.returncode != 0:
        raise InputError(f"yq probe failed: {last_line(meta.stderr if meta.returncode else values.stderr)}")
    try:
        docs = json_stream(meta.stdout.decode("utf-8"), dict)
        pairs = json_stream(values.stdout.decode("utf-8"), lambda items: [list(i) for i in items])
        nodes = {(n[4], n[5]): n for n in docs[0]["nodes"] if n[2] in ("scalar", "alias")}
        keys = docs[0]["keys"][0]
        expected = [
            ("leading comment and blank lines are not counted", docs[0]["nodes"][0][4:6] == [1, 1]),
            ("010 is a plain !!int read as 010", nodes[(1, 4)][0:2] + nodes[(1, 4)][6:] == ["!!int", "", "010"]),
            ("True is a !!bool", nodes[(2, 4)][0] == "!!bool"),
            ('!!str "a" is not double-quoted', nodes[(3, 4)][0] == "!!str" and nodes[(3, 4)][1] != "double"),
            ("!!int 3 is tagged", nodes[(4, 4)][1] == "tagged"),
            ("an anchor is named", nodes[(5, 4)][3] == "x"),
            ("an alias has kind alias", nodes[(6, 4)][2] == "alias"),
            ("<< has tag !!merge", nodes[(7, 5)][0] == "!!merge"),
            ("keys carry line and column", keys[0] == [1, 1, "a"] and keys[-1] == [7, 1, "g"]),
            ("values carry their path", [["a"], 1, 4] in docs[0]["paths"]),
            ("a duplicate key stays in the JSON", pairs[0] == [["d", 1], ["d", 2]]),
            ("two documents give two JSON values", len(pairs) == 2),
        ]
    except (ValueError, KeyError, IndexError, TypeError, UnicodeDecodeError) as exc:
        raise InputError(f"yq probe failed: unexpected output: {type(exc).__name__}: {exc}") from exc
    for name, ok in expected:
        if not ok:
            raise InputError(f"yq probe failed: {name}")


# --- reading pack.yaml -----------------------------------------------------


def read_yaml(text: str, name: str = "pack.yaml") -> tuple:
    """Return (value, violations, where) for the text of a pack.yaml.

    value is the top-level mapping or None; violations are (rule, detail)
    pairs of the yaml-* rules; where maps a JSON path tuple to `line:column`.
    """
    data = text.encode("utf-8")
    violations = []
    value = None
    proc = yq(".", data)
    if proc.returncode != 0:
        violations.append(("yaml-parse", f"{name}: {last_line(proc.stderr)}"))
    else:
        try:
            docs = json_stream(proc.stdout.decode("utf-8"), _last_wins)
        except (ValueError, UnicodeDecodeError) as exc:
            docs = None
            violations.append(("yaml-parse", f"{name}: yq gives no strict JSON: {exc}"))
        if docs is not None:
            if len(docs) > 1:
                violations.append(("yaml-marker", f"{name}: {len(docs)} documents"))
            if docs and type(docs[0]) is dict:
                value = docs[0]
            else:
                violations.append(("yaml-parse", f"{name}: the top level is not a mapping"))
    meta = yq(META_QUERY, data)
    if meta.returncode != 0:
        if proc.returncode == 0:
            violations.append(("yaml-parse", f"{name}: {last_line(meta.stderr)}"))
        return value, violations, {}
    nodes, keys, where = [], [], {}
    for number, doc in enumerate(json_stream(meta.stdout.decode("utf-8"), dict)):
        shift = head_lines(text) if number == 0 else 0
        nodes.extend(n[:4] + [n[4] + shift] + n[5:] for n in doc["nodes"] if n[4] > 0)  # an empty document: 0:0
        keys.extend([[k[0] + shift] + k[1:] for k in mapping if len(k) == 3] for mapping in doc["keys"])  # {}: []
        for path, line, column in doc["paths"]:
            where.setdefault(tuple(path), f"{line + shift}:{column}")
    violations.extend(check_keys(keys, name))
    key_at = {(k[0], k[1]) for mapping in keys for k in mapping}
    violations.extend(check_nodes(nodes, key_at, name))
    violations.extend(scan_raw(text, nodes, name))
    return value, violations, where


def head_lines(text: str) -> int:
    """The leading blank, comment, directive and marker lines, which yq leaves out of its line numbers."""
    count = 0
    for line in LINE_BREAK.split(text):
        if line.strip() and not line.strip().startswith(("#", "%")) and not MARKER.fullmatch(line):
            return count
        count += 1
    return count


def _last_wins(pairs: list) -> dict:
    return dict(pairs)


def check_keys(keys: list, name: str) -> list:
    violations = []
    for mapping in keys:
        seen = set()
        for line, column, key in mapping:
            if key in seen:
                violations.append(("yaml-duplicate-key", f"{name}:{line}:{column}: duplicate key {key!r}"))
            seen.add(key)
    return violations


def check_nodes(nodes: list, key_at: set, name: str) -> list:
    violations = []
    for node in nodes:
        tag, style, kind, anchor, line, column = node[:6]
        raw = node[6] if len(node) > 6 else None
        at = f"{name}:{line}:{column}"

        def hit(rule, what):
            violations.append((rule, f"{at}: {what}"))

        if kind == "alias":
            hit("yaml-anchor-alias", "an alias")
            continue
        if anchor:
            hit("yaml-anchor-alias", f"an anchor {anchor!r}")
        if style in ("tagged", "<unknown>"):
            hit("yaml-tag", f"an explicit tag {tag}")
        if tag == "!!merge":
            hit("yaml-merge-key", "a merge key")
        elif tag in ("!!null", "!!float"):
            hit("yaml-float-null", f"a {tag[2:]} {raw!r}")
        elif tag == "!!timestamp":
            hit("yaml-unquoted-string", f"an unquoted timestamp {raw!r}")
        elif tag not in ("!!map", "!!seq", "!!str", "!!int", "!!bool"):
            hit("yaml-tag", f"a tag {tag}")
        if style in ("literal", "folded"):
            hit("yaml-block-scalar", f"a {style} block scalar")
        elif tag == "!!str" and style == "" and kind == "scalar" and (line, column) not in key_at:
            hit("yaml-unquoted-string", f"an unquoted string {raw!r}")
        if tag == "!!int" and style == "":
            if not INTEGER.fullmatch(raw or "") or abs(int(raw)) > INT_LIMIT:
                hit("yaml-integer", f"an integer written {raw!r}")
        if tag == "!!bool" and style == "" and raw not in ("true", "false"):
            hit("yaml-boolean", f"a boolean written {raw!r}")
    return violations


def quoted_end(line: str, start: int, quote: str) -> tuple:
    """Return (end index of the closing quote or None, bad escapes) of a quoted scalar."""
    escapes = []
    i = start + 1
    while i < len(line):
        ch = line[i]
        if quote == '"' and ch == "\\":
            if i + 1 >= len(line) or line[i + 1] not in '\\"':
                escapes.append(line[i : i + 2])
            i += 2
        elif ch == quote:
            if quote == "'" and line[i + 1 : i + 2] == "'":
                i += 2
                continue
            return i, escapes
        else:
            i += 1
    return None, escapes


def scan_raw(text: str, nodes: list, name: str) -> list:
    violations = []
    lines = LINE_BREAK.split(text)
    spans = {}
    for node in nodes:
        style, kind, line, column = node[1], node[2], node[4], node[5]
        quote = {"double": '"', "single": "'"}.get(style)
        if kind != "scalar" or quote is None:
            continue
        at = f"{name}:{line}:{column}"
        source, start = lines[line - 1] if 1 <= line <= len(lines) else "", column - 1
        anchor = re.match(r"&\S+[ \t]+", source[start:])
        start += anchor.end() if anchor else 0
        if source[start : start + 1] != quote:
            violations.append(("yaml-parse", f"{at}: yq places a quoted string where its text has no quote"))
            continue
        end, escapes = quoted_end(source, start, quote)
        for escape in escapes:
            violations.append(("yaml-escape", f"{at}: the escape {escape!r}"))
        if end is None:
            violations.append(("yaml-multiline-string", f"{at}: the string does not end on its line"))
            end = len(source) - 1
        spans.setdefault(line, []).append((start, end + 1))
    for number, source in enumerate(lines, 1):
        if MARKER.fullmatch(source):
            violations.append(("yaml-marker", f"{name}:{number}: a document marker line"))
        masked = list(source)
        for start, end in spans.get(number, []):
            masked[start:end] = "_" * (end - start)
        code = "".join(masked)
        cut = re.search(r"(^|\s)#", code)
        if cut:
            code = code[: cut.start()]
        if MERGE.search(code):
            violations.append(("yaml-merge-key", f"{name}:{number}: a merge key"))
    return violations


# --- the pack --------------------------------------------------------------


class Param:
    def __init__(self, index: int, name: str, kind: str, default, has_default: bool):
        self.index, self.name, self.type = index, name, kind
        self.default, self.has_default = default, has_default
        self.choices = None
        self.pattern = None


class Pack:
    """What the checks learn about one pack directory."""

    def __init__(self, root, pack_id: str):
        self.root = Path(root)
        self.id = pack_id
        self.dir = self.root / pack_id
        self.data = None
        self.where = {}
        self.texts = {}
        self.settings_files = set()
        self.has_settings_dir = False
        self.params = []
        self.declared = set()
        self.path_names = set()
        self.violations = []

    def add(self, rule: str, detail: str) -> None:
        self.violations.append((rule, " ".join(detail.split())))

    def at(self, path: tuple) -> str:
        where = self.where.get(tuple(path))
        return f"pack.yaml:{where}" if where else "pack.yaml"

    def field(self, path: tuple, message: str) -> str:
        return f"{self.at(path)}: {dotted(path)}: {message}"

    def defaults(self) -> dict:
        return {p.name: p.default for p in self.params if p.has_default}


def dotted(path: tuple) -> str:
    out = ""
    for part in path:
        out += f"[{part}]" if isinstance(part, int) else (f".{part}" if out else part)
    return out or "top level"


def kind_name(kind: str) -> str:
    return KIND_NAMES.get(kind, f"a {kind}")


def is_kind(value, kind: str) -> bool:
    if kind == "string":
        return type(value) is str
    if kind == "integer":
        return type(value) is int
    if kind == "boolean":
        return type(value) is bool
    if kind == "mapping":
        return type(value) is dict
    if kind in ("list", "entries"):
        return type(value) is list
    if kind == "strings":
        return type(value) is list and all(type(v) is str for v in value)
    return True


def typed(obj, key: str, kind: str):
    """obj[key] when it is present and of kind, else None."""
    if type(obj) is not dict or key not in obj or not is_kind(obj[key], kind):
        return None
    return obj[key]


def entries(data, key: str) -> list:
    """The (index, mapping) entries of a list field; other items are left out."""
    items = typed(data, key, "entries") or []
    return [(i, item) for i, item in enumerate(items) if type(item) is dict]


def text_of(value) -> str:
    if type(value) is bool:
        return "true" if value else "false"
    return str(value)


def load_pack(root, pack_id: str) -> tuple:
    """Return (data, violations): pack.yaml's top-level mapping or None, and its yaml-* violations."""
    path = Path(root) / pack_id / "pack.yaml"
    try:
        raw = path.read_bytes()
    except OSError as exc:
        return None, [("layout-files", f"pack.yaml: cannot read: {exc.strerror}")]
    try:
        text = raw.decode("utf-8")
    except UnicodeDecodeError:
        return None, [("encoding", "pack.yaml: not UTF-8")]
    data, violations, _ = read_yaml(text)
    return data, violations


# --- layout ----------------------------------------------------------------


def read_file(pack: Pack, rel: str, path: Path) -> None:
    raw = path.read_bytes()
    if raw.startswith(BOM):
        pack.add("encoding", f"{rel}: starts with a BOM")
        raw = raw[len(BOM) :]
    if b"\r" in raw:
        pack.add("encoding", f"{rel}: holds a CR byte")
    try:
        pack.texts[rel] = raw.decode("utf-8")
    except UnicodeDecodeError as exc:
        pack.add("encoding", f"{rel}: not UTF-8 at byte {exc.start}")


def walk(pack: Pack) -> None:
    for name in sorted(os.listdir(pack.dir)):
        path = pack.dir / name
        mode = os.lstat(path).st_mode
        if name in ("pack.yaml", "conventions.md") and stat.S_ISREG(mode):
            read_file(pack, name, path)
        elif name == "settings" and stat.S_ISDIR(mode):
            pack.has_settings_dir = True
            walk_settings(pack, path, "settings")
        elif stat.S_ISLNK(mode):
            pack.add("layout-files", f"{name}: a symlink")
        else:
            pack.add("layout-files", f"{name}: not part of a pack")
    for name in ("pack.yaml", "conventions.md"):
        if not os.path.lexists(pack.dir / name):
            pack.add("layout-files", f"{name}: missing")
    text = pack.texts.get("conventions.md")
    if text is not None and not text.split("\n", 1)[0].startswith("# "):
        pack.add("conventions-title", "conventions.md: the first line is not a `# ` title")


def walk_settings(pack: Pack, directory: Path, rel: str) -> None:
    names = sorted(os.listdir(directory))
    if not names and rel != "settings":
        pack.add("layout-files", f"{rel}: an empty directory")
    for name in names:
        path, sub = directory / name, f"{rel}/{name}"
        mode = os.lstat(path).st_mode
        if stat.S_ISLNK(mode):
            pack.add("layout-files", f"{sub}: a symlink")
        elif stat.S_ISDIR(mode):
            walk_settings(pack, path, sub)
        elif stat.S_ISREG(mode):
            pack.settings_files.add(sub)
            read_file(pack, sub, path)
        else:
            pack.add("layout-files", f"{sub}: not a regular file")


# --- fields ----------------------------------------------------------------


def check_table(pack: Pack, obj, fields: tuple, path: tuple):
    if type(obj) is not dict:
        pack.add("field-type", pack.field(path, "is not a mapping"))
        return None
    names = [f[0] for f in fields]
    for key in obj:
        if key not in names:
            pack.add("field-unknown", pack.field(path + (key,), "is not a field"))
    for name, kind, required in fields:
        if name not in obj:
            if required:
                pack.add("field-missing", pack.field(path, f"has no {name!r}"))
        elif not is_kind(obj[name], kind):
            pack.add("field-type", pack.field(path + (name,), f"is not {kind_name(kind)}"))
    return obj


def check_enum(pack: Pack, obj: dict, key: str, values: tuple, path: tuple):
    value = typed(obj, key, "string")
    if value is not None and value not in values:
        pack.add("field-enum", pack.field(path + (key,), f"{value!r} is not one of {', '.join(values)}"))
        return None
    return value


def check_fields(pack: Pack) -> None:
    data = pack.data
    check_table(pack, data, TOP_FIELDS, ())
    pack_id = typed(data, "id", "string")
    if pack_id is not None and (not PACK_ID.fullmatch(pack_id) or pack_id != pack.id):
        pack.add("id", pack.field(("id",), f"{pack_id!r} is not a valid id equal to the directory {pack.id!r}"))
    version = typed(data, "version", "integer")
    if version is not None and version < 1:
        pack.add("version-min", pack.field(("version",), f"{version} is below 1"))
    if "detect" in data:
        detect = check_table(pack, data["detect"], DETECT_FIELDS, ("detect",))
        if detect is not None:
            lists = [detect.get(k, []) for k in ("files", "shebangs")]
            if all(type(v) is list for v in lists) and not sum(len(v) for v in lists):
                pack.add("detect-empty", pack.field(("detect",), "lists no file glob or shebang"))
    for key, fields in (("parameters", PARAMETER_FIELDS), ("settings", SETTING_FIELDS), ("gates", GATE_FIELDS)):
        for i, item in enumerate(typed(data, key, "entries") or []):
            check_table(pack, item, fields, (key, i))


def check_parameters(pack: Pack) -> None:
    seen = set()
    for i, entry in entries(pack.data, "parameters"):
        path = ("parameters", i)
        name = typed(entry, "name", "string")
        kind = check_enum(pack, entry, "type", PARAMETER_TYPES, path)
        if name is not None:
            if not PARAM_NAME.fullmatch(name):
                pack.add("param-name", pack.field(path + ("name",), f"{name!r} is not a valid name"))
                name = None
            elif name in seen:
                pack.add("param-unique", pack.field(path + ("name",), f"{name!r} is repeated"))
                name = None
            else:
                seen.add(name)
        pattern = typed(entry, "pattern", "string")
        compiled = None
        if "pattern" in entry and kind in ("integer", "boolean"):
            pack.add("param-pattern", pack.field(path + ("pattern",), f"is not allowed for {kind}"))
        elif pattern is not None:
            try:
                compiled = re.compile(pattern)
            except re.error as exc:
                pack.add("param-pattern", pack.field(path + ("pattern",), f"does not compile: {exc}"))
        if kind is None:
            continue
        default = entry.get("default")
        has_default = "default" in entry and is_kind(default, kind)
        if "default" in entry and not has_default:
            pack.add("param-default", pack.field(path + ("default",), f"{default!r} is not {kind_name(kind)}"))
        param = Param(i, name, kind, default, has_default)
        param.pattern = compiled
        check_choices(pack, entry, param, path)
        if name is not None:
            pack.params.append(param)
        if kind == "string":
            values = [("default", default)] if has_default else []
            values += [(f"choices[{k}]", v) for k, v in enumerate(param.choices or [])]
            for label, value in values:
                for problem in string_problems(param, value, name in pack.path_names):
                    pack.add("value-string", pack.field(path, f"{label} {value!r} {problem}"))


def check_choices(pack: Pack, entry: dict, param: Param, path: tuple) -> None:
    if "choices" not in entry:
        return
    where = path + ("choices",)
    if param.type == "boolean":
        pack.add("param-choices", pack.field(where, "is not allowed for boolean"))
        return
    choices = typed(entry, "choices", "list")
    if choices is None:
        return
    if not choices:
        pack.add("param-choices", pack.field(where, "is empty"))
        return
    good = True
    for k, value in enumerate(choices):
        if not is_kind(value, param.type):
            pack.add("param-choices", pack.field(where + (k,), f"{value!r} is not {kind_name(param.type)}"))
            good = False
        elif value in choices[:k]:
            pack.add("param-choices", pack.field(where + (k,), f"{value!r} is repeated"))
    if good:
        param.choices = choices
        if param.has_default and param.default not in choices:
            pack.add("param-choices", pack.field(where, f"does not hold the default {param.default!r}"))


def string_problems(param: Param, value: str, in_paths: bool) -> list:
    problems = []
    if any(unicodedata.category(c) in ("Cc", "Cf") or c in "\u2028\u2029" for c in value):
        problems.append("holds a control character")
    if param.pattern is not None and not param.pattern.fullmatch(value):
        problems.append(f"does not fully match the pattern {param.pattern.pattern!r}")
    if in_paths and (not PATH_VALUE.fullmatch(value) or value.startswith("-")):
        problems.append("is used in a path or command, so must fully match [A-Za-z0-9._/+-]+ and not start with '-'")
    return problems


def allowed_strings(data) -> list:
    """(path, string) of every place pack.yaml may hold placeholders."""
    out = []
    for i, entry in entries(data, "settings"):
        if type(entry.get("target")) is str:
            out.append((("settings", i, "target"), entry["target"]))
        for k, alt in enumerate(entry.get("alternatives") if type(entry.get("alternatives")) is list else []):
            if type(alt) is str:
                out.append((("settings", i, "alternatives", k), alt))
    for i, entry in entries(data, "gates"):
        if type(entry.get("command")) is str:
            out.append((("gates", i, "command"), entry["command"]))
    return out


def all_strings(value, path: tuple = ()):
    if type(value) is str:
        yield path, value
    elif type(value) is dict:
        for key, item in value.items():
            yield from all_strings(item, path + (key,))
    elif type(value) is list:
        for k, item in enumerate(value):
            yield from all_strings(item, path + (k,))


def check_placeholders(pack: Pack) -> None:
    """Find every placeholder; must run before the parameter checks, which need path_names."""
    allowed = allowed_strings(pack.data)
    allowed_paths = {path for path, _ in allowed}
    for path, text in all_strings(pack.data):
        if path not in allowed_paths and PLACEHOLDER.search(text):
            pack.add("placeholder-not-allowed", pack.field(path, f"{text!r} holds a placeholder"))
    uses = []
    for path, text in allowed:
        names = PLACEHOLDER.findall(text)
        pack.path_names.update(names)
        uses += [(pack.field(path, "the placeholder"), n) for n in names]
    for rel in ["conventions.md"] + sorted(pack.settings_files):
        uses += [(f"{rel}: the placeholder", n) for n in PLACEHOLDER.findall(pack.texts.get(rel, ""))]
    for i, entry in entries(pack.data, "parameters"):
        if type(entry.get("name")) is str:
            pack.declared.add(entry["name"])
    for where, name in uses:
        if name not in pack.declared:
            pack.add("placeholder-undeclared", f"{where} {{{{ {name} }}}} names no parameter of the pack")
    used = {name for _, name in uses}
    for i, entry in entries(pack.data, "parameters"):
        name = entry.get("name")
        if type(name) is str and PARAM_NAME.fullmatch(name) and name not in used:
            pack.add("placeholder-unused", pack.field(("parameters", i, "name"), f"{name!r} is in no placeholder"))


def placeholder_segments(text: str) -> bool:
    for m in PLACEHOLDER.finditer(text):
        if (m.start() > 0 and text[m.start() - 1] != "/") or text[m.end() : m.end() + 1] != "/":
            return False
    return True


def path_problem(path: str, dot: bool = True) -> tuple:
    """Return (path with `.` segments dropped, None) or (None, the problem)."""
    if path == "":
        return None, "is empty"
    if "\\" in path:
        return None, "holds a backslash"
    if path.startswith("/"):
        return None, "is absolute"
    segments = path.split("/")
    if not dot and "." in segments:
        return None, "has a '.' segment"
    kept = [s for s in segments if s != "."]
    if not kept:
        return None, "is empty once '.' segments are dropped"
    for segment in kept:
        if segment == "":
            return None, "has an empty segment"
        if segment == ".." or segment.lower() == ".git":
            return None, f"has a {segment!r} segment"
    return "/".join(kept), None


def check_settings(pack: Pack) -> None:
    items = typed(pack.data, "settings", "entries") if "settings" in pack.data else []
    if items is not None and bool(items) != pack.has_settings_dir:
        what = "settings/ is missing" if items else "settings/ is present, but there are no settings entries"
        pack.add("layout-settings-dir", what)
    templates = set()
    for i, entry in entries(pack.data, "settings"):
        path = ("settings", i)
        check_enum(pack, entry, "kind", SETTING_KINDS, path)
        mode = check_enum(pack, entry, "mode", SETTING_MODES, path) if "mode" in entry else "file"
        if mode == "fragment" and "section" not in entry:
            pack.add("section", pack.field(path, "is a fragment without a section"))
        elif mode == "file" and "section" in entry:
            pack.add("section", pack.field(path + ("section",), "is not allowed in file mode"))
        template = typed(entry, "template", "string")
        if template is not None:
            templates.add(template)
            _, problem = path_problem(template, dot=False)
            if problem is None and GLOB_CHARS & set(template):
                problem = "holds a glob character"
            if problem is None and not template.startswith("settings/"):
                problem = "does not start with settings/"
            if problem is None and template not in pack.settings_files:
                problem = "names no file of the pack"
            if problem:
                pack.add("path-template", pack.field(path + ("template",), f"{template!r} {problem}"))
        for where, text in allowed_strings({"settings": [entry]}):
            if not placeholder_segments(text):
                where = path + where[2:]
                pack.add("path-placeholder-segment", pack.field(where, f"{text!r}: a placeholder is not a segment"))
    if items is not None:
        for rel in sorted(pack.settings_files - templates):
            pack.add("layout-unreferenced", f"{rel}: no settings entry names it")


def check_gates(pack: Pack) -> None:
    seen = set()
    gate_name = re.compile(re.escape(pack.id) + r"-[a-z0-9][a-z0-9-]*")
    for i, entry in entries(pack.data, "gates"):
        path = ("gates", i)
        check_enum(pack, entry, "kind", GATE_KINDS, path)
        name = typed(entry, "name", "string")
        if name is not None:
            if not gate_name.fullmatch(name):
                pack.add("gate-name", pack.field(path + ("name",), f"{name!r} does not match {gate_name.pattern}"))
            if name in seen:
                pack.add("gate-unique", pack.field(path + ("name",), f"{name!r} is repeated"))
            seen.add(name)
        timeout = typed(entry, "timeout", "integer")
        if timeout is not None and timeout < 1:
            pack.add("gate-timeout", pack.field(path + ("timeout",), f"{timeout} is below 1"))


def analyse(root, pack_id: str) -> Pack:
    """Every check of one pack that needs neither its base nor rendering."""
    pack = Pack(root, pack_id)
    walk(pack)
    if "pack.yaml" in pack.texts:
        pack.data, violations, pack.where = read_yaml(pack.texts["pack.yaml"])
        pack.violations.extend(violations)
    if pack.data is not None:
        check_fields(pack)
        check_placeholders(pack)
        check_parameters(pack)
        check_settings(pack)
        check_gates(pack)
    return pack


# --- rendering -------------------------------------------------------------


def render_text(template: str, values: dict) -> str:
    """The README's single pass; a name without a value stays as it is."""
    texts = {name: text_of(value) for name, value in values.items()}
    return PLACEHOLDER.sub(lambda m: texts.get(m.group(1), m.group(0)), template)


def render_parts(pack: Pack, values: dict) -> dict:
    """The rendered texts with their indexes; paths are as rendered, `.` segments kept."""
    settings = []
    for i, entry in entries(pack.data, "settings"):
        template = typed(entry, "template", "string")
        target = typed(entry, "target", "string")
        mode = typed(entry, "mode", "string") or "file"
        text = pack.texts.get(template) if template in pack.settings_files else None
        settings.append(
            {
                "index": i,
                "template": template,
                "target": None if target is None else render_text(target, values),
                "kind": typed(entry, "kind", "string"),
                "mode": mode,
                "section": typed(entry, "section", "string") if mode == "fragment" else None,
                "optional": typed(entry, "optional", "boolean") or False,
                "alternatives": [render_text(a, values) for a in typed(entry, "alternatives", "strings") or []],
                "text": None if text is None else render_text(text, values),
            }
        )
    gates = []
    for i, entry in entries(pack.data, "gates"):
        command = typed(entry, "command", "string")
        gate = {"index": i, "name": typed(entry, "name", "string")}
        gate["command"] = None if command is None else render_text(command, values)
        if "timeout" in entry:
            gate["timeout"] = entry["timeout"]
        gates.append(gate)
    return {
        "conventions": render_text(pack.texts.get("conventions.md", ""), values),
        "settings": settings,
        "gates": gates,
    }


def normalised(path):
    if path is None:
        return None
    kept, problem = path_problem(path)
    return path if problem else kept


def render_pack(pack: Pack, values: dict) -> dict:
    """The render output for a pack checked by analyse, with values for every parameter."""
    parts = render_parts(pack, values)
    settings = []
    for s in parts["settings"]:
        entry = {k: s[k] for k in ("template", "target", "kind", "mode", "section", "optional", "alternatives", "text")}
        entry["target"] = normalised(s["target"])
        entry["alternatives"] = [normalised(a) for a in s["alternatives"]]
        settings.append(entry)
    return {
        "pack": pack.data.get("id"),
        "version": pack.data.get("version"),
        "values": {p.name: values.get(p.name, p.default) for p in pack.params},
        "conventions": parts["conventions"],
        "settings": settings,
        "gates": [{k: v for k, v in g.items() if k != "index"} for g in parts["gates"]],
    }


def rendered_problems(pack: Pack, values: dict, base_entries=None) -> tuple:
    """Return (violations, entries) of the rules checked on rendered text."""
    parts = render_parts(pack, values)
    violations = []

    def add(rule, path, message):
        violations.append((rule, " ".join(pack.field(path, message).split())))

    own = []
    for s in parts["settings"]:
        path = ("settings", s["index"])
        if s["target"] is not None:
            kept, problem = path_problem(s["target"])
            if problem is None and GLOB_CHARS & set(s["target"]):
                problem = "holds a glob character"
            if problem:
                add("path-target", path + ("target",), f"renders to {s['target']!r}, which {problem}")
            else:
                own.append((dotted(path), kept, s["mode"], s["section"]))
        for k, alt in enumerate(s["alternatives"]):
            _, problem = path_problem(alt)
            if problem:
                add("path-alternatives", path + ("alternatives", k), f"renders to {alt!r}, which {problem}")
        if s["mode"] == "fragment" and s["section"] is not None and s["text"] is not None:
            for problem in fragment_problems(s["text"], s["section"]):
                add("fragment-header", path + ("template",), f"{s['template']}: {problem}")
    combined = (base_entries or []) + own
    for problem in unique_problems(combined):
        violations.append(("settings-unique", f"pack.yaml: {problem}"))
    for g in parts["gates"]:
        if g["command"] is None:
            continue
        try:
            words = shlex.split(g["command"])
        except ValueError as exc:
            words = None
            add("gate-command", ("gates", g["index"], "command"), f"{g['command']!r}: {exc}")
        if words == []:
            add("gate-command", ("gates", g["index"], "command"), f"{g['command']!r} has no word")
    return violations, own


def fragment_problems(text: str, section: str) -> list:
    problems = []
    lines = text.split("\n")
    if not HEADER_LINE.fullmatch(lines[0].strip()):
        problems.append("does not start with a header line")
    seen = set()
    for number, line in enumerate(lines, 1):
        stripped = line.strip()
        if DOUBLE_HEADER.fullmatch(stripped):
            problems.append(f"line {number}: a [[...]] header")
        m = HEADER_LINE.fullmatch(stripped)
        if not m:
            continue
        name = m.group(1)
        if name != section and not (name.startswith(section + ".") and len(name) > len(section) + 1):
            problems.append(f"line {number}: the header {name!r} is not {section!r} or under it")
        if name in seen:
            problems.append(f"line {number}: the header {name!r} is repeated")
        seen.add(name)
    return problems


def unique_problems(items: list) -> list:
    problems = []
    pairs, by_target = {}, {}
    for label, target, mode, section in items:
        key = (target, section)
        if key in pairs and "file" not in (mode, pairs[key][1]):  # a file entry is reported below
            problems.append(f"{label} repeats the target {target!r} and section {section!r} of {pairs[key][0]}")
        pairs.setdefault(key, (label, mode))
        by_target.setdefault(target, []).append((label, mode))
    for target, users in by_target.items():
        files = [label for label, mode in users if mode == "file"]
        if files and len(users) > 1:
            others = ", ".join(label for label, _ in users if label != files[0])
            problems.append(f"{files[0]} is a file entry whose target {target!r} {others} also use")
    return problems


# --- base packs and the whole check ---------------------------------------


def is_pack_dir(path: Path) -> bool:
    return path.is_dir() and not path.is_symlink()


def check_base(pack: Pack):
    """Apply the base rule; return the base's rendered entries when the base itself is clean."""
    base_id = typed(pack.data, "base", "string")
    if base_id is None:
        return None
    if base_id == pack.id:
        pack.add("base", pack.field(("base",), "names the pack itself"))
        return None
    if "/" in base_id or base_id in (".", "..") or not is_pack_dir(pack.root / base_id):
        pack.add("base", pack.field(("base",), f"{base_id!r} names no pack"))
        return None
    base = analyse(pack.root, base_id)
    if base.data is not None and "base" in base.data:
        pack.add("base", pack.field(("base",), f"{base_id!r} has a base of its own"))
        return None
    if base.data is None or base.violations:
        return None
    problems, own = rendered_problems(base, base.defaults())
    if problems:
        return None
    return [(f"{base_id}: {label}", target, mode, section) for label, target, mode, section in own]


def checked(root, pack_id: str) -> tuple:
    """Return (pack, violations) for every rule but version-bump."""
    pack = analyse(root, pack_id)
    if pack.data is not None:
        base_entries = check_base(pack)
        problems, _ = rendered_problems(pack, pack.defaults(), base_entries)
        pack.violations.extend(problems)
    unique = []
    for violation in pack.violations:
        if violation not in unique:
            unique.append(violation)
    return pack, unique


def check_pack(root, pack_id: str) -> list:
    """The (rule, detail) violations of one pack, the version rule apart."""
    return checked(root, pack_id)[1]


# --- the version rule ------------------------------------------------------


class History:
    def __init__(self, root: Path, ref: str):
        proc = run(["git", "rev-parse", "--show-toplevel"], cwd=root)
        if proc.returncode != 0:
            raise InputError(f"--base needs {root} inside a git work tree: {last_line(proc.stderr)}")
        self.top = proc.stdout.decode("utf-8").strip()
        self.prefix = os.path.relpath(os.path.realpath(root), os.path.realpath(self.top))
        proc = self.git("rev-parse", "--verify", "--quiet", "--end-of-options", f"{ref}^{{commit}}")
        if proc.returncode != 0:
            raise InputError(f"--base {ref!r} does not resolve to a commit")
        commit = proc.stdout.decode().strip()
        proc = self.git("merge-base", commit, "HEAD")
        if proc.returncode != 0:
            raise InputError(f"no merge base of {ref!r} and HEAD")
        self.merge_base = proc.stdout.decode().strip()

    def git(self, *args: str) -> subprocess.CompletedProcess:
        return run(["git", "-c", "core.quotePath=false", *args], cwd=self.top)

    def path(self, pack_id: str) -> str:
        return pack_id if self.prefix == "." else f"{self.prefix}/{pack_id}"

    def listed(self, *args: str) -> bool:
        proc = self.git(*args)
        if proc.returncode != 0:
            raise InputError(f"git {args[0]} failed: {last_line(proc.stderr)}")
        return bool(proc.stdout.strip())

    def changed(self, pack_id: str) -> bool:
        spec = f":(literal){self.path(pack_id)}"
        return self.listed("diff", "--name-only", "--no-renames", self.merge_base, "--", spec) or self.listed(
            "ls-files", "--others", "--exclude-standard", "--", spec
        )

    def base_version(self, pack_id: str):
        """The merge base's version of the pack, or None when it has no pack.yaml there."""
        blob = f"{self.merge_base}:{self.path(pack_id)}/pack.yaml"
        if self.git("cat-file", "-e", blob).returncode != 0:
            return None
        proc = self.git("show", blob)
        converted = yq(".", proc.stdout) if proc.returncode == 0 else proc
        try:
            version = json_stream(converted.stdout.decode("utf-8"), _last_wins)[0]["version"]
        except (ValueError, UnicodeDecodeError, IndexError, KeyError, TypeError):
            version = None
        if converted.returncode != 0 or type(version) is not int:
            raise InputError(f"cannot read the version of {blob}")
        return version

    def violations(self, pack: Pack) -> list:
        version = typed(pack.data, "version", "integer")
        if version is None or not self.changed(pack.id):
            return []
        base = self.base_version(pack.id)
        if base is None and version != 1:
            return [("version-bump", f"pack.yaml: a new pack starts at version 1, not {version}")]
        if base is not None and version != base + 1:
            return [
                ("version-bump", f"pack.yaml: version {version}; the pack changed since {base}, so it is {base + 1}")
            ]
        return []


# --- commands --------------------------------------------------------------


def option_value(argv: list, i: int, options: dict) -> int:
    name = argv[i]
    if name in options:
        raise InputError(f"{name} given twice; {USAGE}")
    if i + 1 >= len(argv) or argv[i + 1] == "" or argv[i + 1].startswith("-"):
        raise InputError(f"{name} needs a value; {USAGE}")
    options[name] = argv[i + 1]
    return i + 2


def pack_root(options: dict) -> Path:
    root = Path(options.get("--root", DEFAULT_ROOT))
    if not root.is_dir():
        raise InputError(f"the pack root {str(root)!r} is not a directory")
    return root


def report(pack_id: str, violations: list, stream) -> None:
    for rule, detail in violations:
        print(f"{pack_id}: {rule}: {detail}", file=stream)


def check(argv: list) -> int:
    options, i = {}, 0
    while i < len(argv):
        if argv[i] not in ("--root", "--base"):
            raise InputError(f"unknown argument {argv[i]!r}; {USAGE}")
        i = option_value(argv, i, options)
    root = pack_root(options)
    probe()
    history = History(root, options["--base"]) if "--base" in options else None
    ids, count = [], 0
    for name in sorted(os.listdir(root)):
        if is_pack_dir(root / name):
            ids.append(name)
        elif (root / name).is_symlink() and (root / name).is_dir():
            report(name, [("layout-files", f"{name}: a symlink to a directory in the pack root")], sys.stdout)
            count += 1
    for pack_id in ids:
        pack, violations = checked(root, pack_id)
        if history is not None and pack.data is not None:
            violations += history.violations(pack)
        report(pack_id, violations, sys.stdout)
        count += len(violations)
    if history is None:
        print("check: version-bump not evaluated (no --base)")
    print(f"check: {len(ids)} packs, {count} violations")
    return 1 if count else 0


def literal(param: Param, text: str):
    if param.type == "integer":
        if not INTEGER.fullmatch(text) or abs(int(text)) > INT_LIMIT:
            raise InputError(f"{param.name}={text!r} is not a plain decimal integer within +-{INT_LIMIT}")
        return int(text)
    if param.type == "boolean":
        if text not in ("true", "false"):
            raise InputError(f"{param.name}={text!r} is not true or false")
        return text == "true"
    return text


def render(argv: list) -> int:
    options, pack_id, given, i = {}, None, {}, 0
    while i < len(argv):
        arg = argv[i]
        if arg == "--root":
            i = option_value(argv, i, options)
            continue
        if pack_id is None:
            if arg.startswith("-") or "=" in arg:
                raise InputError(f"render needs a PACK first, not {arg!r}; {USAGE}")
            pack_id = arg
        elif "=" not in arg:
            raise InputError(f"{arg!r} is not name=value")
        else:
            name, value = arg.split("=", 1)
            if name in given:
                raise InputError(f"{name} given twice")
            given[name] = value
        i += 1
    if pack_id is None:
        raise InputError(f"render needs a PACK; {USAGE}")
    root = pack_root(options)
    probe()
    if pack_id in (".", "..") or "/" in pack_id or not is_pack_dir(root / pack_id):
        raise InputError(f"no pack {pack_id!r} under {root}")
    pack, violations = checked(root, pack_id)
    if violations:
        report(pack_id, violations, sys.stderr)
        return 1
    params = {p.name: p for p in pack.params}
    values = pack.defaults()
    for name, text in given.items():
        if name not in params:
            raise InputError(f"{pack_id} declares no parameter {name!r}")
        values[name] = literal(params[name], text)
    for name in given:
        param, value = params[name], values[name]
        if param.choices is not None and value not in param.choices:
            violations.append(("param-choices", f"{name}={value!r} is not one of {param.choices!r}"))
        if param.type == "string":
            for problem in string_problems(param, value, name in pack.path_names):
                violations.append(("value-string", f"{name}={value!r} {problem}"))
    problems, _ = rendered_problems(pack, values)
    violations += problems
    if violations:
        report(pack_id, violations, sys.stderr)
        return 1
    print(json.dumps(render_pack(pack, values), ensure_ascii=False, indent=2))
    return 0


def main(argv: list) -> int:
    try:
        if argv[:1] == ["check"]:
            return check(argv[1:])
        if argv[:1] == ["render"]:
            return render(argv[1:])
        raise InputError(USAGE)
    except InputError as exc:
        print(f"pack.py: {exc}", file=sys.stderr)
        return 2
    except Exception as exc:  # a crash must never read as a pass or a violation
        message = " ".join(str(exc).split())
        print(f"pack.py: unexpected error: {type(exc).__name__}: {message}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
