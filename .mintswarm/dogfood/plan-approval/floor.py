#!/usr/bin/env python3
"""Check a plan and its documents against the deterministic floor of plan approval.

    floor.py --plan PATH --component NAME --base REF
             [--issue-document PATH] [--analysis PATH]

Dogfooding stand-in for the control plane's plan-approval floor, the
deterministic rules that run on a plan before the approval decision. Every
hit sends the plan to a human; a clean verdict only means that no rule
written here objects. Paths are relative to the working directory, which is
the worktree root. --plan, --component and --base are required, and
--issue-document and --analysis optional; each is given at most once, as
`--name value`. Anything else, an empty value, a value that is an argument
name or a REF starting with `-` is unusable input. A document that is not
given is a hit of each rule that reads it. The first action is to delete any
stale .mintswarm/job/outputs/floor.json; when that fails, the input is
unusable.

Inputs:

- PATH, the plan, read as UTF-8. Its footprint is the `## Footprint` section
  (see the plan stage): a single "```json" block holding
  {"footprint": {"files", "new_files", "estimated_lines", "new_dependencies",
  "external_steps", "gate_changes", "overridden_assumptions"}}, keys in that
  order.
- NAME, the issue's component.
- REF, the base ref. The configuration is .mintswarm/config.yaml at the
  commit REF resolves to, read with `git show` and converted with
  `yq -p=yaml -o=json`. The worktree's config is never opened, so a change
  to it in the branch under review cannot loosen the floor. The history is
  the subject of every commit reachable from that commit, read with
  `git -c log.showSignature=false log -z --format=%s`, with replace objects
  and grafts off.
- The PATH given to --issue-document, the issue document (the job's
  .mintswarm/job/inputs/issue.md), and the PATH given to --analysis, the
  analysis, both read as UTF-8. Only the issue document's lines before its
  first `## Comments` heading are read, so headings in comments do nothing.

Sections of the documents and the plan are found outside fenced blocks: a
fence opens on a line whose first non-space characters are 3 or more
backticks or tildes, and closes on a line of only the same character, at
least as long. A section starts at a `## <title>` line (trailing whitespace
ignored) and runs to the next `## ` or `# ` heading. A section a rule reads
that is missing or appears more than once is a hit of that rule.

The configuration is validated; any failure is unusable input:
`components` maps names to mappings whose optional `paths` is a list of
globs; `shared` is absent or a list of globs; `pipeline.plan_approval` has
`mode` (`always`, `shadow`, `judged` or `never`), `limits` with exactly the
non-negative integers `files`, `lines`, `new_files` and `new_dependencies`,
and optionally `require_human`, with optional lists `components` (each
defined) and `paths` (globs); `gates.tests` is absent or a string;
`gates.run` is absent or a list of mappings with a `command` string that
shlex can split. NAME must be a defined component.

A glob matches the whole path: `**` matches anything including `/`, `**/`
zero or more directories, `*` anything within one segment, `?` one
character other than `/`; everything else is literal. The same semantics as
.mintswarm/dogfood/github/request-review.py.

Rules, evaluated in this order. The declared paths are `files` plus each
new dependency's `manifest`.

  footprint                  the footprint is missing, misplaced, not strict
                             JSON (no duplicate keys, NaN or Infinity), not
                             of the shape above (lists without null or
                             repeated values, `estimated_lines` an integer
                             of 0 or more, `files` not empty, every
                             `new_files` path in `files`)
                             or declares a path that is empty, absolute,
                             starts with `./`, ends with `/`, has an empty,
                             `.` or `..` segment, a control character, `\\`,
                             `*`, `?`, `[` or `]`, or is under .mintswarm/job/
  require-human-paths        a declared path matches require_human.paths,
                             compared case-insensitively
  require-human-components   NAME is in require_human.components, or a
                             declared path is in the paths of one of them,
                             compared case-insensitively
  component-scope            a declared path is in neither NAME's paths nor
                             `shared`
  no-component               a declared path is in no component's paths and
                             not in `shared`
  limit-files                len(files) is over limits.files
  limit-lines                estimated_lines is over limits.lines
  limit-new-files            len(new_files) is over limits.new_files
  limit-new-dependencies     len(new_dependencies) is over
                             limits.new_dependencies
  external-steps             external_steps is not empty
  gate-changes               gate_changes is not empty, or a declared path is
                             .mintswarm/config.yaml or a word of a gates.run
                             command (without a leading `./`), compared
                             case-insensitively
  tests-none                 gates.tests is `none` and a declared path is
                             outside the `docs` component's paths
  overridden-assumptions     overridden_assumptions is not empty
  blocking-questions         the `## Questions` section of the analysis or
                             of the plan has a line at column 0 starting
                             with `- **Blocking:**` (the reason names the
                             question), or any other line at column 0 that
                             does not start with
                             `- **Non-blocking (assumed):**`; blank and
                             indented lines are skipped; no --analysis
  acceptance-criteria        the criteria the first cells of the plan's
                             `## Verification` table quote are not the
                             issue's `## Acceptance criteria`, compared as
                             multisets after whitespace normalisation (each
                             missing and each extra criterion named); the
                             table does not start with a header and a `---`
                             row; the issue has no criteria; no
                             --issue-document
  first-of-kind              the issue document's header (its lines before
                             the first `## ` heading) has not exactly one
                             `- **Kind:** implementation` line and one
                             `- **Component:**` line equal to NAME, or no
                             subject in the history matches
                             `<type>(NAME)`, with an optional `!`, then
                             `: `, the scope compared exactly; no
                             --issue-document

Issue criteria are the column-0 items of `## Acceptance criteria` marked
`N.`, `N)`, `-`, `*` or `+`, with an optional `[ ]` or `[x]` after the
marker; following non-blank lines that are not items belong to the item,
and a blank line ends it. Table rows are split on `|` not preceded by `\\`,
and `\\|` then reads as `|`. Question and criterion text in reasons is put on
one line and cut to 200 characters.

When `footprint` is a hit, the rules from require-human-paths to
overridden-assumptions, which read the footprint, are not evaluated; the
three document rules always are.

The verdict is .mintswarm/job/outputs/floor.json, written to a temporary
file and renamed onto it, with these keys in this order: `tool`
(`mintswarm-plan-floor`), `version` (2), `plan` ({`path`, `sha256` of its
bytes}), `issue_document` and `analysis` (each {`path`, `sha256` of its
bytes}, or null when not given), `component`, `base` ({`ref` as given,
`commit`, the full hash}), `config` (`.mintswarm/config.yaml`), `mode` (as
read), `verdict` (`hit` or `clean`), `rules` (one {`rule`, `result`, `reasons`} per evaluated rule, in
the order above, `reasons` empty when clean) and `not_evaluated` (the
identifiers skipped, in order). One line per evaluated rule is printed, then
`floor: hit` or `floor: clean`.

Exits 0 when clean, 1 on any hit, 2 on unusable input: bad arguments, an
unresolvable base ref, no config at it, an invalid config, an unreadable
or non-UTF-8 plan, issue document or analysis, git or yq missing or
failing (reading the history too), a stale verdict that cannot be deleted,
or any unexpected error. Unusable input leaves no verdict file, unless a
stale one could not be deleted. The interpreter itself exits 1 when it
crashes outside main(), for example at import time, so exit 0 or 1 counts
only together with a verdict file whose `plan.sha256`, `base.commit` and
document bindings match the request; exit 2 never comes with a valid verdict. Standard library
only, apart from running git and yq.
"""

