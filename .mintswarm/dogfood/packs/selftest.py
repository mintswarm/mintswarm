#!/usr/bin/env python3
"""Offline self-test of pack.py, the guideline pack checker and renderer.

    selftest.py [unittest options]

Needs `yq` and `git` on PATH; nothing else, and no network. It writes
fixture packs into a fresh temporary directory and runs pack.py on them as
`sys.executable -B pack.py ...`, a subprocess, so exit codes and output are
seen as a gate sees them. Fixtures are built in code: `alpha`, a valid base
pack with string, integer and boolean parameters, a fragment and a file
setting (its template in a subdirectory of settings/) and two gates;
`beta`, a valid pack derived from it; `plain`, a valid pack with no
parameters, settings or gates. Each failing row applies small mutations
to them, to the pack.yaml data, a file or the YAML text, and names the exact set of
`<pack>: <rule>` pairs check must print.

Checks:

- passing: the valid fixtures, and edge cases, each exit 0 with no
  violation: integer 0, a single-quoted pattern with `\\d`,
  `${{ secrets.TOKEN }}`, `{{- x }}` and `{{ Name }}` as literal text,
  `{{name}}` without spaces, a subdirectory under settings/, a derived pack
  on a valid base, a `<section>.<rest>` fragment header, and pack.yaml
  starting with one or two comment lines;
- failing rows: each exits 1 and prints exactly the expected pairs; every
  rule named in pack.py's docstring has at least one failing row
  (version-bump in the git cases); among them an escape and a two-line
  string after leading comment lines;
- positions: line:column after leading comment lines, and one
  settings-unique line for two file entries sharing a target;
- the README: the `sample` pack of defaults/packs/README.md's Example
  section, read at test time, passes check, and render with defaults gives
  the targets, alternative, gates and conventions the README states, and
  its adapted-copy block;
- render: the JSON object's keys and values with defaults and given values,
  the single-pass rule, a derived pack rendered alone, and exit 1 with
  nothing on stdout for a pack with violations and for given values outside
  `choices`, not matching `pattern`, `..` in a path or starting with `-`;
- unusable input exits 2 with nothing on stdout: bad arguments, a missing
  root, yq missing from PATH, a fake yq that answers the probe wrongly, an
  unknown --base, --base outside a git work tree, and render with an unknown
  pack, an argument without `=`, an undeclared or repeated name, or
  `1.5`, `010` and `yes` as typed values; a root with no pack directories
  exits 0;
- the version rule, in a fixture git repository: a changed pack at the base
  version plus one passes, at the same version or plus two fails; a new pack
  at 1 passes, at 2 fails (committed or untracked); an unchanged pack
  passes; a pack that differs only by an untracked file needs the bump.

Prints one line per check and exits 0 only when every check passed.
Standard library only.
"""

import ast
import copy
import json
import os
import re
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent
REPO = HERE.parents[2]
SCRIPT = HERE / "pack.py"
README = REPO / "defaults" / "packs" / "README.md"
ENV = dict(os.environ, PYTHONDONTWRITEBYTECODE="1")
DIR = object()  # a fixture entry that is an empty directory
TMP = None

ALPHA = {
    "id": "alpha",
    "version": 1,
    "name": "Alpha",
    "description": "A fixture base pack.",
    "detect": {"files": ["**/*.alp"], "shebangs": ["alpha"]},
    "parameters": [
        {"name": "package_root", "type": "string", "default": ".", "description": "Where is the package?"},
        {
            "name": "line_length",
            "type": "integer",
            "default": 100,
            "description": "How long may a line be?",
            "choices": [80, 100, 120],
        },
        {"name": "strict", "type": "boolean", "default": True, "description": "Is the linter strict?"},
        {"name": "greeting", "type": "string", "default": "hello", "description": "How to greet?"},
        {"name": "other", "type": "string", "default": "world", "description": "Whom?", "pattern": "[a-z]+"},
        {
            "name": "flavour",
            "type": "string",
            "default": "mild",
            "description": "Which flavour?",
            "choices": ["mild", "hot"],
            "pattern": "[a-z]+",
        },
    ],
    "settings": [
        {
            "template": "settings/pyproject.toml",
            "target": "{{ package_root }}/pyproject.toml",
            "kind": "formatter",
            "mode": "fragment",
            "section": "tool.alpha",
        },
        {
            "template": "settings/editor/alpha.toml",
            "target": "{{ package_root }}/.alpha.toml",
            "kind": "linter",
            "optional": True,
            "alternatives": ["{{ package_root }}/setup.cfg"],
        },
    ],
    "gates": [
        {"name": "alpha-format", "command": "alpha fmt --check {{ package_root }}", "kind": "format"},
        {
            "name": "alpha-lint",
            "command": "alpha lint --strict={{ strict }} {{ package_root }}",
            "kind": "lint",
            "timeout": 600,
        },
    ],
}
ALPHA_FILES = {
    "conventions.md": "# Alpha conventions\n\nSay {{ greeting }} to {{other}}, {{ flavour }}.\n",
    "settings/pyproject.toml": "[tool.alpha]\nline-length = {{ line_length }}\n\n"
    "[tool.alpha.extra]\nstrict = {{ strict }}\n",
    "settings/editor/alpha.toml": 'flavour = "{{ flavour }}"\n',
}
BETA = {
    "id": "beta",
    "version": 1,
    "name": "Beta",
    "base": "alpha",
    "detect": {"files": ["**/*.bet"]},
    "parameters": [{"name": "beta_root", "type": "string", "default": "web", "description": "Where is Beta?"}],
    "settings": [
        {
            "template": "settings/pyproject.toml",
            "target": "{{ beta_root }}/pyproject.toml",
            "kind": "linter",
            "mode": "fragment",
            "section": "tool.beta",
        }
    ],
    "gates": [{"name": "beta-lint", "command": "beta lint {{ beta_root }}", "kind": "lint"}],
}
BETA_FILES = {"conventions.md": "# Beta conventions\n", "settings/pyproject.toml": "[tool.beta]\n"}
PLAIN = {"id": "plain", "version": 1, "name": "Plain", "detect": {"shebangs": ["plain"]}}
PLAIN_FILES = {"conventions.md": "# Plain conventions\n"}


