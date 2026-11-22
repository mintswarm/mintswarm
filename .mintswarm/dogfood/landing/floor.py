#!/usr/bin/env python3
"""Check a finished change against its approved plan: the landing floor.

    floor.py --plan PATH --component NAME --base REF --head REF

Dogfooding stand-in for the deterministic rules that run on a finished
change before any landing decision. It needs git alone, never GitHub. Every
hit sends the change to a human; a clean verdict only means that no rule
written here objects. Paths are relative to the working directory, which is
the root of a checkout holding both commits: git runs there, the plan is
read from there and the verdict is written there. All four arguments are
required, each once, as `--name value`; anything else, a missing value or a
REF starting with `-` is unusable input. The first action is to delete any
stale .mintswarm/job/outputs/landing-floor.json; when that fails, the input
is unusable.

The footprint parser, the glob matching, the configuration checks and the
gate words come from ../plan-approval/floor.py, the plan floor, found next
to this script through its own path and loaded by file path, never through
the working directory. Its own verdict, .mintswarm/job/outputs/floor.json,
is never touched. Both scripts come from wherever this one is run: run from
the branch under review they are the branch's versions, so a trusted run
uses a base-ref copy of both, in the same relative layout (that is the
landing decision's job, not this script's).

Inputs:

- PATH, the approved plan, read as UTF-8. Its footprint is parsed exactly
  as the plan floor parses it (see plan-approval/floor.py).
- NAME, the issue's component.
- REF after --base, the base ref. The configuration is .mintswarm/config.yaml
  at the commit it resolves to, read with `git show` and converted with
  `yq -p=yaml -o=json`. The worktree's config is never opened, and neither
  is the config at the head, so a change to either in the branch under
  review cannot loosen the floor.
- REF after --head, the head of the change. The diff is
  `git diff --numstat --no-renames -z <base>...<head>`, which runs from the
  merge base of the two; added paths come from the same range with
  `--diff-filter=A`. A rename counts as its old and its new path.

The configuration must pass the plan floor's checks (components, shared,
pipeline.plan_approval and gates) and NAME must be a defined component.
`pipeline.landing` must then be a mapping with only these keys; any failure
is unusable input:

  mode             required: `always`, `shadow`, `judged` or `never`
  require_human    optional mapping with optional lists `components` (each
                   a defined component) and `paths` (globs)
  footprint        required mapping with exactly `lines_factor`, an integer
                   or a finite number, never a boolean, of 1 or more

Globs follow the plan floor: a glob matches the whole path, `**` matches
anything including `/`, `**/` zero or more directories, `*` anything within
one segment, `?` one character other than `/`; everything else is literal.

Rules, evaluated in this order. The changed paths are every path in the
diff, deletions and mode-only changes included; the declared paths are the
footprint's `files` plus each new dependency's `manifest`.

  footprint                  the plan's footprint is unusable, by the plan
                             floor's definition
  undeclared-files           a changed path is not a declared path, compared
                             exactly
  undeclared-new-files       an added path is not in `new_files`, compared
                             exactly
  size                       the added plus deleted lines of the diff are
                             over floor(estimated_lines × lines_factor),
                             computed exactly, or an entry is binary or
                             otherwise unmeasurable
  require-human-paths        a changed path matches require_human.paths,
                             compared case-insensitively
  require-human-components   NAME is in require_human.components, or a
                             changed path is in the paths of one of them,
                             compared case-insensitively
  protected                  a changed path is .mintswarm/config.yaml, a
                             word of a gates.run command at the base
                             (without a leading `./`), CODEOWNERS,
                             .github/CODEOWNERS or docs/CODEOWNERS, or is
                             under .github/workflows/, compared
                             case-insensitively; fixed here, whatever the
                             config says

When `footprint` is a hit, `undeclared-files`, `undeclared-new-files` and
`size` are not evaluated; the other rules need only the diff and are.

The verdict is .mintswarm/job/outputs/landing-floor.json, written to a
temporary file and renamed onto it, with these keys in this order: `tool`
(`mintswarm-landing-floor`), `version` (1), `plan` ({`path`, `sha256` of its
bytes}), `component`, `base` and `head` ({`ref` as given, `commit`, the full
hash}), `merge_base` (the full hash), `config` (`.mintswarm/config.yaml`),
`mode` (pipeline.landing.mode as read), `verdict` (`hit` or `clean`),
`rules` (one {`rule`, `result`, `reasons`} per evaluated rule, in the order
above, `reasons` empty when clean) and `not_evaluated` (the identifiers
skipped, in order). One line per evaluated rule is printed, then
`floor: hit` or `floor: clean`.

Exits 0 when clean, 1 on any hit, 2 on unusable input: bad arguments, a
missing or broken plan-approval/floor.py, an unresolvable base or head, no
merge base, no config at the base, an invalid config, an unreadable plan, a
diff that does not parse, git or yq missing or failing, a stale verdict that
cannot be deleted, or any unexpected error. Unusable input leaves no verdict
file, unless a stale one could not be deleted. The interpreter itself exits
1 when it crashes outside main(), for example at import time, so exit 0 or
1 counts only together with a verdict file whose `plan.sha256`,
`base.commit` and `head.commit` match the request; exit 2 never comes with
a valid verdict. Standard library only, apart from running git and yq.
"""