import hashlib
import json
import os
import re
import shlex
import subprocess
import sys
import tempfile
import unicodedata
from pathlib import Path

VERDICT = Path(".mintswarm/job/outputs/floor.json")
CONFIG = ".mintswarm/config.yaml"
TOOL = "mintswarm-plan-floor"
VERSION = 2
DOCS = "docs"
MODES = ("always", "shadow", "judged", "never")
LIMITS = ("files", "lines", "new_files", "new_dependencies")
FOOTPRINT_KEYS = (
    "files", "new_files", "estimated_lines", "new_dependencies",
    "external_steps", "gate_changes", "overridden_assumptions",
)
RULES = (
    "footprint",
    "require-human-paths",
    "require-human-components",
    "component-scope",
    "no-component",
    "limit-files",
    "limit-lines",
    "limit-new-files",
    "limit-new-dependencies",
    "external-steps",
    "gate-changes",
    "tests-none",
    "overridden-assumptions",
    "blocking-questions",
    "acceptance-criteria",
    "first-of-kind",
)
FOOTPRINT_RULES = RULES[1:13]  # the rules that read the footprint, skipped after a `footprint` hit
DOCUMENT_RULES = RULES[13:]
REQUIRED = ("--plan", "--component", "--base")
OPTIONAL = ("--issue-document", "--analysis")
ARGS = REQUIRED + OPTIONAL
USAGE = ("usage: floor.py --plan PATH --component NAME --base REF "
         "[--issue-document PATH] [--analysis PATH]")
FENCE = re.compile(r"^[ \t]*(?:`{3,}|~{3,})json", re.IGNORECASE)
REASON_LIMIT = 200
BLOCKING = "- **Blocking:**"
NON_BLOCKING = "- **Non-blocking (assumed):**"
KIND = "- **Kind:**"
COMPONENT = "- **Component:**"
ITEM = re.compile(r"(?:[0-9]+[.)]|[-*+])(?:[ \t]+|$)(.*)", re.DOTALL)
CHECKBOX = re.compile(r"\[[ x]\](?:[ \t]+|$)(.*)", re.DOTALL)
SEPARATOR_CELL = re.compile(r"[ \t]*:?-{3,}:?[ \t]*")
CELL_BAR = re.compile(r"(?<!\\)\|")


