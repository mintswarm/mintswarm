#!/usr/bin/env python3
"""Check a finished change against its approved plan: the landing floor.

    floor.py --plan PATH --component NAME --base REF --head REF --facts PATH

Dogfooding stand-in for the deterministic rules that run on a finished
change before any landing decision. It needs git alone, never GitHub. Every
hit sends the change to a human; a clean verdict only means that no rule
written here objects. Paths are relative to the working directory, which is
the root of a checkout holding both commits: git runs there, the plan and
the facts are read from there and the verdict is written there. All five
arguments are required, each once, as `--name value`; anything else, a
missing value or a REF starting with `-` is unusable input. The first action is to delete any
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

- PATH after --plan, the approved plan, read as UTF-8. Its footprint is parsed exactly
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
  `--diff-filter=A`, and file modes and blob ids from `--raw`. A rename
  counts as its old and its new path. A head that the base already contains
  (the merge base is the head, so there is no change to judge) is unusable.
  Every diff also passes `--ignore-submodules=none`, so
  neither a `.gitmodules` `ignore` setting (read from the worktree, which
  the branch controls) nor any config can hide a gitlink, and
  `--diff-algorithm=myers`, `--no-ext-diff`, `--no-textconv`,
  `--no-relative` and `--no-color`, which override any `diff.*` config.
- PATH after --facts, the landing facts: the facts.json that
  `collect-facts.py assemble` writes (see landing/collect-facts.py). A
  symbolic link as its final component, or anything but a regular file,
  is refused; symbolic links among the directories above it are followed. It is read after the merge base is found and
  before the plan, and must be strict UTF-8 JSON (no duplicate keys, no
  NaN or Infinity) holding an object with `tool`
  `mintswarm-landing-facts`, `version` 1, and `head` and `base` full
  hashes equal to the commits resolved from --head and --base. Only the
  fields the rules read are checked, by type: `checks` a list of objects
  with `name` and `source` strings and `conclusion` and `app` a string or
  null; `gate_rounds` with `round` an integer (never a boolean),
  `verdict` a string, `failed_tests` a list and `reason` a string or null
  (a failed test that is not a string, such as a CTRF test without a
  name, is an unnamed failed test); `review_rounds` with `round` an integer and `verdict` a
  string; `steers` with `kind`, `author`, `time` and `url` strings. Round
  numbers are positive and strictly increasing, so the last entry is the
  latest round. A verdict or conclusion value the rules do not know is a
  hit, not unusable input.

Every git command, the plan floor's included, runs with a neutral
configuration set in this process's environment:

- the system and global config files are not read (GIT_CONFIG_NOSYSTEM=1,
  GIT_CONFIG_GLOBAL set to the null device, GIT_ATTR_NOSYSTEM=1);
- config, external-diff and diff-option settings passed through the
  environment (GIT_CONFIG_PARAMETERS, GIT_CONFIG_COUNT with its
  GIT_CONFIG_KEY_n and GIT_CONFIG_VALUE_n, GIT_EXTERNAL_DIFF,
  GIT_DIFF_OPTS) are dropped;
- replace objects and grafts are off (GIT_NO_REPLACE_OBJECTS=1,
  GIT_GRAFT_FILE set to the null device, GIT_REPLACE_REF_BASE dropped), so
  neither refs/replace/ (`git replace`, `git replace --graft`) nor
  info/grafts, which every worktree of the repository shares, can swap the
  content of the base or the head, the config read at the base, or the
  history the merge base is found in.

The repository's own config is still read, as git needs it; the options
above override the settings in it that could hide or reshape entries.

Binary entries are found from their content, not from attributes: an entry
is binary when either side's blob holds a NUL byte in its first 8000 bytes,
git's own test, read with `git cat-file --batch`. Attributes can make git
count an entry as binary (`-` in --numstat), and such an entry stays
unmeasurable, which only adds `size` hits; they cannot make a binary entry
count as text. That covers every attribute source: a tracked or untracked
.gitattributes in the worktree, $GIT_DIR/info/attributes, the repository's
core.attributesFile and its default $XDG_CONFIG_HOME/git/attributes (or
~/.config/git/attributes), and the system file.

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
                             computed exactly, or an entry is binary
                             (see above) or a submodule (gitlink) on
                             either side
  require-human-paths        a changed path matches require_human.paths,
                             compared case-insensitively
  require-human-components   NAME is in require_human.components, or a
                             changed path is in the paths of one of them,
                             compared case-insensitively
  protected                  a changed path is .mintswarm/config.yaml, a
                             word of a gates.run command at the base
                             (without a leading `./`), CODEOWNERS,
                             .github/CODEOWNERS or docs/CODEOWNERS, is
                             .github/workflows or under it, or is a
                             directory above any of these (every changed
                             path is a single entry, so such a path is a
                             file, symlink or gitlink standing in for that
                             directory), compared case-insensitively; fixed
                             here, whatever the config says
  gates                      there is no gate round, or the last one's
                             verdict is not `passed`, or it lists a failed
                             test (each named, or called unnamed when its
                             name is not a string)
  checks                     a check on the head has a conclusion other
                             than `success`, a null conclusion (not
                             completed) included, each named with its
                             source and app; or there are no checks while
                             the base has a .github/workflows entry (found
                             case-insensitively with `git ls-tree` of the
                             base's root tree and of its .github tree).
                             Zero checks is clean only while the base has
                             no workflows; revisit both with the
                             required-checks list, once CI exists
  review                     there is no review round, or the last one's
                             verdict is not `pass`
  steers                     a steer is listed, each named with its kind,
                             author, time and URL

When `footprint` is a hit, `undeclared-files`, `undeclared-new-files` and
`size` are not evaluated; the other rules need only the diff or the facts
and are.

The facts rules trust what the facts file says. Kept rounds record no
commit, so the last kept gate and review round are trusted to belong to
the head. `fetched_at` and `approval` are not judged here. The facts file
and the kept rounds it is built from sit in job paths a stage can write, so
running the floor from trusted state is the landing decision's concern, not
this script's.

The verdict is .mintswarm/job/outputs/landing-floor.json, written to a
temporary file and renamed onto it, with these keys in this order: `tool`
(`mintswarm-landing-floor`), `version` (1), `plan` ({`path`, `sha256` of its
bytes}), `facts` ({`path` as given, `sha256` of its bytes, `checks`: the
checks it lists, each {`name`, `conclusion`, `source`, `app`}, in file
order}), `component`, `base` and `head` ({`ref` as given, `commit`, the full
hash}), `merge_base` (the full hash), `config` (`.mintswarm/config.yaml`),
`mode` (pipeline.landing.mode as read), `verdict` (`hit` or `clean`),
`rules` (one {`rule`, `result`, `reasons`} per evaluated rule, in the order
above, `reasons` empty when clean) and `not_evaluated` (the identifiers
skipped, in order). One line per evaluated rule is printed, with its first
reason, if any, after a colon; control characters and line or paragraph
separators in that reason are printed as JSON escapes (`\\n`, `\\u2028`), so
a reason cannot add a line. Then `floor: hit` or `floor: clean`.

Exits 0 when clean, 1 on any hit, 2 on unusable input: bad arguments, a
missing or broken plan-approval/floor.py, an unresolvable base or head, no
merge base, a head the base already contains, no config at the base, an
invalid config, a missing, unreadable, malformed or mismatched facts
file, an unreadable plan, a diff that does not parse, git or yq missing or
failing, a stale verdict that cannot be deleted, or any unexpected error.
Unusable input leaves no verdict file, unless a stale one could not be
deleted. The interpreter itself exits 1 when it crashes outside main(), for
example at import time, so exit 0 or 1 counts only together with a verdict
file whose `plan.sha256`, `facts.sha256`, `base.commit` and `head.commit`
match the request; exit 2 never comes with a valid verdict. Standard library only, apart from running git and yq.
"""