import hashlib
import importlib.util
import json
import math
import os
import re
import subprocess
import sys
import tempfile
from fractions import Fraction
from pathlib import Path

PLAN_FLOOR = Path(__file__).resolve().parent.parent / "plan-approval" / "floor.py"
VERDICT = Path(".mintswarm/job/outputs/landing-floor.json")
CONFIG = ".mintswarm/config.yaml"
TOOL = "mintswarm-landing-floor"
VERSION = 1
MODES = ("always", "shadow", "judged", "never")
RULES = (
    "footprint",
    "undeclared-files",
    "undeclared-new-files",
    "size",
    "require-human-paths",
    "require-human-components",
    "protected",
)
DIFF_RULES = ("undeclared-files", "undeclared-new-files", "size")
CODEOWNERS = ("codeowners", ".github/codeowners", "docs/codeowners")
PROTECTED_GLOBS = (".github/workflows/**",)
ARGS = ("--plan", "--component", "--base", "--head")
USAGE = "usage: floor.py --plan PATH --component NAME --base REF --head REF"
HASH = re.compile(r"[0-9a-f]{40}|[0-9a-f]{64}")
DIFF_OPTIONS = ("--no-renames", "-z", "--no-ext-diff", "--no-textconv", "--no-relative", "--no-color")

P = None  # the plan floor's module, once loaded


class InputError(Exception):
    """Unusable input: exit 2, no verdict."""


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


def load_plan_floor():
    if not PLAN_FLOOR.is_file():
        raise InputError(f"the plan floor {PLAN_FLOOR} is missing")
    sys.dont_write_bytecode = True  # leave no __pycache__ next to the plan floor
    try:
        spec = importlib.util.spec_from_file_location("msw_plan_floor", PLAN_FLOOR)
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
    except Exception as exc:
        message = " ".join(str(exc).split())
        raise InputError(f"cannot load the plan floor {PLAN_FLOOR}: {type(exc).__name__}: {message}") from exc
    return module


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
    missing = [name for name in ARGS if name not in args]
    if missing:
        raise InputError(f"missing {', '.join(missing)}; {USAGE}")
    for name in ("--base", "--head"):
        if args[name].startswith("-"):
            raise InputError(f"{name} must not start with '-': {args[name]!r}")
    return {
        "plan": args["--plan"],
        "component": args["--component"],
        "base": args["--base"],
        "head": args["--head"],
    }


# --- base-ref configuration ---------------------------------------------

def validate_landing(config: dict, components: dict) -> dict:
    pipeline = config.get("pipeline")
    if not isinstance(pipeline, dict) or "landing" not in pipeline:
        raise InputError("config: pipeline.landing is missing")
    landing = pipeline["landing"]
    if not isinstance(landing, dict):
        raise InputError("config: pipeline.landing must be a mapping")
    unknown = sorted(set(landing) - {"mode", "require_human", "footprint"})
    if unknown:
        raise InputError(f"config: unknown keys in pipeline.landing: {', '.join(unknown)}")
    mode = landing.get("mode")
    if mode not in MODES:
        raise InputError(f"config: pipeline.landing.mode must be one of {', '.join(MODES)}, not {mode!r}")

    require = landing.get("require_human")
    if require is None:
        require = {}
    if not isinstance(require, dict):
        raise InputError("config: pipeline.landing.require_human must be a mapping")
    unknown = sorted(set(require) - {"components", "paths"})
    if unknown:
        raise InputError(f"config: unknown keys in pipeline.landing.require_human: {', '.join(unknown)}")
    rh_components = P.string_list(require.get("components"), "pipeline.landing.require_human.components")
    rh_paths = P.string_list(require.get("paths"), "pipeline.landing.require_human.paths")
    undefined = [c for c in rh_components if c not in components]
    if undefined:
        raise InputError(
            f"config: pipeline.landing.require_human.components names undefined components: {', '.join(undefined)}")

    footprint = landing.get("footprint")
    if not isinstance(footprint, dict) or set(footprint) != {"lines_factor"}:
        raise InputError("config: pipeline.landing.footprint must be a mapping with exactly lines_factor")
    factor = footprint["lines_factor"]
    if (not isinstance(factor, (int, float)) or isinstance(factor, bool)
            or (isinstance(factor, float) and not math.isfinite(factor)) or factor < 1):
        raise InputError(f"config: pipeline.landing.footprint.lines_factor must be a number of 1 or more, not {factor!r}")
    return {"mode": mode, "rh_components": rh_components, "rh_paths": rh_paths, "lines_factor": factor}