class InputError(Exception):
    """Unusable input: exit 2, no verdict."""


class FootprintError(Exception):
    """The footprint rule is a hit."""


class DocumentError(Exception):
    """A document rule is a hit; the message is its reason."""


# A copy of glob_to_regex in .mintswarm/dogfood/github/request-review.py;
# selftest.py checks that the two agree.
def glob_to_regex(pattern: str) -> re.Pattern:
    out = []
    i = 0
    while i < len(pattern):
        if pattern.startswith("**/", i):
            out.append("(?:.*/)?")
            i += 3
        elif pattern.startswith("**", i):
            out.append(".*")
            i += 2
        elif pattern[i] == "*":
            out.append("[^/]*")
            i += 1
        elif pattern[i] == "?":
            out.append("[^/]")
            i += 1
        else:
            out.append(re.escape(pattern[i]))
            i += 1
    return re.compile("".join(out), re.DOTALL)


def glob_ci(pattern: str) -> re.Pattern:
    return re.compile(glob_to_regex(pattern).pattern, re.DOTALL | re.IGNORECASE)


def matching(globs: list, path: str, ci: bool = False) -> list:
    compile_ = glob_ci if ci else glob_to_regex
    return [g for g in globs if compile_(g).fullmatch(path)]


# --- strict JSON ---------------------------------------------------------

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


def canonical(value) -> str:
    return json.dumps(value, sort_keys=True, ensure_ascii=False)


# --- arguments -----------------------------------------------------------

def parse_args(argv: list) -> dict:
    args = {}
    i = 0
    while i < len(argv):
        name = argv[i]
        if name not in ARGS:
            raise InputError(f"unknown argument {name!r}; {USAGE}")
        if name in args:
            raise InputError(f"{name} given twice; {USAGE}")
        if i + 1 >= len(argv):
            raise InputError(f"{name} needs a value; {USAGE}")
        value = argv[i + 1]
        if value == "" or value in ARGS:
            raise InputError(f"{name} needs a value; {USAGE}")
        args[name] = value
        i += 2
    missing = [name for name in REQUIRED if name not in args]
    if missing:
        raise InputError(f"missing {', '.join(missing)}; {USAGE}")
    if args["--base"].startswith("-"):
        raise InputError(f"--base must not start with '-': {args['--base']!r}")
    return {
        "plan": args["--plan"],
        "component": args["--component"],
        "base": args["--base"],
        "issue_document": args.get("--issue-document"),
        "analysis": args.get("--analysis"),
    }


# --- base-ref configuration ---------------------------------------------

def run(argv: list, data: bytes = None) -> bytes:
    try:
        proc = subprocess.run(argv, input=data, capture_output=True)
    except OSError as exc:
        raise InputError(f"cannot run {argv[0]}: {exc}") from exc
    if proc.returncode != 0:
        detail = proc.stderr.decode("utf-8", errors="replace").strip().splitlines()
        raise InputError(f"{' '.join(argv[:2])} failed: {detail[-1] if detail else f'exit {proc.returncode}'}")
    return proc.stdout


def resolve_base(ref: str) -> str:
    try:
        proc = subprocess.run(
            ["git", "rev-parse", "--verify", "--quiet", "--end-of-options", f"{ref}^{{commit}}"],
            capture_output=True,
        )
    except OSError as exc:
        raise InputError(f"cannot run git: {exc}") from exc
    commit = proc.stdout.decode("ascii", errors="replace").strip()
    if proc.returncode != 0 or not re.fullmatch(r"[0-9a-f]{40}|[0-9a-f]{64}", commit):
        raise InputError(f"base ref {ref!r} does not resolve to a commit")
    return commit


def load_config(commit: str) -> dict:
    try:
        proc = subprocess.run(["git", "show", f"{commit}:{CONFIG}"], capture_output=True)
    except OSError as exc:
        raise InputError(f"cannot run git: {exc}") from exc
    if proc.returncode != 0:
        raise InputError(f"no {CONFIG} at {commit}")
    converted = run(["yq", "-p=yaml", "-o=json", "."], proc.stdout)
    try:
        config = strict_json(converted.decode("utf-8"))
    except (UnicodeDecodeError, ValueError) as exc:
        raise InputError(f"{CONFIG} at {commit} does not convert to JSON: {exc}") from exc
    if not isinstance(config, dict):
        raise InputError(f"{CONFIG} at {commit} is not a mapping")
    return config