# --- fixtures --------------------------------------------------------------


def scalar(value) -> str:
    if type(value) is bool:
        return "true" if value else "false"
    if type(value) in (int, str):
        return json.dumps(value, ensure_ascii=False)
    if type(value) is list:
        return "[" + ", ".join(scalar(v) for v in value) + "]"
    if type(value) is dict and not value:
        return "{}"
    raise TypeError(f"cannot emit {value!r}")


def emit(obj: dict, indent: int = 0) -> list:
    """Block YAML obeying the README: quoted strings, flow lists of scalars."""
    pad, lines = " " * indent, []
    for key, value in obj.items():
        if type(value) is dict and value:
            lines.append(f"{pad}{key}:")
            lines += emit(value, indent + 2)
        elif type(value) is list and value and all(type(v) is dict and v for v in value):
            lines.append(f"{pad}{key}:")
            for item in value:
                sub = emit(item)
                lines.append(f"{pad}  - {sub[0]}")
                lines += [f"{pad}    {line}" for line in sub[1:]]
        else:
            lines.append(f"{pad}{key}: {scalar(value)}")
    return lines


class Fixture:
    def __init__(self):
        self.packs = {
            "alpha": [copy.deepcopy(ALPHA), dict(ALPHA_FILES)],
            "beta": [copy.deepcopy(BETA), dict(BETA_FILES)],
            "plain": [copy.deepcopy(PLAIN), dict(PLAIN_FILES)],
        }
        self.edits = []

    def data(self, pack: str) -> dict:
        return self.packs[pack][0]

    def files(self, pack: str) -> dict:
        return self.packs[pack][1]

    def yaml(self, pack: str, old: str, new: str) -> None:
        self.edits.append((pack, old, new))

    def write(self, root: Path) -> Path:
        root.mkdir(parents=True)
        for pack_id, (data, files) in self.packs.items():
            directory = root / pack_id
            directory.mkdir()
            text = "\n".join(emit(data)) + "\n"
            for pack, old, new in self.edits:
                if pack == pack_id:
                    if old not in text:
                        raise AssertionError(f"fixture edit: {old!r} is not in {pack}/pack.yaml")
                    text = text.replace(old, new, 1)
            for rel, content in dict({"pack.yaml": text}, **files).items():
                if content is None:
                    continue
                path = directory / rel
                path.parent.mkdir(parents=True, exist_ok=True)
                if content is DIR:
                    path.mkdir()
                elif type(content) is tuple:
                    path.symlink_to(content[1])
                else:
                    path.write_bytes(content if type(content) is bytes else content.encode("utf-8"))
        return root


def run_pack(args: list, cwd=None, env=None) -> subprocess.CompletedProcess:
    return subprocess.run(
        [sys.executable, "-B", str(SCRIPT), *args], cwd=cwd, env=env or ENV, capture_output=True, text=True
    )


def found(stdout: str) -> set:
    pairs = re.findall(r"^([^:\s]+): ([a-z][a-z-]*): ", stdout, re.MULTILINE)
    return {f"{pack}: {rule}" for pack, rule in pairs if pack != "check"}


def fresh(name: str) -> Path:
    return Path(tempfile.mkdtemp(prefix=re.sub(r"[^a-z0-9]+", "-", name.lower())[:30] + "-", dir=TMP))


def check_fixture(fixture: Fixture, name: str) -> subprocess.CompletedProcess:
    return run_pack(["check", "--root", str(fixture.write(fresh(name) / "packs"))])


# --- rows ------------------------------------------------------------------
#
# A row is (name, ops, expected). An op is a tuple whose first item is the
# operation, prefixed `<pack>.` when it is not alpha's:
#   ("top", key, value)            set a top-level field (DROP deletes it)
#   ("detect" | "param" NAME | "setting" I | "gate" I, key, value)
#   ("add", list, entry)           append an entry; a str entry copies that parameter, an int that setting
#   ("file", rel, content)         set a file: text, bytes, None (absent), DIR or ("link", target)
#   ("text", rel, old, new)        replace text in a file; ("yaml", old, new) in the emitted pack.yaml
#   ("header", new)                replace alpha's [tool.alpha.extra] fragment header
# expected is space-separated `rule` (alpha's) or `pack:rule`.

DROP = object()
R, PY, DESC = "{{ package_root }}", "settings/pyproject.toml", '"A fixture base pack."'
RETRIES = {"name": "retries", "type": "integer", "default": 0, "description": "How many retries?"}
SPARE = {"name": "spare", "type": "boolean", "default": False, "description": "Spare?"}
NO_WORD = {"name": "alpha-x", "command": " ", "kind": "test"}
HEAD1, HEAD2 = ("yaml", 'id: "alpha"', '# Alpha\nid: "alpha"'), (
    "yaml",
    'id: "alpha"',
    '# Alpha\n# pack\n\nid: "alpha"',
)
ON_MARKER = '--- {id: "plain", version: 1, name: "Plain", detect: {shebangs: ["plain"]}}\n'
TWO_DOCS = "\n".join(emit(PLAIN)) + '\n---\nid: "plain"\n'
NOT_UTF8 = b'flavour = "\xff{{ flavour }}"\n'
BETA_REPEATS = [("beta.setting", 0, "section", "tool.alpha"), ("beta.param", "beta_root", "default", ".")]
BETA_REPEATS.append(("beta.file", "settings/pyproject.toml", "[tool.alpha]\n"))