def line_limit(estimated_lines: int, factor) -> int:
    """floor(estimated_lines × factor) in exact arithmetic: 100 × 1.15 is 115."""
    exact = Fraction(factor) if isinstance(factor, int) else Fraction(repr(factor))
    return math.floor(estimated_lines * exact)


# --- the diff ------------------------------------------------------------

def resolve_head(ref: str) -> str:
    try:
        return P.resolve_base(ref)
    except P.InputError as exc:
        if "does not resolve" in str(exc):
            raise InputError(f"head ref {ref!r} does not resolve to a commit") from exc
        raise


def merge_base(base: str, head: str) -> str:
    try:
        proc = subprocess.run(["git", "merge-base", base, head], capture_output=True)
    except OSError as exc:
        raise InputError(f"cannot run git: {exc}") from exc
    commit = proc.stdout.decode("ascii", errors="replace").strip()
    if proc.returncode != 0 or not HASH.fullmatch(commit):
        raise InputError(f"base {base} and head {head} have no merge base")
    return commit


def split_z(data: bytes, what: str) -> list:
    if not data:
        return []
    if not data.endswith(b"\0"):
        raise InputError(f"{what}: output does not end in NUL")
    records = []
    for raw in data[:-1].split(b"\0"):
        try:
            records.append(raw.decode("utf-8"))
        except UnicodeDecodeError as exc:
            raise InputError(f"{what}: a record is not UTF-8: {raw!r}") from exc
    return records


def read_diff(base: str, head: str) -> tuple:
    """Return ([(path, added, deleted)], [added path]); counts are None when unmeasurable."""
    span = f"{base}...{head}"
    entries = []
    for record in split_z(P.run(["git", "diff", "--numstat", *DIFF_OPTIONS, span]), "git diff --numstat"):
        parts = record.split("\t", 2)
        if len(parts) != 3 or parts[2] == "":
            raise InputError(f"git diff --numstat: unexpected record {record!r}")
        added, deleted, path = parts
        if added == "-" and deleted == "-":
            entries.append((path, None, None))
        elif added.isdigit() and deleted.isdigit() and added.isascii() and deleted.isascii():
            entries.append((path, int(added), int(deleted)))
        else:
            raise InputError(f"git diff --numstat: unexpected counts in {record!r}")
    changed = [path for path, _, _ in entries]
    if len(set(changed)) != len(changed):
        raise InputError("git diff --numstat: a path appears twice")
    new = split_z(P.run(["git", "diff", "--name-only", "--diff-filter=A", *DIFF_OPTIONS, span]),
                  "git diff --diff-filter=A")
    stray = [path for path in new if path not in changed]
    if stray or any(path == "" for path in new):
        raise InputError(f"git diff --diff-filter=A: paths missing from --numstat: {stray}")
    return entries, new


# --- rules ---------------------------------------------------------------

def footprint_rules(footprint: dict, entries: list, new: list, landing: dict) -> list:
    declared = list(footprint["files"])
    for dep in footprint["new_dependencies"]:
        if dep["manifest"] not in declared:
            declared.append(dep["manifest"])
    results = []

    def rule(name: str, reasons: list) -> None:
        results.append({"rule": name, "result": "hit" if reasons else "clean", "reasons": reasons})

    rule("undeclared-files", [
        f"{path} is changed but is neither in files nor a new dependency's manifest"
        for path, _, _ in entries if path not in declared
    ])
    rule("undeclared-new-files", [
        f"{path} is added but is not in new_files"
        for path in new if path not in footprint["new_files"]
    ])

    reasons = []
    limit = line_limit(footprint["estimated_lines"], landing["lines_factor"])
    total = sum(a + d for _, a, d in entries if a is not None)
    if total > limit:
        reasons.append(
            f"the diff counts {total} lines, over the limit of {limit} "
            f"(estimated_lines {footprint['estimated_lines']} × lines_factor {landing['lines_factor']!r})")
    for path, added, _ in entries:
        if added is None:
            reasons.append(f"{path} is binary or otherwise unmeasurable")
    rule("size", reasons)
    return results