def string_list(value, where: str, optional: bool = True) -> list:
    if value is None and optional:
        return []
    if not isinstance(value, list) or not all(isinstance(v, str) for v in value):
        raise InputError(f"config: {where} must be a list of strings")
    return value


def validate_config(config: dict, component: str) -> dict:
    components = config.get("components")
    if not isinstance(components, dict):
        raise InputError("config: components must be a mapping")
    paths = {}
    for name, spec in components.items():
        if not isinstance(spec, dict):
            raise InputError(f"config: components.{name} must be a mapping")
        paths[name] = string_list(spec.get("paths"), f"components.{name}.paths")
    shared = string_list(config.get("shared"), "shared")

    pipeline = config.get("pipeline")
    if not isinstance(pipeline, dict) or "plan_approval" not in pipeline:
        raise InputError("config: pipeline.plan_approval is missing")
    approval = pipeline["plan_approval"]
    if not isinstance(approval, dict):
        raise InputError("config: pipeline.plan_approval must be a mapping")
    unknown = sorted(set(approval) - {"mode", "require_human", "limits"})
    if unknown:
        raise InputError(f"config: unknown keys in pipeline.plan_approval: {', '.join(unknown)}")
    mode = approval.get("mode")
    if mode not in MODES:
        raise InputError(f"config: pipeline.plan_approval.mode must be one of {', '.join(MODES)}, not {mode!r}")

    require = approval.get("require_human")
    if require is None:
        require = {}
    if not isinstance(require, dict):
        raise InputError("config: pipeline.plan_approval.require_human must be a mapping")
    unknown = sorted(set(require) - {"components", "paths"})
    if unknown:
        raise InputError(f"config: unknown keys in require_human: {', '.join(unknown)}")
    rh_components = string_list(require.get("components"), "require_human.components")
    rh_paths = string_list(require.get("paths"), "require_human.paths")
    undefined = [c for c in rh_components if c not in components]
    if undefined:
        raise InputError(f"config: require_human.components names undefined components: {', '.join(undefined)}")

    limits = approval.get("limits")
    if not isinstance(limits, dict) or set(limits) != set(LIMITS):
        raise InputError(f"config: pipeline.plan_approval.limits must have exactly {', '.join(LIMITS)}")
    for key in LIMITS:
        value = limits[key]
        if not isinstance(value, int) or isinstance(value, bool) or value < 0:
            raise InputError(f"config: limits.{key} must be a non-negative integer, not {value!r}")

    gates = config.get("gates")
    if gates is None:
        gates = {}
    if not isinstance(gates, dict):
        raise InputError("config: gates must be a mapping")
    tests = gates.get("tests")
    if tests is not None and not isinstance(tests, str):
        raise InputError("config: gates.tests must be a string")
    run_ = gates.get("run")
    if run_ is None:
        run_ = []
    if not isinstance(run_, list):
        raise InputError("config: gates.run must be a list")
    gate_words = []
    for index, gate in enumerate(run_):
        if not isinstance(gate, dict) or not isinstance(gate.get("command"), str):
            raise InputError(f"config: gates.run[{index}] must be a mapping with a command string")
        name = gate.get("name") if isinstance(gate.get("name"), str) else f"gates.run[{index}]"
        try:
            words = shlex.split(gate["command"])
        except ValueError as exc:
            raise InputError(f"config: gate {name}: command does not split: {exc}") from exc
        for word in words:
            while word.startswith("./"):
                word = word[2:]
            gate_words.append((name, word))

    if component not in components:
        raise InputError(f"--component {component!r} is not a component in the config")
    return {
        "paths": paths,
        "shared": shared,
        "mode": mode,
        "rh_components": rh_components,
        "rh_paths": rh_paths,
        "limits": limits,
        "tests": tests,
        "gate_words": gate_words,
    }


# --- footprint -----------------------------------------------------------

def read_plan(path: str) -> bytes:
    try:
        data = Path(path).read_bytes()
        data.decode("utf-8")
    except (OSError, UnicodeDecodeError) as exc:
        raise InputError(f"cannot read the plan {path}: {exc}") from exc
    return data


def is_heading(line: str) -> bool:
    return line.startswith("## ")