PASSING = [
    ("the valid fixtures", []),
    (
        "integer 0",
        [("add", "parameters", RETRIES), ("text", "settings/pyproject.toml", "\n\n", "\nr = {{ retries }}\n\n")],
    ),
    ("a single-quoted pattern with \\d", ("yaml", 'pattern: "[a-z]+"', "pattern: '[a-z]+|\\d+'")),
    (
        "${{ secrets.TOKEN }}, {{- x }}, {{ Name }}",
        ("text", "conventions.md", "Say", "${{ secrets.TOKEN }} {{- x }} {{ Name }}"),
    ),
    ("{{name}} without spaces", ("text", "conventions.md", "{{ greeting }}", "{{greeting}}")),
    ("an integer at the 32-bit limit", ("top", "version", 2147483647)),
    ("one leading comment line", [HEAD1, ("yaml", DESC, '"A, <<: b"')]),
    ("two leading comment lines and a blank", [HEAD2, ("yaml", DESC, '"A, \\"<<: b\\""')]),
]
# Edge cases the valid fixtures hold: (name, pack, file, text).
BAKED = [
    ("a subdirectory under settings/", "alpha", "settings/editor/alpha.toml", "flavour"),
    ("a <section>.<rest> fragment header", "alpha", "settings/pyproject.toml", "[tool.alpha.extra]"),
    ("a derived pack on a valid base", "beta", "conventions.md", "# Beta"),
    ("a '.' default dropped from a target (see render)", "alpha", "conventions.md", "# Alpha"),
]
FAILING = [
    # layout
    ("pack.yaml missing", ("file", "pack.yaml", None), "layout-files"),
    ("an extra file", ("file", "notes.txt", "x\n"), "layout-files"),
    ("an extra directory", ("file", "docs/x.md", "x\n"), "layout-files"),
    ("a symlink under settings/", ("file", "settings/link.toml", ("link", "editor/alpha.toml")), "layout-files"),
    ("an empty directory under settings/", ("file", "settings/empty", DIR), "layout-files"),
    (
        "settings/, no entries",
        ("plain.file", "settings/x.toml", "x\n"),
        "plain:layout-settings-dir plain:layout-unreferenced",
    ),
    ("an empty settings/ without entries", ("plain.file", "settings", DIR), "plain:layout-settings-dir"),
    ("entries without settings/", ("beta.file", PY, None), "beta:layout-settings-dir beta:path-template"),
    ("an unreferenced settings file", ("file", "settings/extra.toml", "x\n"), "layout-unreferenced"),
    ("a BOM", ("text", "conventions.md", "#", "\ufeff#"), "encoding"),
    ("a CR byte", ("text", "conventions.md", "\n\n", "\r\n\r\n"), "encoding"),
    ("not UTF-8", ("file", "settings/editor/alpha.toml", NOT_UTF8), "encoding"),
    ("a BOM in pack.yaml", ("yaml", 'id: "alpha"', '\ufeffid: "alpha"'), "encoding"),
    ("no title", ("text", "conventions.md", "# Alpha", "Alpha"), "conventions-title"),
    ("a copy header", ("text", "conventions.md", "# ", "<!-- mintswarm-pack: alpha 1 -->\n# "), "conventions-title"),
    # YAML
    ("unparsable YAML", ("yaml", 'name: "Alpha"', 'name: ["Alpha"'), "yaml-parse"),
    ("a list at the top level", ("plain.file", "pack.yaml", '- "plain"\n'), "plain:yaml-parse"),
    ("an empty pack.yaml", ("plain.file", "pack.yaml", ""), "plain:yaml-parse"),
    ("a duplicate key", ("yaml", 'name: "Alpha"', 'name: "Alpha"\nname: "Alpha"'), "yaml-duplicate-key"),
    ("a nested repeat", ("yaml", '  kind: "lint"', '  kind: "lint"\n    kind: "lint"'), "yaml-duplicate-key"),
    ("a --- line", ("yaml", 'id: "alpha"', '---\nid: "alpha"'), "yaml-marker"),
    ("a ... line", ("yaml", "timeout: 600\n", "timeout: 600\n...\n"), "yaml-marker"),
    ("two documents", ("plain.file", "pack.yaml", TWO_DOCS), "plain:yaml-marker"),
    ("content on a marker line", ("plain.file", "pack.yaml", ON_MARKER), "plain:yaml-marker plain:yaml-parse"),
    ("an escape after a comment line", [HEAD1, ("yaml", DESC, '"A\\tb"')], "yaml-escape"),
    ("a two-line string after two comment lines", [HEAD2, ("yaml", DESC, '"A\n  b"')], "yaml-multiline-string"),
    ("anchor, alias", [("yaml", 'name: "Alpha"', 'name: &n "Alpha"'), ("yaml", DESC, "*n")], "yaml-anchor-alias"),
    ("a merge key", ("yaml", "detect:\n", "detect:\n  <<: {}\n"), "yaml-merge-key"),
    (
        "a flow merge key",
        ("plain.yaml", 'detect:\n  shebangs: ["plain"]', 'detect: {<<: {}, shebangs: ["plain"]}'),
        "plain:yaml-merge-key",
    ),
    ("a !!int tag", ("yaml", "version: 1", "version: !!int 1"), "yaml-tag"),
    ("a !!str tag", ("yaml", 'name: "Alpha"', 'name: !!str "Alpha"'), "yaml-tag"),
    ("a custom tag", ("yaml", 'name: "Alpha"', 'name: !x "Alpha"'), "yaml-tag"),
    ("an unquoted string", ("yaml", 'name: "Alpha"', "name: Alpha"), "yaml-unquoted-string"),
    ("an unquoted list item", ("yaml", '"alpha"]', "alpha]"), "yaml-unquoted-string"),
    ("a timestamp", ("yaml", DESC, "2026-11-22"), "yaml-unquoted-string"),
    ("a literal block scalar", ("yaml", DESC, "|\n  A fixture."), "yaml-block-scalar"),
    ("a folded block scalar", ("yaml", DESC, ">-\n  A fixture."), "yaml-block-scalar"),
    ("a string over two lines", ("yaml", DESC, '"A fixture\n  base pack."'), "yaml-multiline-string"),
    ("a single-quoted string over two lines", ("yaml", DESC, "'A\n  b'"), "yaml-multiline-string"),
    ("a \\t escape", ("yaml", DESC, '"A fixture\\tbase pack."'), "yaml-escape"),
    ("a \\u escape", ("yaml", DESC, '"A fixture \\u0041"'), "yaml-escape"),
    ("a leading zero", ("yaml", "version: 1", "version: 01"), "yaml-integer"),
    ("-0", ("yaml", "default: 100", "default: -0"), "yaml-integer param-choices"),
    ("a + sign", ("yaml", "version: 1", "version: +1"), "yaml-integer"),
    ("a hex integer", ("yaml", "version: 1", "version: 0x1"), "yaml-integer"),
    ("an integer over 2147483647", ("top", "version", 2147483648), "yaml-integer"),
    ("True", ("yaml", "default: true", "default: True"), "yaml-boolean"),
    ("a float", ("yaml", "version: 1", "version: 1.5"), "yaml-float-null field-type"),
    ("a null", ("yaml", DESC, "null"), "yaml-float-null field-type"),
    ("an exponent", ("yaml", "version: 1", "version: 1e3"), "yaml-float-null"),
    # fields
    ("an unknown top-level field", ("top", "colour", "blue"), "field-unknown"),
    ("an unknown gate field", ("gate", 0, "shell", "bash"), "field-unknown"),
    ("name missing", ("top", "name", DROP), "field-missing"),
    ("a parameter description missing", ("param", "greeting", "description", DROP), "field-missing"),
    ("detect missing", ("plain.top", "detect", DROP), "plain:field-missing"),
    ("name an integer", ("top", "name", 3), "field-type"),
    ("version a boolean", ("top", "version", True), "field-type"),
    ("optional a string", ("setting", 1, "optional", "yes"), "field-type"),
    ("detect files a string", ("detect", "files", "**/*.alp"), "field-type"),
    ("a parameter that is a string", ("plain.top", "parameters", ["x"]), "plain:field-type"),
    ("a parameter type outside the enum", ("param", "greeting", "type", "text"), "field-enum"),
    ("a settings kind outside the enum", ("setting", 0, "kind", "beautifier"), "field-enum"),
    ("a settings mode outside the enum", ("setting", 1, "mode", "partial"), "field-enum"),
    ("a gate kind outside the enum", ("gate", 0, "kind", "style"), "field-enum"),
    ("an id other than the directory", ("plain.top", "id", "other"), "plain:id"),
    ("an id with capitals", ("plain.top", "id", "Plain"), "plain:id"),
    ("version 0", ("top", "version", 0), "version-min"),
    ("detect empty", ("top", "detect", {}), "detect-empty"),
    ("detect lists empty", ("top", "detect", {"files": [], "shebangs": []}), "detect-empty"),
    # parameters
    (
        "name Greeting",
        [("param", "greeting", "name", "Greeting"), ("text", "conventions.md", "{{ greeting }}", "")],
        "param-name",
    ),
    ("a parameter name repeated", ("add", "parameters", "other"), "param-unique"),
    ("a default of another type", ("param", "line_length", "default", "100"), "param-default"),
    ("a boolean default 1", ("param", "strict", "default", 1), "param-default"),
    ("choices on a boolean", ("param", "strict", "choices", [True, False]), "param-choices"),
    ("choices empty", ("param", "line_length", "choices", []), "param-choices"),
    ("a choice repeated", ("param", "line_length", "choices", [80, 80, 100]), "param-choices"),
    ("choices without the default", ("param", "line_length", "choices", [80, 120]), "param-choices"),
    ("a choice of another type", ("param", "line_length", "choices", [80, "100", 100]), "param-choices"),
    ("a pattern on an integer", ("param", "line_length", "pattern", "[0-9]+"), "param-pattern"),
    ("a pattern that does not compile", ("param", "other", "pattern", "[a-"), "param-pattern"),
    ("a choice not matching the pattern", ("param", "flavour", "pattern", "[a-z]{4}"), "value-string"),
    ("a default not matching the pattern", ("param", "other", "default", "World"), "value-string"),
    ("a default with a Cf character", ("param", "greeting", "default", "hi\u200b"), "value-string"),
    ("a default with U+2028", ("param", "greeting", "default", "hi\u2028there"), "value-string yaml-multiline-string"),
    ("a path value with a space", ("param", "package_root", "default", "my dir"), "value-string"),
    ("a path value starting with -", ("param", "package_root", "default", "-x"), "value-string"),
    # settings
    ("a fragment without a section", ("setting", 0, "section", DROP), "section"),
    ("a file with a section", ("setting", 1, "section", "tool.alpha"), "section"),
    ("a placeholder inside a segment", ("setting", 0, "target", R + "pyproject.toml"), "path-placeholder-segment"),
    ("a last-segment placeholder", ("setting", 1, "alternatives", [R]), "path-placeholder-segment path-alternatives"),
    ("a placeholder after text", ("setting", 1, "alternatives", ["x" + R + "/setup.cfg"]), "path-placeholder-segment"),
    ("a target with ..", ("setting", 0, "target", "../pyproject.toml"), "path-target"),
    ("a target under .GIT", ("setting", 1, "target", R + "/.GIT/alpha.toml"), "path-target"),
    ("an absolute target", ("setting", 1, "target", "/{{ package_root }}/alpha.toml"), "path-target"),
    ("a target with an empty segment", ("setting", 1, "target", R + "//alpha.toml"), "path-target"),
    ("a target with a backslash", ("setting", 1, "target", R + "/a\\b.toml"), "path-target"),
    ("a target with a glob", ("setting", 1, "target", R + "/*.toml"), "path-target"),
    ("a target of only .", ("setting", 1, "target", R + "/."), "path-target"),
    ("an alternative with ..", ("setting", 1, "alternatives", [R + "/../setup.cfg"]), "path-alternatives"),
    ("template ./", ("setting", 1, "template", "settings/./editor/alpha.toml"), "path-template layout-unreferenced"),
    ("a template outside settings/", ("setting", 1, "template", "conventions.md"), "path-template layout-unreferenced"),
    (
        "template of no file",
        ("setting", 1, "template", "settings/editor/beta.toml"),
        "path-template layout-unreferenced",
    ),
    ("template glob", ("setting", 1, "template", "settings/editor/*.toml"), "path-template layout-unreferenced"),
    ("a fragment not starting with a header", ("text", PY, "[tool.alpha]\n", "# x\n[tool.alpha]\n"), "fragment-header"),
    ("a fragment header of another section", ("header", "[tool.other]"), "fragment-header"),
    ("a fragment header sharing a prefix", ("header", "[tool.alphabet]"), "fragment-header"),
    ("a fragment header with an empty rest", ("header", "[tool.alpha.]"), "fragment-header"),
    ("a fragment header repeated", ("header", "[ tool.alpha ] # x"), "fragment-header"),
    ("a [[...]] header", ("header", "[[tool.alpha.extra]]"), "fragment-header"),
    ("a file entry sharing a target", ("setting", 1, "target", R + "/pyproject.toml"), "settings-unique"),
    ("a target and section repeated", ("add", "settings", 0), "settings-unique"),
    (
        "repeated once rendered",
        [("add", "settings", 0), ("setting", 2, "target", "./pyproject.toml")],
        "settings-unique",
    ),
    ("a derived entry repeating the base's", BETA_REPEATS, "beta:settings-unique"),
    # gates
    ("a gate name without the id", ("gate", 0, "name", "format"), "gate-name"),
    ("a gate name of another pack", ("gate", 0, "name", "beta-format"), "gate-name"),
    ("a gate name repeated", ("gate", 0, "name", "alpha-lint"), "gate-unique"),
    ("a timeout of 0", ("gate", 1, "timeout", 0), "gate-timeout"),
    ("a command shlex refuses", ("gate", 0, "command", "alpha 'fmt {{ package_root }}"), "gate-command"),
    ("a command of no word", ("add", "gates", NO_WORD), "gate-command"),
    # templates
    ("an undeclared placeholder", ("text", "conventions.md", "Say", "{{ nobody }} Say"), "placeholder-undeclared"),
    ("${{ false }}", ("text", "conventions.md", "Say", "${{ false }} Say"), "placeholder-undeclared"),
    ("an undeclared placeholder in a command", ("gate", 0, "command", "alpha {{ nobody }}"), "placeholder-undeclared"),
    (
        "a base parameter used",
        ("beta.gate", 0, "command", "beta {{ beta_root }} {{ strict }}"),
        "beta:placeholder-undeclared",
    ),
    ("an unused parameter", ("add", "parameters", SPARE), "placeholder-unused"),
    ("a placeholder in description", ("top", "description", "{{ greeting }}"), "placeholder-not-allowed"),
    ("a placeholder in detect", ("detect", "files", [R + "/*.alp"]), "placeholder-not-allowed"),
    ("section placeholder", ("setting", 0, "section", "{{ greeting }}"), "placeholder-not-allowed fragment-header"),
    ("a placeholder in a default", ("param", "greeting", "default", "{{ other }}"), "placeholder-not-allowed"),
    # base packs
    ("a base naming the pack itself", ("plain.top", "base", "plain"), "plain:base"),
    ("a base naming no pack", ("beta.top", "base", "gamma"), "beta:base"),
    ("a base with a base", ("plain.top", "base", "beta"), "plain:base"),
]