def policy_rules(entries: list, component: str, cfg: dict, landing: dict) -> list:
    changed = [path for path, _, _ in entries]
    paths = cfg["paths"]
    results = []

    def rule(name: str, reasons: list) -> None:
        results.append({"rule": name, "result": "hit" if reasons else "clean", "reasons": reasons})

    reasons = []
    for path in changed:
        for glob in P.matching(landing["rh_paths"], path, ci=True):
            reasons.append(f"{path} matches require_human.paths {glob}")
    rule("require-human-paths", reasons)

    reasons = []
    if component in landing["rh_components"]:
        reasons.append(f"the issue's component {component} is in require_human.components")
    for path in changed:
        for name in landing["rh_components"]:
            for glob in P.matching(paths[name], path, ci=True):
                reasons.append(f"{path} is in component {name} ({glob}), which requires a human")
    rule("require-human-components", reasons)

    reasons = []
    for path in changed:
        lower = path.lower()
        if lower == CONFIG.lower():
            reasons.append(f"{path} holds the gates and the landing policy")
        if lower in CODEOWNERS:
            reasons.append(f"{path} is a CODEOWNERS location")
        for name, word in cfg["gate_words"]:
            if lower == word.lower():
                reasons.append(f"{path} is named by gate {name}")
        for glob in P.matching(list(PROTECTED_GLOBS), path, ci=True):
            reasons.append(f"{path} is under .github/workflows/ ({glob})")
    rule("protected", reasons)
    return results


# --- verdict -------------------------------------------------------------

def write_verdict(verdict: dict) -> None:
    VERDICT.parent.mkdir(parents=True, exist_ok=True)
    fd, tmp = tempfile.mkstemp(prefix=".landing-floor.", suffix=".json", dir=VERDICT.parent)
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


def landing_floor(argv: list) -> int:
    global P
    remove_stale_verdict()
    args = parse_args(argv)
    P = load_plan_floor()

    base = P.resolve_base(args["base"])
    config = P.load_config(base)
    cfg = P.validate_config(config, args["component"])
    landing = validate_landing(config, cfg["paths"])
    head = resolve_head(args["head"])
    merge = merge_base(base, head)
    data = P.read_plan(args["plan"])
    entries, new = read_diff(base, head)

    try:
        footprint = P.parse_footprint(data.decode("utf-8"))
    except P.FootprintError as exc:
        results = [{"rule": "footprint", "result": "hit", "reasons": [str(exc)]}]
    else:
        results = [{"rule": "footprint", "result": "clean", "reasons": []}]
        results += footprint_rules(footprint, entries, new, landing)
    results += policy_rules(entries, args["component"], cfg, landing)
    evaluated = [r["rule"] for r in results]
    hit = any(r["result"] == "hit" for r in results)
    verdict = {
        "tool": TOOL,
        "version": VERSION,
        "plan": {"path": args["plan"], "sha256": hashlib.sha256(data).hexdigest()},
        "component": args["component"],
        "base": {"ref": args["base"], "commit": base},
        "head": {"ref": args["head"], "commit": head},
        "merge_base": merge,
        "config": CONFIG,
        "mode": landing["mode"],
        "verdict": "hit" if hit else "clean",
        "rules": results,
        "not_evaluated": [name for name in RULES if name not in evaluated],
    }
    write_verdict(verdict)
    for r in results:
        line = f"{r['result']:7} {r['rule']}"
        if r["reasons"]:
            line += f": {r['reasons'][0]}"
        print(line)
    print(f"floor: {verdict['verdict']}")
    return 1 if hit else 0


def is_input_error(exc: Exception) -> bool:
    return isinstance(exc, InputError) or (P is not None and isinstance(exc, P.InputError))


def main(argv: list) -> int:
    try:
        return landing_floor(argv)
    except Exception as exc:  # a crash must never read as a hit or clean
        message = " ".join(str(exc).split())
        if is_input_error(exc):
            print(f"landing/floor.py: {message}", file=sys.stderr)
            return 2
        try:
            remove(VERDICT)
        except Exception:  # the handler itself must always end in exit 2
            pass
        print(f"landing/floor.py: unexpected error: {type(exc).__name__}: {message}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