def locate_footprint(lines: list) -> str:
    sections = [i for i, line in enumerate(lines) if line == "## Footprint"]
    if not sections:
        raise FootprintError("no '## Footprint' section")
    if len(sections) > 1:
        raise FootprintError(f"{len(sections)} '## Footprint' sections")
    section = sections[0]
    before = [lines[i] for i in range(section) if is_heading(lines[i])]
    if not before or before[-1] != "## Steps":
        found = repr(before[-1]) if before else "nothing"
        raise FootprintError(f"'## Footprint' must follow '## Steps', it follows {found}")
    fences = [i for i, line in enumerate(lines) if FENCE.match(line)]
    if len(fences) != 1:
        raise FootprintError(f"{len(fences)} json fences in the plan, expected 1")
    start = fences[0]
    if lines[start] != "```json":
        raise FootprintError(f"line {start + 1}: the json fence must be exactly '```json'")
    previous = next((lines[i] for i in range(start - 1, -1, -1) if lines[i].strip()), None)
    if previous != "## Footprint":
        raise FootprintError("the json fence must come right after '## Footprint'")
    end = next((i for i in range(start + 1, len(lines)) if lines[i] == "```"), None)
    if end is None:
        raise FootprintError("the json fence is not closed")
    for i in range(end + 1, len(lines)):
        if is_heading(lines[i]):
            break
        if lines[i].strip():
            raise FootprintError(f"line {i + 1}: text after the footprint block")
    return "\n".join(lines[start + 1:end])


def check_path(path: str, where: str) -> None:
    def bad(why: str):
        raise FootprintError(f"{where}: {path!r} {why}")

    if path == "":
        bad("is empty")
    if any(unicodedata.category(c) == "Cc" for c in path):
        bad("has a control character")
    for c in "\\*?[]":
        if c in path:
            bad(f"has {c!r}")
    if path.startswith("/"):
        bad("is absolute")
    if path.startswith("./"):
        bad("starts with './'")
    if path.endswith("/"):
        bad("ends with '/'")
    if any(segment in ("", ".", "..") for segment in path.split("/")):
        bad("has an empty, '.' or '..' segment")
    lower = path.lower()
    if lower == ".mintswarm/job" or lower.startswith(".mintswarm/job/"):
        bad("is under .mintswarm/job/")


def check_list(value, key: str) -> list:
    if not isinstance(value, list):
        raise FootprintError(f"{key} must be a list")
    seen = set()
    for item in value:
        if item is None:
            raise FootprintError(f"{key} holds null")
        form = canonical(item)
        if form in seen:
            raise FootprintError(f"{key} holds {form} twice")
        seen.add(form)
    return value


def check_entries(value: list, key: str, fields: tuple) -> None:
    for item in value:
        if not isinstance(item, dict) or set(item) != set(fields) or len(item) != len(fields):
            raise FootprintError(f"each {key} entry must have exactly {', '.join(fields)}")
        for field in fields:
            if not isinstance(item[field], str):
                raise FootprintError(f"{key}.{field} must be a string")


def parse_footprint(text: str) -> dict:
    lines = [line[:-1] if line.endswith("\r") else line for line in text.split("\n")]
    block = locate_footprint(lines)
    try:
        document = strict_json(block)
    except ValueError as exc:
        raise FootprintError(f"the footprint is not valid JSON: {exc}") from exc
    if not isinstance(document, dict) or list(document) != ["footprint"]:
        raise FootprintError("the top level must be an object with the single key 'footprint'")
    footprint = document["footprint"]
    if not isinstance(footprint, dict):
        raise FootprintError("footprint must be an object")
    if tuple(footprint) != FOOTPRINT_KEYS:
        raise FootprintError(f"footprint keys must be exactly {', '.join(FOOTPRINT_KEYS)}, in that order")
    lines_ = footprint["estimated_lines"]
    if not isinstance(lines_, int) or isinstance(lines_, bool) or lines_ < 0:
        raise FootprintError(f"estimated_lines must be an integer of 0 or more, not {lines_!r}")
    for key in FOOTPRINT_KEYS:
        if key != "estimated_lines":
            check_list(footprint[key], key)
    for key in ("files", "new_files", "external_steps", "gate_changes"):
        if not all(isinstance(item, str) for item in footprint[key]):
            raise FootprintError(f"{key} must hold strings")
    check_entries(footprint["new_dependencies"], "new_dependencies", ("name", "manifest"))
    check_entries(footprint["overridden_assumptions"], "overridden_assumptions", ("assumption", "reason"))
    if not footprint["files"]:
        raise FootprintError("files is empty: a plan changes at least one file")
    for path in footprint["new_files"]:
        if path not in footprint["files"]:
            raise FootprintError(f"new_files: {path!r} is not in files")
    for path in footprint["files"]:
        check_path(path, "files")
    for path in footprint["new_files"]:
        check_path(path, "new_files")
    for dep in footprint["new_dependencies"]:
        check_path(dep["manifest"], "new_dependencies.manifest")
    return footprint


# --- rules ---------------------------------------------------------------