def param(f: Fixture, name: str, pack: str = "alpha") -> dict:
    return next(p for p in f.data(pack)["parameters"] if p["name"] == name)


def apply(f: Fixture, op: tuple) -> None:
    pack, _, kind = op[0].rpartition(".")
    pack, args = pack or "alpha", op[1:]
    data, files = f.data(pack), f.files(pack)
    if kind == "header":
        kind, args = "text", (PY, "[tool.alpha.extra]", args[0])
    if kind == "yaml":
        f.yaml(pack, *args)
    elif kind == "file":
        files[args[0]] = args[1]
    elif kind == "text":
        rel, old, new = args
        assert old in files[rel], (rel, old)
        files[rel] = files[rel].replace(old, new, 1)
    elif kind == "add":
        key, entry = args
        if type(entry) is str:
            entry = param(f, entry, pack)
        elif type(entry) is int:
            entry = data[key][entry]
        data[key].append(copy.deepcopy(entry))
    else:
        if kind == "top":
            container = data
        elif kind == "detect":
            container = data["detect"]
        else:
            container = param(f, args[0], pack) if kind == "param" else data[kind + "s"][args[0]]
            args = args[1:]
        key, value = args
        if value is DROP:
            del container[key]
        else:
            container[key] = value


def run_row(case: unittest.TestCase, ops, expected: str, name: str) -> None:
    fixture = Fixture()
    for op in [ops] if type(ops) is tuple else ops:
        apply(fixture, op)
    proc = check_fixture(fixture, name)
    pairs = {word if ":" in word else f"alpha:{word}" for word in expected.split()}
    pairs = {pair.replace(":", ": ") for pair in pairs}
    case.assertEqual(proc.returncode, 1 if pairs else 0, proc.stdout + proc.stderr)
    case.assertEqual(found(proc.stdout), pairs, proc.stdout + proc.stderr)
    case.assertRegex(proc.stdout.splitlines()[-1], r"^check: 3 packs, \d+ violations$")