import hashlib
import importlib.util
import json
import math
import os
import re
import stat
import subprocess
import sys
import tempfile
import unicodedata
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
    "gates",
    "checks",
    "review",
    "steers",
)
DIFF_RULES = ("undeclared-files", "undeclared-new-files", "size")
CODEOWNERS = ("codeowners", ".github/codeowners", "docs/codeowners")
PROTECTED_DIRS = (".github/workflows",)
PROTECTED_GLOBS = (".github/workflows/**",)
ARGS = ("--plan", "--component", "--base", "--head", "--facts")
USAGE = "usage: floor.py --plan PATH --component NAME --base REF --head REF --facts PATH"
FACTS_TOOL = "mintswarm-landing-facts"  # collect-facts.py's TOOL and VERSION; floor-selftest.py checks both
FACTS_VERSION = 1
HASH = re.compile(r"[0-9a-f]{40}|[0-9a-f]{64}")
DIFF_OPTIONS = (
    "--no-renames", "-z", "--ignore-submodules=none", "--diff-algorithm=myers",
    "--no-ext-diff", "--no-textconv", "--no-relative", "--no-color",
)
GITLINK = "160000"
RAW_HEADER = re.compile(r":([0-7]{6}) ([0-7]{6}) ([0-9a-f]{40}|[0-9a-f]{64}) ([0-9a-f]{40}|[0-9a-f]{64}) ([A-Z])")
DROPPED_ENV = (
    "GIT_CONFIG_PARAMETERS", "GIT_CONFIG_COUNT", "GIT_EXTERNAL_DIFF", "GIT_DIFF_OPTS", "GIT_REPLACE_REF_BASE",
)
BINARY_PROBE = 8000  # bytes git looks at for a NUL when it decides a blob is binary
BLOB_MODES = ("100644", "100755", "120000")
DROPPED_ENV_PREFIXES = ("GIT_CONFIG_KEY_", "GIT_CONFIG_VALUE_")

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