def evaluate(footprint: dict, component: str, cfg: dict) -> list:
    declared = list(footprint["files"])
    for dep in footprint["new_dependencies"]:
        if dep["manifest"] not in declared:
            declared.append(dep["manifest"])
    paths = cfg["paths"]
    shared = cfg["shared"]
    results = []

    def rule(name: str, reasons: list) -> None:
        results.append({"rule": name, "result": "hit" if reasons else "clean", "reasons": reasons})

    reasons = []
    for path in declared:
        for glob in matching(cfg["rh_paths"], path, ci=True):
            reasons.append(f"{path} matches require_human.paths {glob}")
    rule("require-human-paths", reasons)

    reasons = []
    if component in cfg["rh_components"]:
        reasons.append(f"the issue's component {component} is in require_human.components")
    for path in declared:
        for name in cfg["rh_components"]:
            for glob in matching(paths[name], path, ci=True):
                reasons.append(f"{path} is in component {name} ({glob}), which requires a human")
    rule("require-human-components", reasons)

    reasons = []
    for path in declared:
        if not matching(paths[component], path) and not matching(shared, path):
            reasons.append(f"{path} is outside component {component} and shared")
    rule("component-scope", reasons)

    reasons = []
    for path in declared:
        if not matching(shared, path) and not any(matching(globs, path) for globs in paths.values()):
            reasons.append(f"{path} is in no component and not in shared")
    rule("no-component", reasons)

    limits = cfg["limits"]
    for name, key, value in (
        ("limit-files", "files", len(footprint["files"])),
        ("limit-lines", "lines", footprint["estimated_lines"]),
        ("limit-new-files", "new_files", len(footprint["new_files"])),
        ("limit-new-dependencies", "new_dependencies", len(footprint["new_dependencies"])),
    ):
        over = value > limits[key]
        rule(name, [f"{key} is {value}, over the limit of {limits[key]}"] if over else [])

    rule("external-steps", [f"external step: {step}" for step in footprint["external_steps"]])

    reasons = [f"declared gate change: {change}" for change in footprint["gate_changes"]]
    for path in declared:
        if path.lower() == CONFIG.lower():
            reasons.append(f"{path} holds the gate definitions (gates.run)")
        for name, word in cfg["gate_words"]:
            if path.lower() == word.lower():
                reasons.append(f"{path} is named by gate {name}")
    rule("gate-changes", reasons)

    reasons = []
    if cfg["tests"] == "none":
        docs = paths.get(DOCS, [])
        for path in declared:
            if not matching(docs, path):
                reasons.append(f"gates.tests is none and {path} is outside component {DOCS}")
    rule("tests-none", reasons)

    rule("overridden-assumptions", [
        f"overridden assumption: {item['assumption']} ({item['reason']})"
        for item in footprint["overridden_assumptions"]
    ])
    return results


# --- documents -----------------------------------------------------------

def read_document(path: str, what: str) -> bytes:
    try:
        data = Path(path).read_bytes()
        data.decode("utf-8")
    except (OSError, UnicodeDecodeError) as exc:
        raise InputError(f"cannot read the {what} {path}: {exc}") from exc
    return data


def document_lines(text: str) -> list:
    return [line[:-1] if line.endswith("\r") else line for line in text.split("\n")]


def headings(lines: list) -> list:
    """(index, level) of each `# ` (1) and `## ` (2) heading outside fenced blocks."""
    found = []
    fence = None  # (character, length) of the open fence
    for i, line in enumerate(lines):
        stripped = line.lstrip(" \t")
        run = re.match(r"`{3,}|~{3,}", stripped)
        if fence is not None:
            if run and run.group()[0] == fence[0] and len(run.group()) >= fence[1] \
                    and not stripped[len(run.group()):].strip():
                fence = None
            continue
        if run:
            fence = (run.group()[0], len(run.group()))
        elif line.startswith("## "):
            found.append((i, 2))
        elif line.startswith("# "):
            found.append((i, 1))
    return found


def section(lines: list, title: str, what: str) -> tuple:
    """(index of the first body line, body lines) of the one `## <title>` section; raises DocumentError."""
    marks = headings(lines)
    starts = [i for i, level in marks if level == 2 and lines[i].rstrip() == f"## {title}"]
    if not starts:
        raise DocumentError(f"{what}: no '## {title}' section")
    if len(starts) > 1:
        raise DocumentError(f"{what}: {len(starts)} '## {title}' sections")
    start = starts[0]
    end = next((i for i, _ in marks if i > start), len(lines))
    return start + 1, lines[start + 1:end]


def issue_body(text: str) -> list:
    """The issue document's lines before its first `## Comments` heading."""
    lines = document_lines(text)
    comments = [i for i, level in headings(lines) if level == 2 and lines[i].rstrip() == "## Comments"]
    return lines[:comments[0]] if comments else lines


def flat(text: str) -> str:
    """Document text on one line, cut to REASON_LIMIT characters."""
    return " ".join(text.split())[:REASON_LIMIT]