def docstring_rules() -> list:
    doc = ast.get_docstring(ast.parse(SCRIPT.read_text(encoding="utf-8")))
    block = doc.split("Rules, in README order:\n", 1)[1].split("\n\n", 1)[0]
    return re.findall(r"^  ([a-z][a-z-]*[a-z])  ", block, re.MULTILINE)


class Rows(unittest.TestCase):
    pass


def add_row(prefix: str, number: int, name: str, ops, expected: str) -> None:
    def test(self):
        run_row(self, ops, expected, name)

    test.__doc__ = f"{prefix}: {name}" + (f" -> {expected}" if expected else "")
    setattr(Rows, f"test_{prefix}_{number:03d}", test)


for _n, (_name, _ops) in enumerate(PASSING):
    add_row("pass", _n, _name, _ops, "")
for _n, (_name, _ops, _expected) in enumerate(FAILING):
    add_row("fail", _n, _name, _ops, _expected)


class Coverage(unittest.TestCase):
    def test_rules(self):
        """every rule in pack.py's docstring has a failing row"""
        rules = docstring_rules()
        self.assertEqual(len(rules), 47)
        hit = {word.split(":")[-1] for _, _, expected in FAILING for word in expected.split()} | {"version-bump"}
        self.assertEqual(sorted(set(rules) - hit), [])
        self.assertEqual(sorted(hit - set(rules)), [])

    def test_positions(self):
        """line:column after leading comments; one settings-unique line for two file entries"""
        fixture = Fixture()
        for op in (HEAD2, ("yaml", DESC, '"A\\tb"'), ("setting", 0, "mode", "file"), ("setting", 0, "section", DROP)):
            apply(fixture, op)
        apply(fixture, ("text", PY, "[tool.alpha.extra]", "[tool.alpha_extra]"))
        apply(fixture, ("setting", 1, "target", R + "/pyproject.toml"))
        proc = check_fixture(fixture, "positions")
        text = next(Path(TMP).glob("positions-*/packs/alpha/pack.yaml")).read_text().split("\n")
        line = next(n for n, t in enumerate(text, 1) if t.startswith("description:"))
        self.assertIn(f"alpha: yaml-escape: pack.yaml:{line}:14: ", proc.stdout)
        self.assertEqual(proc.stdout.count("alpha: settings-unique: "), 1, proc.stdout)

    def test_baked(self):
        """the valid fixtures hold the baked-in edge cases"""
        for name, pack, rel, needle in BAKED:
            self.assertIn(needle, (ALPHA_FILES if pack == "alpha" else BETA_FILES)[rel], name)
        self.assertEqual((BETA["base"], param(Fixture(), "package_root")["default"]), ("alpha", "."))