def neutral_git_config() -> None:
    """Keep system, global and environment config away from every git command run here."""
    for name in list(os.environ):
        if name in DROPPED_ENV or name.startswith(DROPPED_ENV_PREFIXES):
            del os.environ[name]
    os.environ["GIT_CONFIG_NOSYSTEM"] = "1"
    os.environ["GIT_CONFIG_GLOBAL"] = os.devnull
    os.environ["GIT_ATTR_NOSYSTEM"] = "1"
    os.environ["GIT_NO_REPLACE_OBJECTS"] = "1"
    os.environ["GIT_GRAFT_FILE"] = os.devnull


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
        "facts": args["--facts"],
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
    if commit == head:
        raise InputError(f"head {head} is already in the history of base {base}: there is no change to judge")
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


def read_modes(span: str) -> tuple:
    """Return ({path: (old mode, new mode, old blob, new blob)}, [added path]) from `git diff --raw`."""
    records = split_z(P.run(["git", "diff", "--raw", "--no-abbrev", *DIFF_OPTIONS, span]), "git diff --raw")
    if len(records) % 2:
        raise InputError("git diff --raw: a header without a path")
    modes, added = {}, []
    for header, path in zip(records[0::2], records[1::2]):
        match = RAW_HEADER.fullmatch(header)
        if not match or path == "" or path in modes:
            raise InputError(f"git diff --raw: unexpected record {header!r} {path!r}")
        modes[path] = (match.group(1), match.group(2), match.group(3), match.group(4))
        if match.group(5) == "A":
            added.append(path)
    return modes, added