def question_reasons(text: str, what: str) -> list:
    lines = document_lines(text)
    try:
        start, body = section(lines, "Questions", what)
    except DocumentError as exc:
        return [str(exc)]
    reasons = []
    for offset, line in enumerate(body):
        if not line.strip() or line[0] in " \t":
            continue  # blank, a continuation or a sub-bullet
        if line.startswith(BLOCKING):
            reasons.append(f"{what}: blocking question: {flat(line[len(BLOCKING):])}")
        elif not line.startswith(NON_BLOCKING):
            reasons.append(f"{what}: line {start + offset + 1} of '## Questions' is neither a "
                           f"'{BLOCKING}' nor a '{NON_BLOCKING}' entry")
    return reasons


def issue_criteria(text: str) -> list:
    _, body = section(issue_body(text), "Acceptance criteria", "the issue document")
    criteria = []
    current = None
    for line in body:
        if not line.strip():
            current = None  # a blank line ends the item
            continue
        item = ITEM.fullmatch(line)
        if item:
            rest = item.group(1)
            box = CHECKBOX.fullmatch(rest)
            current = [box.group(1) if box else rest]
            criteria.append(current)
        elif current is not None:
            current.append(line)
    return [" ".join(" ".join(parts).split()) for parts in criteria]


def table_cells(line: str) -> list:
    row = line.strip()
    if row.startswith("|"):
        row = row[1:]
    if row.endswith("|") and not row.endswith("\\|"):
        row = row[:-1]
    return [cell.replace("\\|", "|") for cell in CELL_BAR.split(row)]


def plan_criteria(text: str) -> list:
    _, body = section(document_lines(text), "Verification", "the plan")
    rows = []
    for offset, line in enumerate(body):
        if line.strip():
            rows = body[offset:]
            break
    table = []
    for line in rows:
        if not line.lstrip(" \t").startswith("|"):
            break
        table.append(line)
    if len(table) < 2 or not all(SEPARATOR_CELL.fullmatch(cell) for cell in table_cells(table[1])):
        raise DocumentError("the plan: '## Verification' does not start with a table header and a '---' row")
    return [" ".join(table_cells(line)[0].split()) for line in table[2:]]


def criteria_reasons(issue: str, plan_text: str) -> list:
    try:
        wanted = issue_criteria(issue)
        if not wanted:
            raise DocumentError("the issue document: '## Acceptance criteria' holds no criteria")
        quoted = plan_criteria(plan_text)
    except DocumentError as exc:
        return [str(exc)]
    reasons = []
    remaining = list(quoted)
    for criterion in wanted:
        if criterion in remaining:
            remaining.remove(criterion)
        else:
            reasons.append(f"the Verification table lacks the criterion: {flat(criterion)}")
    extra = list(wanted)
    for row in quoted:
        if row in extra:
            extra.remove(row)
        else:
            reasons.append(f"the Verification table has a row that is not a criterion of the issue: {flat(row)}")
    return reasons


def header_value(header: list, marker: str) -> tuple:
    """(value, reason): the value of the one header line starting with marker, or why there is none."""
    found = [line for line in header if line.startswith(marker)]
    if len(found) != 1:
        return None, f"the issue document's header has {len(found)} '{marker}' lines, expected 1"
    return found[0][len(marker):].strip(), None


def first_of_kind_reasons(issue: str, component: str, subjects: list) -> list:
    lines = issue_body(issue)
    first = next((i for i, level in headings(lines) if level == 2), len(lines))
    header = lines[:first]
    reasons = []
    kind, why = header_value(header, KIND)
    if why:
        reasons.append(why)
    elif kind != "implementation":
        reasons.append(f"the issue's kind is {flat(kind)!r}, not implementation")
    named, why = header_value(header, COMPONENT)
    if why:
        reasons.append(why)
    elif named != component:
        reasons.append(f"the issue's component is {flat(named)!r}, not --component {component}")
    if reasons:
        return reasons
    scoped = re.compile(rf"[A-Za-z]+\({re.escape(component)}\)!?: ")
    if not any(scoped.match(subject) for subject in subjects):
        reasons.append(f"first of its kind: no commit reachable from the base commit has a subject "
                       f"scoped ({component})")
    return reasons


def history(commit: str) -> list:
    """The subjects of every commit reachable from commit, replace objects and grafts off."""
    env = {k: v for k, v in os.environ.items() if k != "GIT_REPLACE_REF_BASE"}
    env["GIT_NO_REPLACE_OBJECTS"] = "1"
    env["GIT_GRAFT_FILE"] = os.devnull
    argv = ["git", "-c", "log.showSignature=false", "log", "-z", "--format=%s", commit, "--"]
    try:
        proc = subprocess.run(argv, capture_output=True, env=env)
    except OSError as exc:
        raise InputError(f"cannot run git: {exc}") from exc
    if proc.returncode != 0:
        detail = proc.stderr.decode("utf-8", errors="replace").strip().splitlines()
        raise InputError(f"git log failed: {detail[-1] if detail else f'exit {proc.returncode}'}")
    return [s for s in proc.stdout.decode("utf-8", errors="replace").split("\0") if s]