# --- the README's sample ---------------------------------------------------


def readme_example() -> tuple:
    """(blocks by path, prose) of the README's Example section."""
    section = README.read_text(encoding="utf-8").split("\n## Example\n", 1)[1].split("\n## ", 1)[0]
    lines, blocks, prose, last = section.split("\n"), {}, [], None
    i = 0
    while i < len(lines):
        fence = re.fullmatch(r"```([a-z]+)", lines[i])
        if fence:
            end = lines.index("```", i + 1)
            paths = re.findall(r"`((?:defaults/packs/sample/|docs/conventions/sample\.md)[^`]*)`", last or "")
            if not paths:
                raise AssertionError(f"no path before the block at Example line {i + 1}")
            blocks[paths[-1]] = "\n".join(lines[i + 1 : end]) + "\n"
            i, last = end + 1, None
            continue
        if lines[i].strip():
            last = lines[i]
            prose.append(lines[i])
        i += 1
    return blocks, "\n".join(prose)


class Sample(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.blocks, cls.prose = readme_example()
        cls.root = fresh("sample") / "packs"
        pack = cls.root / "sample"
        (pack / "settings").mkdir(parents=True)
        prefix = "defaults/packs/sample/"
        for rel in ("pack.yaml", "settings/pyproject.toml", "conventions.md"):
            (pack / rel).write_bytes(cls.blocks[prefix + rel].encode("utf-8"))
        (pack / "settings" / "sample-lint.toml").write_bytes(b"# Sample lint settings\n")

    def test_blocks(self):
        """README sample: the Example holds pack.yaml, the fragment, conventions.md and the copy"""
        names = {
            "defaults/packs/sample/pack.yaml",
            "defaults/packs/sample/settings/pyproject.toml",
            "defaults/packs/sample/conventions.md",
            "docs/conventions/sample.md",
        }
        self.assertEqual(set(self.blocks), names)

    def test_check(self):
        """README sample: check exits 0"""
        proc = run_pack(["check", "--root", str(self.root)])
        self.assertEqual(
            (proc.returncode, proc.stdout.splitlines()[-1]), (0, "check: 1 packs, 0 violations"), proc.stdout
        )

    def test_render(self):
        """README sample: render with defaults gives what the README states"""
        proc = run_pack(["render", "sample", "--root", str(self.root)])
        self.assertEqual(proc.returncode, 0, proc.stderr)
        out = json.loads(proc.stdout)
        self.assertEqual([s["target"] for s in out["settings"]], ["pyproject.toml", ".sample-lint.toml"])
        self.assertEqual(out["settings"][1]["alternatives"], ["setup.cfg"])
        gates = [{"name": "sample-format", "command": "sample fmt --check ."}]
        gates.append({"name": "sample-lint", "command": "sample lint .", "timeout": 600})
        self.assertEqual(out["gates"], gates)
        self.assertTrue(out["conventions"].startswith("# Sample conventions\n"))
        self.assertEqual(out["settings"][0]["text"], '[tool.sample]\nline-length = 120\ntarget-version = "3.9"\n')
        for literal in ("pyproject.toml", "sample-format", "sample fmt --check .", "sample-lint", "sample lint ."):
            self.assertIn(f"`{literal}`", self.prose)
        self.assertIn("a timeout of 600", self.prose)
        header = f"<!-- mintswarm-pack: {out['pack']} {out['version']} -->"
        first = out["conventions"].split("\n", 1)[0]
        self.assertEqual(f"{header}\n{first}\n", self.blocks["docs/conventions/sample.md"])


# --- render ----------------------------------------------------------------


class Render(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.root = Fixture().write(fresh("render") / "packs")

    def render(self, *args: str) -> subprocess.CompletedProcess:
        return run_pack(["render", args[0], "--root", str(self.root), *args[1:]])

    def test_defaults(self):
        """render: alpha with defaults, keys in order, '.' segments dropped"""
        proc = self.render("alpha")
        self.assertEqual(proc.returncode, 0, proc.stderr)
        out = json.loads(proc.stdout)
        self.assertEqual(list(out), ["pack", "version", "values", "conventions", "settings", "gates"])
        self.assertEqual((out["pack"], out["version"]), ("alpha", 1))
        values = {"package_root": ".", "line_length": 100, "strict": True}
        values.update({"greeting": "hello", "other": "world", "flavour": "mild"})
        self.assertEqual(list(out["values"].items()), list(values.items()))
        self.assertEqual(out["conventions"], "# Alpha conventions\n\nSay hello to world, mild.\n")
        keys = ["template", "target", "kind", "mode", "section", "optional", "alternatives", "text"]
        self.assertEqual([list(s) for s in out["settings"]], [keys, keys])
        first, second = out["settings"]
        self.assertEqual(
            (first["target"], first["mode"], first["section"], first["optional"]),
            ("pyproject.toml", "fragment", "tool.alpha", False),
        )
        self.assertEqual(first["text"], "[tool.alpha]\nline-length = 100\n\n[tool.alpha.extra]\nstrict = true\n")
        self.assertEqual((second["target"], second["mode"], second["section"]), (".alpha.toml", "file", None))
        self.assertEqual((second["alternatives"], second["text"]), (["setup.cfg"], 'flavour = "mild"\n'))
        gates = [{"name": "alpha-format", "command": "alpha fmt --check ."}]
        gates.append({"name": "alpha-lint", "command": "alpha lint --strict=true .", "timeout": 600})
        self.assertEqual(out["gates"], gates)

    def test_values(self):
        """render: given values change targets, texts and commands"""
        proc = self.render("alpha", "package_root=src/app", "line_length=120", "strict=false", "flavour=hot")
        self.assertEqual(proc.returncode, 0, proc.stderr)
        out = json.loads(proc.stdout)
        self.assertEqual([s["target"] for s in out["settings"]], ["src/app/pyproject.toml", "src/app/.alpha.toml"])
        self.assertIn("line-length = 120\n", out["settings"][0]["text"])
        self.assertEqual(out["gates"][1]["command"], "alpha lint --strict=false src/app")
        self.assertEqual(out["values"]["strict"], False)

    def test_single_pass(self):
        """render: a given value {{ other }} renders verbatim, never rescanned"""
        proc = self.render("alpha", "greeting={{ other }}")
        self.assertEqual(proc.returncode, 0, proc.stderr)
        self.assertIn("Say {{ other }} to world,", json.loads(proc.stdout)["conventions"])

    def test_derived(self):
        """render: a derived pack renders alone"""
        proc = self.render("beta", "beta_root=site")
        self.assertEqual(proc.returncode, 0, proc.stderr)
        out = json.loads(proc.stdout)
        self.assertEqual((list(out["values"]), out["settings"][0]["target"]), (["beta_root"], "site/pyproject.toml"))

    def test_refused_values(self):
        """render: values breaking a value or path rule exit 1, stdout empty"""
        cases = [
            (["line_length=90"], "param-choices"),
            (["other=World"], "value-string"),
            (["package_root=.."], "path-target"),
            (["package_root=.."], "path-alternatives"),
            (["package_root=-x"], "value-string"),
            (["package_root=a b"], "value-string"),
            (["greeting=a\u2028b"], "value-string"),
        ]
        for args, rule in cases:
            proc = self.render("alpha", *args)
            self.assertEqual((proc.returncode, proc.stdout), (1, ""), args)
            self.assertIn(f"alpha: {rule}: ", proc.stderr, args)

    def test_broken_pack(self):
        """render: a pack with violations exits 1, stdout empty"""
        fixture = Fixture()
        fixture.data("alpha")["version"] = 0
        proc = run_pack(["render", "alpha", "--root", str(fixture.write(fresh("broken") / "packs"))])
        self.assertEqual((proc.returncode, proc.stdout), (1, ""))
        self.assertIn("alpha: version-min: ", proc.stderr)

    def test_unusable(self):
        """render: unusable arguments exit 2, stdout empty"""
        cases = [
            ["gamma"],
            ["alpha", "strict"],
            ["alpha", "nobody=1"],
            ["alpha", "strict=true", "strict=false"],
            ["alpha", "line_length=1.5"],
            ["alpha", "line_length=010"],
            ["alpha", "line_length=-0"],
            ["alpha", "line_length=2147483648"],
            ["alpha", "strict=yes"],
            ["alpha", "strict=True"],
            ["..", "strict=true"],
        ]
        for args in cases:
            proc = self.render(*args)
            self.assertEqual((proc.returncode, proc.stdout), (2, ""), args)
            self.assertTrue(proc.stderr.startswith("pack.py: "), args)
        for args in (["render"], ["render", "--root", str(self.root)], ["render", "alpha", "--root"]):
            proc = run_pack(args)
            self.assertEqual((proc.returncode, proc.stdout), (2, ""), args)


# --- unusable input --------------------------------------------------------


class Unusable(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.root = Fixture().write(fresh("unusable") / "packs")

    def exit_2(self, args: list, env=None, needle: str = "pack.py: ") -> None:
        proc = run_pack(args, env=env)
        self.assertEqual((proc.returncode, proc.stdout), (2, ""), args)
        self.assertIn(needle, proc.stderr, args)

    def test_arguments(self):
        """unusable: bad arguments exit 2"""
        root = str(self.root)
        for args in (
            [],
            ["lint"],
            ["check", "--bogus"],
            ["check", "--root"],
            ["check", "--root", root, "--root", root],
        ):
            self.exit_2(args)
        self.exit_2(["check", "--base", "-x", "--root", root])

    def test_missing_root(self):
        """unusable: a missing root exits 2"""
        self.exit_2(["check", "--root", str(self.root / "nowhere")], needle="is not a directory")

    def test_no_yq(self):
        """unusable: yq missing from PATH exits 2"""
        empty = fresh("bin")
        self.exit_2(["check", "--root", str(self.root)], dict(ENV, PATH=str(empty)), "cannot run yq")

    def test_fake_yq(self):
        """unusable: a yq answering the probe wrongly exits 2"""
        bin_dir = fresh("fakebin")
        fake = bin_dir / "yq"
        fake.write_text('#!/bin/sh\ncat >/dev/null\necho \'{"nodes": [], "paths": [], "keys": [[]]}\'\n')
        fake.chmod(0o755)
        self.exit_2(["check", "--root", str(self.root)], dict(ENV, PATH=str(bin_dir)), "yq probe failed")
        self.exit_2(["render", "alpha", "--root", str(self.root)], dict(ENV, PATH=str(bin_dir)), "yq probe failed")

    def test_base_outside_git(self):
        """unusable: --base outside a git work tree exits 2"""
        env = dict(ENV, GIT_CEILING_DIRECTORIES=os.path.realpath(Path(TMP).parent))
        self.exit_2(["check", "--root", str(self.root), "--base", "master"], env, "git work tree")

    def test_empty_root(self):
        """a root with no pack directories exits 0, other entries ignored"""
        root = fresh("empty")
        (root / "README.md").write_text("# Packs\n")
        proc = run_pack(["check", "--root", str(root)])
        self.assertEqual(proc.returncode, 0, proc.stderr)
        self.assertEqual(proc.stdout, "check: version-bump not evaluated (no --base)\ncheck: 0 packs, 0 violations\n")

    def test_symlinked_pack(self):
        """a symlink to a directory in the root is a violation"""
        root = fresh("linked")
        (root / "real").mkdir()
        (root / "link").symlink_to("real")
        proc = run_pack(["check", "--root", str(root)])
        self.assertEqual(proc.returncode, 1, proc.stdout)
        self.assertEqual(found(proc.stdout), {"link: layout-files", "real: layout-files"})


# --- the version rule ------------------------------------------------------


def plain_pack(pack_id: str, version: int, settings: bool = False) -> dict:
    data = dict(PLAIN, id=pack_id, version=version)
    if settings:
        data["settings"] = [{"template": "settings/a.toml", "target": "a.toml", "kind": "editor"}]
    return {"pack.yaml": "\n".join(emit(data)) + "\n", "conventions.md": f"# {pack_id}\n"}


class Versions(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.repo = fresh("git")
        home = fresh("home")
        cls.env = {k: v for k, v in ENV.items() if not k.startswith("GIT_")}
        cls.env.update({"HOME": str(home), "GIT_CONFIG_NOSYSTEM": "1", "GIT_CONFIG_GLOBAL": os.devnull})
        cls.git("init", "-q", "-b", "master")
        packs = {f"p-{n}": plain_pack(f"p-{n}", 1) for n in ("bump", "same", "two", "keep")}
        packs["p-untracked"] = plain_pack("p-untracked", 1, settings=True)
        cls.write(packs)
        cls.git("add", "packs")
        cls.commit("base packs")
        cls.git("checkout", "-q", "-b", "work")
        cls.write(
            {
                "p-bump": plain_pack("p-bump", 2),
                "p-same": plain_pack("p-same", 1),
                "p-two": plain_pack("p-two", 3),
                "n-one": plain_pack("n-one", 1),
                "n-two": plain_pack("n-two", 2),
            }
        )
        for name in ("p-bump", "p-same", "p-two"):
            path = cls.repo / "packs" / name / "conventions.md"
            path.write_text(path.read_text() + "\nChanged.\n")
        cls.git("add", "packs/p-bump", "packs/p-same", "packs/p-two", "packs/n-one")
        cls.commit("changes")
        cls.write({"n-late": plain_pack("n-late", 2), "n-late-one": plain_pack("n-late-one", 1)})
        (cls.repo / "packs" / "p-untracked" / "settings").mkdir()
        (cls.repo / "packs" / "p-untracked" / "settings" / "a.toml").write_text("x = 1\n")

    @classmethod
    def git(cls, *args: str) -> None:
        subprocess.run(["git", *args], cwd=cls.repo, env=cls.env, check=True, capture_output=True)

    @classmethod
    def commit(cls, message: str) -> None:
        cls.git(
            "-c", "user.name=pack selftest", "-c", "user.email=selftest@example.invalid", "commit", "-q", "-m", message
        )

    @classmethod
    def write(cls, packs: dict) -> None:
        for pack_id, files_ in packs.items():
            for rel, content in files_.items():
                path = cls.repo / "packs" / pack_id / rel
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text(content)

    def test_base(self):
        """version rule: bumps against the merge base, tracked and untracked"""
        proc = run_pack(["check", "--root", "packs", "--base", "master"], cwd=self.repo, env=self.env)
        self.assertEqual(proc.returncode, 1, proc.stdout + proc.stderr)
        expected = {f"{p}: version-bump" for p in ("p-same", "p-two", "n-two", "n-late", "p-untracked")}
        self.assertEqual(found(proc.stdout), expected, proc.stdout)
        self.assertEqual(proc.stdout.splitlines()[-1], "check: 9 packs, 5 violations")
        self.assertNotIn("not evaluated", proc.stdout)

    def test_no_base(self):
        """version rule: not evaluated without --base"""
        proc = run_pack(["check", "--root", "packs"], cwd=self.repo, env=self.env)
        self.assertEqual(proc.returncode, 0, proc.stdout + proc.stderr)
        self.assertIn("check: version-bump not evaluated (no --base)\n", proc.stdout)

    def test_unknown_base(self):
        """version rule: an unknown --base exits 2"""
        proc = run_pack(["check", "--root", "packs", "--base", "nosuch"], cwd=self.repo, env=self.env)
        self.assertEqual((proc.returncode, proc.stdout), (2, ""), proc.stderr)
        self.assertIn("does not resolve", proc.stderr)


class OneLine(unittest.TextTestResult):
    def getDescription(self, test) -> str:
        return test.shortDescription() or str(test)


def main() -> int:
    global TMP
    with tempfile.TemporaryDirectory(prefix="msw-packs-") as tmp:
        TMP = tmp
        runner = unittest.TextTestRunner(verbosity=2, resultclass=OneLine)
        program = unittest.main(exit=False, testRunner=runner)
        return 0 if program.result.wasSuccessful() else 1


if __name__ == "__main__":
    sys.exit(main())