def binary_blobs(blobs: set) -> set:
    """Return the blobs that hold a NUL in their first BINARY_PROBE bytes."""
    blobs = sorted(b for b in blobs if b.strip("0"))
    if not blobs:
        return set()
    try:
        proc = subprocess.Popen(["git", "cat-file", "--batch"], stdin=subprocess.PIPE,
                                stdout=subprocess.PIPE, stderr=subprocess.DEVNULL)
    except OSError as exc:
        raise InputError(f"cannot run git: {exc}") from exc
    binary = set()
    try:
        for blob in blobs:
            proc.stdin.write(blob.encode("ascii") + b"\n")
            proc.stdin.flush()
            header = proc.stdout.readline().decode("ascii", errors="replace").split()
            if len(header) != 3 or header[0] != blob or header[1] != "blob" or not header[2].isdigit():
                raise InputError(f"git cat-file --batch: cannot read blob {blob}: {' '.join(header)!r}")
            size = int(header[2])
            probe = proc.stdout.read(min(size, BINARY_PROBE))
            remaining = size - len(probe) + 1  # the rest of the content and its closing newline
            while remaining > 0:
                chunk = proc.stdout.read(min(remaining, 1 << 20))
                if not chunk:
                    break
                remaining -= len(chunk)
            if len(probe) != min(size, BINARY_PROBE) or remaining:
                raise InputError(f"git cat-file --batch: blob {blob} is cut short")
            if b"\0" in probe:
                binary.add(blob)
    finally:
        try:
            proc.stdin.close()
        except OSError:
            pass
        proc.stdout.close()
        proc.wait()
    return binary


def read_diff(base: str, head: str) -> tuple:
    """Return ([(path, added, deleted)], [added path], {gitlink path}).

    Counts are None when unmeasurable: binary, or a gitlink on either side.
    """
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
    modes, raw_added = read_modes(span)
    if sorted(modes) != sorted(changed) or sorted(raw_added) != sorted(new):
        raise InputError("git diff --raw and --numstat disagree on the changed or added paths")
    gitlinks = {path for path, (old, new_, _, _) in modes.items() if GITLINK in (old, new_)}
    blobs = {blob for old, new_, a, b in modes.values() for mode, blob in ((old, a), (new_, b)) if mode in BLOB_MODES}
    binary = binary_blobs(blobs)
    unmeasurable = gitlinks | {path for path, (_, _, a, b) in modes.items() if a in binary or b in binary}
    entries = [(path, None, None) if path in unmeasurable else (path, a, d) for path, a, d in entries]
    return entries, new, gitlinks


# --- the facts file --------------------------------------------------------

def brief(value) -> str:
    text = json.dumps(value, ensure_ascii=False)
    return text if len(text) <= 60 else text[:57] + "..."


def strict_json(data: bytes, where: str):
    def no_duplicates(pairs):
        result = {}
        for key, value in pairs:
            if key in result:
                raise ValueError(f"duplicate key {key!r}")
            result[key] = value
        return result

    def no_constant(name):
        raise ValueError(f"{name} is not JSON")

    try:
        text = data.decode("utf-8")
    except UnicodeDecodeError as exc:
        raise InputError(f"{where} is not UTF-8: {exc}") from exc
    try:
        return json.loads(text, object_pairs_hook=no_duplicates, parse_constant=no_constant)
    except ValueError as exc:
        raise InputError(f"{where} is not JSON: {exc}") from exc


def read_bytes_nofollow(path: str, where: str) -> bytes:
    try:
        info = os.lstat(path)
    except FileNotFoundError as exc:
        raise InputError(f"{where} is missing") from exc
    except OSError as exc:
        raise InputError(f"cannot read {where}: {exc}") from exc
    if stat.S_ISLNK(info.st_mode):
        raise InputError(f"{where} is a symbolic link")
    if not stat.S_ISREG(info.st_mode):
        raise InputError(f"{where} is not a regular file")
    chunks = []
    try:
        fd = os.open(path, os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0))
        try:
            while True:
                chunk = os.read(fd, 1 << 20)
                if not chunk:
                    break
                chunks.append(chunk)
        finally:
            os.close(fd)
    except OSError as exc:
        raise InputError(f"cannot read {where}: {exc}") from exc
    return b"".join(chunks)


def is_string(value) -> bool:
    return isinstance(value, str)


def is_string_or_null(value) -> bool:
    return value is None or isinstance(value, str)


def is_integer(value) -> bool:
    return isinstance(value, int) and not isinstance(value, bool)


def is_list(value) -> bool:
    return isinstance(value, list)