def evaluate_documents(issue, analysis, plan_text: str, component: str, subjects: list) -> list:
    """The three document rules; issue and analysis are text, or None when not given."""
    results = []

    def rule(name: str, reasons: list) -> None:
        results.append({"rule": name, "result": "hit" if reasons else "clean", "reasons": reasons})

    if analysis is None:
        reasons = ["no analysis given (--analysis)"]
    else:
        reasons = question_reasons(analysis, "analysis")
    rule("blocking-questions", reasons + question_reasons(plan_text, "plan"))

    no_issue = ["no issue document given (--issue-document)"]
    rule("acceptance-criteria", no_issue if issue is None else criteria_reasons(issue, plan_text))
    rule("first-of-kind", no_issue if issue is None else first_of_kind_reasons(issue, component, subjects))
    return results


def conclude(plan_text: str, issue, analysis, component: str, cfg: dict, subjects: list) -> tuple:
    """What the floor concludes: (rules, not_evaluated, footprint or None)."""
    try:
        footprint = parse_footprint(plan_text)
    except FootprintError as exc:
        footprint = None
        rules = [{"rule": "footprint", "result": "hit", "reasons": [str(exc)]}]
    else:
        rules = [{"rule": "footprint", "result": "clean", "reasons": []}]
        rules += evaluate(footprint, component, cfg)
    rules += evaluate_documents(issue, analysis, plan_text, component, subjects)
    evaluated = [r["rule"] for r in rules]
    return rules, [name for name in RULES if name not in evaluated], footprint


# --- verdict -------------------------------------------------------------

def remove(path) -> None:
    """Delete a file if it exists; raises OSError when it cannot."""
    try:
        os.unlink(path)
    except FileNotFoundError:
        pass


def remove_stale_verdict() -> None:
    try:
        remove(VERDICT)
    except OSError as exc:
        raise InputError(f"cannot delete the stale verdict {VERDICT}: {exc}") from exc


def write_verdict(verdict: dict) -> None:
    VERDICT.parent.mkdir(parents=True, exist_ok=True)
    fd, tmp = tempfile.mkstemp(prefix=".floor.", suffix=".json", dir=VERDICT.parent)
    try:
        with os.fdopen(fd, "w", encoding="utf-8") as out:
            out.write(json.dumps(verdict, indent=2, ensure_ascii=False) + "\n")
        os.replace(tmp, VERDICT)
    except BaseException:
        try:
            remove(tmp)
        except OSError:
            pass
        raise


def floor(argv: list) -> int:
    remove_stale_verdict()
    args = parse_args(argv)
    commit = resolve_base(args["base"])
    cfg = validate_config(load_config(commit), args["component"])
    data = read_plan(args["plan"])
    documents = {}
    for key, what in (("issue_document", "issue document"), ("analysis", "analysis")):
        path = args[key]
        documents[key] = None if path is None else (path, read_document(path, what))
    subjects = history(commit)

    texts = {key: None if doc is None else doc[1].decode("utf-8") for key, doc in documents.items()}
    results, skipped, _ = conclude(data.decode("utf-8"), texts["issue_document"], texts["analysis"],
                                   args["component"], cfg, subjects)
    hit = any(r["result"] == "hit" for r in results)
    bound = {key: None if doc is None else {"path": doc[0], "sha256": hashlib.sha256(doc[1]).hexdigest()}
             for key, doc in documents.items()}
    verdict = {
        "tool": TOOL,
        "version": VERSION,
        "plan": {"path": args["plan"], "sha256": hashlib.sha256(data).hexdigest()},
        "issue_document": bound["issue_document"],
        "analysis": bound["analysis"],
        "component": args["component"],
        "base": {"ref": args["base"], "commit": commit},
        "config": CONFIG,
        "mode": cfg["mode"],
        "verdict": "hit" if hit else "clean",
        "rules": results,
        "not_evaluated": skipped,
    }
    write_verdict(verdict)
    for r in results:
        line = f"{r['result']:7} {r['rule']}"
        if r["reasons"]:
            line += f": {r['reasons'][0]}"
        print(line)
    print(f"floor: {verdict['verdict']}")
    return 1 if hit else 0


def main(argv: list) -> int:
    try:
        return floor(argv)
    except InputError as exc:
        print(f"floor.py: {exc}", file=sys.stderr)
        return 2
    except Exception as exc:  # a crash must never read as a hit or clean
        try:
            remove(VERDICT)
        except Exception:  # the handler itself must always end in exit 2
            pass
        message = " ".join(str(exc).split())
        print(f"floor.py: unexpected error: {type(exc).__name__}: {message}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