FACTS_LISTS = (
    ("checks", (("name", is_string, "a string"), ("conclusion", is_string_or_null, "a string or null"),
                ("source", is_string, "a string"), ("app", is_string_or_null, "a string or null"))),
    ("gate_rounds", (("round", is_integer, "an integer"), ("verdict", is_string, "a string"),
                     ("failed_tests", is_list, "a list"),
                     ("reason", is_string_or_null, "a string or null"))),
    ("review_rounds", (("round", is_integer, "an integer"), ("verdict", is_string, "a string"))),
    ("steers", (("kind", is_string, "a string"), ("author", is_string, "a string"),
                ("time", is_string, "a string"), ("url", is_string, "a string"))),
)


def read_facts(path: str, base: str, head: str) -> tuple:
    """Return (the facts file's bytes, its parsed object), checked by type against the floor's commits."""
    where = f"the facts file {path}"
    data = read_bytes_nofollow(path, where)
    facts = strict_json(data, where)
    if not isinstance(facts, dict):
        raise InputError(f"{where} is not a JSON object")
    if facts.get("tool") != FACTS_TOOL:
        raise InputError(f"{where}: tool must be {FACTS_TOOL!r}, not {brief(facts.get('tool'))}")
    version = facts.get("version")
    if not is_integer(version) or version != FACTS_VERSION:
        raise InputError(f"{where}: version must be {FACTS_VERSION}, not {brief(version)}")
    for key, commit in (("head", head), ("base", base)):
        value = facts.get(key)
        if not isinstance(value, str) or not HASH.fullmatch(value):
            raise InputError(f"{where}: {key} must be a full commit hash, not {brief(value)}")
        if value != commit:
            raise InputError(f"{where} is for {key} {value}, not the floor's {key} {commit}")
    for key, fields in FACTS_LISTS:
        if key not in facts:
            raise InputError(f"{where} has no {key}")
        entries = facts[key]
        if not isinstance(entries, list):
            raise InputError(f"{where}: {key} must be a list, not {brief(entries)}")
        for i, entry in enumerate(entries):
            at = f"{key}[{i}]"
            if not isinstance(entry, dict):
                raise InputError(f"{where}: {at} must be an object, not {brief(entry)}")
            for name, valid, what in fields:
                if name not in entry:
                    raise InputError(f"{where}: {at} has no {name}")
                if not valid(entry[name]):
                    raise InputError(f"{where}: {at}.{name} must be {what}, not {brief(entry[name])}")
        if key.endswith("_rounds"):
            numbers = [entry["round"] for entry in entries]
            if any(n < 1 for n in numbers) or any(a >= b for a, b in zip(numbers, numbers[1:])):
                raise InputError(f"{where}: {key} round numbers must be positive and strictly increasing, "
                                 f"not {brief(numbers)}")
    return data, facts


def ls_tree(treeish: str) -> list:
    """Return [(type, object, name)] for the entries of a tree, from `git ls-tree -z --full-tree`."""
    entries = []
    for record in split_z(P.run(["git", "ls-tree", "-z", "--full-tree", treeish]), "git ls-tree"):
        header, tab, name = record.partition("\t")
        parts = header.split(" ")
        if not tab or name == "" or len(parts) != 3:
            raise InputError(f"git ls-tree: unexpected record {record!r}")
        entries.append((parts[1], parts[2], name))
    return entries


def has_workflows(base: str) -> bool:
    """True when the base commit has a .github/workflows entry, names compared case-insensitively."""
    for kind, obj, name in ls_tree(base):
        if kind == "tree" and name.lower() == ".github":
            if any(entry.lower() == "workflows" for _, _, entry in ls_tree(obj)):
                return True
    return False


# --- rules ---------------------------------------------------------------

def footprint_rules(footprint: dict, entries: list, new: list, gitlinks: set, landing: dict) -> list:
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
        if path in gitlinks:
            reasons.append(f"{path} is a submodule (gitlink), whose change is not measured in lines")
        elif added is None:
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

    targets = [CONFIG, *CODEOWNERS, *PROTECTED_DIRS, *(word for _, word in cfg["gate_words"])]
    above = {}
    for target in targets:
        segments = target.lower().split("/")
        for i in range(1, len(segments)):
            directory = "/".join(segments[:i])
            if directory and all(segments[:i]):
                above.setdefault(directory, target)
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
        if lower in PROTECTED_DIRS:
            reasons.append(f"{path} is the protected directory itself, changed as a single entry")
        if lower in above:
            reasons.append(f"{path} is a single entry standing in for a directory above the protected {above[lower]}")
    rule("protected", reasons)
    return results


def facts_rules(facts: dict, base: str) -> list:
    results = []

    def rule(name: str, reasons: list) -> None:
        results.append({"rule": name, "result": "hit" if reasons else "clean", "reasons": reasons})

    reasons = []
    if not facts["gate_rounds"]:
        reasons.append("there is no gate round")
    else:
        last = facts["gate_rounds"][-1]
        if last["verdict"] != "passed":
            line = f"the last gate round, {last['round']}, has verdict {last['verdict']!r}, not 'passed'"
            if last["reason"]:
                line += f": {last['reason']}"
            reasons.append(line)
        for test in last["failed_tests"]:
            if isinstance(test, str):
                reasons.append(f"gate {test} failed in the last gate round, {last['round']}")
            else:  # the collector names a CTRF test without a string name as it found it
                reasons.append(f"an unnamed gate ({brief(test)}) failed in the last gate round, {last['round']}")
    rule("gates", reasons)

    reasons = []
    for check in facts["checks"]:
        if check["conclusion"] == "success":
            continue
        app = check["app"] if check["app"] is not None else "none"
        label = f"check {check['name']} ({check['source']}, app {app})"
        if check["conclusion"] is None:
            reasons.append(f"{label} is not completed")
        else:
            reasons.append(f"{label} has conclusion {check['conclusion']!r}, not 'success'")
    if not facts["checks"] and has_workflows(base):
        reasons.append("no check is reported on the head, but the base has .github/workflows")
    rule("checks", reasons)

    reasons = []
    if not facts["review_rounds"]:
        reasons.append("there is no review round")
    else:
        last = facts["review_rounds"][-1]
        if last["verdict"] != "pass":
            reasons.append(f"the last review round, {last['round']}, has verdict {last['verdict']!r}, not 'pass'")
    rule("review", reasons)

    rule("steers", [
        f"{steer['kind']} by {steer['author']} at {steer['time']} is a steer: {steer['url']}"
        for steer in facts["steers"]
    ])
    return results


# --- verdict -------------------------------------------------------------

def printable(text: str) -> str:
    """Escape control characters and line or paragraph separators, so a printed reason stays on its line."""
    return "".join(json.dumps(c)[1:-1] if unicodedata.category(c) in ("Cc", "Zl", "Zp") else c for c in text)


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
    neutral_git_config()
    P = load_plan_floor()

    base = P.resolve_base(args["base"])
    config = P.load_config(base)
    cfg = P.validate_config(config, args["component"])
    landing = validate_landing(config, cfg["paths"])
    head = resolve_head(args["head"])
    merge = merge_base(base, head)
    facts_data, facts = read_facts(args["facts"], base, head)
    data = P.read_plan(args["plan"])
    entries, new, gitlinks = read_diff(base, head)

    try:
        footprint = P.parse_footprint(data.decode("utf-8"))
    except P.FootprintError as exc:
        results = [{"rule": "footprint", "result": "hit", "reasons": [str(exc)]}]
    else:
        results = [{"rule": "footprint", "result": "clean", "reasons": []}]
        results += footprint_rules(footprint, entries, new, gitlinks, landing)
    results += policy_rules(entries, args["component"], cfg, landing)
    results += facts_rules(facts, base)
    evaluated = [r["rule"] for r in results]
    hit = any(r["result"] == "hit" for r in results)
    verdict = {
        "tool": TOOL,
        "version": VERSION,
        "plan": {"path": args["plan"], "sha256": hashlib.sha256(data).hexdigest()},
        "facts": {
            "path": args["facts"],
            "sha256": hashlib.sha256(facts_data).hexdigest(),
            "checks": [{"name": c["name"], "conclusion": c["conclusion"], "source": c["source"], "app": c["app"]}
                       for c in facts["checks"]],
        },
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
            line += f": {printable(r['reasons'][0])}"
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
