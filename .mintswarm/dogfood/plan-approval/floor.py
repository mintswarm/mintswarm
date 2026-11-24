#!/usr/bin/env python3
"""Check a plan and its documents against the deterministic floor of plan approval.

    floor.py --plan PATH --component NAME --base REF
             [--issue-document PATH] [--analysis PATH] [--screen PATH]

Dogfooding stand-in for the control plane's plan-approval floor, the
deterministic rules that run on a plan before the approval decision. Every
hit sends the plan to a human; a clean verdict only means that no rule
written here objects. Paths are relative to the working directory, which is
the worktree root. --plan, --component and --base are required, and
--issue-document, --analysis and --screen optional; each is given at most
once, as `--name value`. Anything else, an empty value, a value that is an
argument name or a REF starting with `-` is unusable input. A document that
is not given is a hit of each rule that reads it. The first action is to
delete any stale .mintswarm/job/outputs/floor.json; when that fails, the
input is unusable.

Inputs:

- PATH, the plan, read as UTF-8. Its footprint is the `## Footprint` section
  (see the plan stage): a single "```json" block holding
  {"footprint": {"files", "new_files", "estimated_lines", "new_dependencies",
  "external_steps", "gate_changes", "overridden_assumptions"}}, keys in that
  order, or the same with "production_lines" right after "estimated_lines".
  A footprint without it reads as one whose production_lines is
  estimated_lines.
- NAME, the issue's component.
- REF, the base ref. The configuration is .mintswarm/config.yaml at the
  commit REF resolves to, read with `git show` and converted with
  `yq -p=yaml -o=json`. The worktree's config is never opened, so a change
  to it in the branch under review cannot loosen the floor. The history is
  the subject of every commit reachable from that commit, read with
  `git -c log.showSignature=false log -z --format=%s`.

The base commit, the config at it and the history are read from an object
store made for this run, never from the job repository's own objects:

- every git command runs with a neutral environment: no system or global
  config, no config, external-diff or diff options passed through the
  environment, none of the variables that point git at another repository
  or object directory (GIT_DIR, GIT_OBJECT_DIRECTORY,
  GIT_ALTERNATE_OBJECT_DIRECTORIES and the rest), replace objects, grafts
  and the commit-graph off (GIT_NO_REPLACE_OBJECTS, GIT_GRAFT_FILE at the
  null device, core.commitGraph=false), git's grafts deprecation hint off
  (advice.graftFileDeprecated=false) and no lazy fetch;
- a repository whose config makes it a partial clone is refused;
- REF is resolved in the job repository with
  `git rev-parse --verify REF^{object}`, without peeling, to the full id
  of an object the job repository holds;
- a fresh bare repository is made with `tempfile.mkdtemp` (prefix
  `msw-floor-objects-`) in the system temp directory, outside every
  worktree, and filled by a protocol v2 `git fetch` of that id over
  `file://` from the job repository's git directory, its upload-pack run
  with replace objects, grafts and the commit-graph off. The receiving side
  hashes every object it is sent, so a forged object fails the fetch. Its
  connectivity check does not cover a history the job repository's own
  .git/shallow cuts short, so the store is refused if it is shallow, and
  then every object reachable from the fetched id is walked in it
  (`git rev-list --objects`): an object missing from the store fails the
  fetch too, with git's whole error output;
- the id is peeled to its commit in that store, and every later read runs
  as `git --git-dir=<store>`; the store is deleted when the run ends,
  whatever its outcome.

The trust boundary that remains: ref names and any revision syntax in REF
are resolved in the job repository, whose refs the stages can write; the
store sits in the system temp directory under the same account, so the
stages cannot write it by convention only (and a killed run leaves it
behind); and a stage that forges any object in the history makes every
run exit 2, which counts as a human decision.
- The PATH given to --issue-document, the issue document (the job's
  .mintswarm/job/inputs/issue.md), and the PATH given to --analysis, the
  analysis, both read as UTF-8. Only the issue document's lines before its
  first `## Comments` heading are read, so headings in comments do nothing.
- The PATH given to --screen, the session's screen record of the plan's
  declared packages (the assess job's .mintswarm/job/outputs/screen.json,
  from .mintswarm/dogfood/dependencies/screen.py), read as UTF-8. The floor
  stays offline: it only reads the record the session made.
- The result records of the analysis and the plan, read as UTF-8 when they
  exist: the file beside the PATH given to --analysis, and the one beside
  the plan's PATH, each named as .mintswarm/dogfood/save-output.py names
  it (`<name>.md` becomes `<name>.result.json`, any other name gets
  `.result.json` appended). A document without one is read from its
  Markdown alone, the transition for documents written before the records.
  The records are not bound in the verdict: decision.py reads them again
  from the same place and recomputes the rules.

Sections of the documents and the plan are found outside fenced blocks: a
fence opens on a line whose first non-space characters are 3 or more
backticks or tildes, and closes on a line of only the same character, at
least as long. A section starts at a `## <title>` line (trailing whitespace
ignored) and runs to the next `## ` or `# ` heading. A section a rule reads
that is missing or appears more than once is a hit of that rule.

The configuration is validated; any failure is unusable input: `components`
maps names to mappings whose optional `paths` is a list of globs; `shared`
is absent or a list of globs; `pipeline.plan_approval` has `mode` (`always`,
`shadow`, `judged` or `never`), `limits` with exactly the non-negative
integers `files`, `lines` and `new_files`, optionally `require_human`, with
optional lists `components` (each defined) and `paths` (globs), and
optionally `min_confidence`, an integer (not a boolean) from 90 to 100, the
auto-approval confidence the decision requires under `judged`, 90 when
absent, optionally `non_production`, a list of globs matching the paths
whose lines are not production lines (when absent: `**/tests/**`,
`**/*selftest*`, `**/test_*.py`, `**/*_test.*`, `**/*.test.*`,
`**/*.spec.*`, `**/fixtures/**`, `**/Cargo.lock` and `**/pnpm-lock.yaml`),
and optionally `limits_by_component`, mapping defined components to mappings
with some of the keys of `limits`, each a non-negative integer (not a
boolean), which replace the flat `limits` value key by key for that
component; `gates.tests` is absent or a string; `gates.run` is absent or a
list of mappings with a `command` string that shlex can split. NAME must be
a defined component.

A glob matches the whole path: `**` matches anything including `/`, `**/`
zero or more directories, `*` anything within one segment, `?` one
character other than `/`; everything else is literal.

Rules, evaluated in this order. The declared paths are `files` plus each
new dependency's `manifest`.

  footprint                  the footprint is missing, misplaced, not strict
                             JSON (no duplicate keys, NaN or Infinity), not
                             of the shape above (lists without null or
                             repeated values, `estimated_lines` an integer
                             of 0 or more, `production_lines` an integer
                             from 0 to `estimated_lines`, never a boolean,
                             `files` not empty, every
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
  limit-lines                production_lines, when a path in `files`
                             matches `non_production`, or else
                             estimated_lines is over limits.lines
  limit-new-files            len(new_files) is over limits.new_files
  dependency-screen          new_dependencies is not empty and no --screen
                             is given; or the screen record is not usable
                             (below), named by a fixed cause; or, matching
                             the record's packages to new_dependencies by
                             (name, manifest) as multisets, a declared
                             package the record does not list, a record
                             package the footprint does not declare
                             (unbound), a package whose status is
                             `unscreened` (its reason named) and each of a
                             package's disqualifiers (the package, version
                             and manifest named); nothing is counted, and
                             no other field of the record is ever a hit
  external-steps             external_steps is not empty
  gate-changes               gate_changes is not empty, or a declared path is
                             .mintswarm/config.yaml or a word of a gates.run
                             command (without a leading `./`, and also joined
                             to the directory of every earlier `cd ARG` in
                             the same command, the ARG's leading `./`, `.`
                             and empty segments dropped), compared
                             case-insensitively
  tests-none                 gates.tests is `none` and a declared path is
                             outside the `docs` component's paths
  overridden-assumptions     overridden_assumptions is not empty
  blocking-questions         for the analysis and for the plan: the
                             `## Questions` section, which runs to the next
                             `## ` heading, has a `# ` heading or a line
                             at column 0 that does not start with a marker
                             (below); with a usable result record (below),
                             each of its questions whose kind is
                             `must_decide` or `recommended` (the question
                             named), and bullets whose counts disagree with
                             the record's (below, both sets of counts
                             named); with an unusable one, its fixed cause;
                             without one, each line starting with a
                             blocking marker after any spaces or tabs (the
                             question named); other indented lines are
                             skipped; no --analysis
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

The markers of `## Questions` entries are `- **Must decide:**`,
`- **Recommended:**` and `- **Assumption:**`, and for the transition the old
`- **Blocking:**` and `- **Non-blocking (assumed):**`. The blocking markers
are Must decide, Recommended and Blocking; they count at any indentation,
the other two at column 0 only. With a usable record the bullets are counted:
M Must decide, R Recommended, B Blocking and A Assumption plus Non-blocking,
against the record's m `must_decide`, r `recommended` and a `assumption`
questions. They agree when A is a, M is at most m, R is at most r and
(m - M) + (r - R) is B, so an old Blocking bullet stands for either blocking
kind; no bullet is a hit of its own, and assumptions never are.

A usable result record is strict JSON, an object whose `questions` is a list
of objects, each with a string `question` and a `kind` of `must_decide`,
`recommended` or `assumption`; no other field is read. With an unusable
one, the section-level rules still run, but the bullets are neither hits
nor counted.

The limits of the limit-* rules are NAME's: limits with NAME's
limits_by_component applied, a reason naming the override when it decides
the limit.

A usable screen record is strict JSON, an object whose `tool` is
`dependency-screen`, `version` the integer 1, `mode` `plan` and `packages` a
list of objects, each with string `name`, `manifest` and `reason`, `status`
`screened` or `unscreened`, and `disqualifiers` a list of unique values out
of `advisory`, `yanked`, `deprecated` and `typosquat`; other keys are read
by nobody here. With no new_dependencies, no --screen and a record with no
packages are both clean.

Issue criteria are the column-0 items of `## Acceptance criteria` marked
`N.`, `N)`, `-`, `*` or `+`, with an optional `[ ]` or `[x]` after the
marker; following non-blank lines that are not items belong to the item, and
so do indented lines after a blank line, its later paragraphs; a line at
column 0 after a blank line that is not an item ends it. Table rows are
split on `|` not preceded by `\\`, and `\\|` then reads as `|`. Question and
criterion text, and the screen record's text, in reasons is put on one line
and cut to 200 characters.

When `footprint` is a hit, the rules from require-human-paths to
overridden-assumptions, which read the footprint, are not evaluated; the
three document rules always are.

The verdict is .mintswarm/job/outputs/floor.json, written to a temporary
file and renamed onto it, with these keys in this order: `tool`
(`mintswarm-plan-floor`), `version` (3), `plan` ({`path`, `sha256` of its
bytes}), `issue_document`, `analysis` and `screen` (each {`path`, `sha256`
of its bytes}, or null when not given), `component`, `base` ({`ref` as
given, `commit`, the full hash}), `config` (`.mintswarm/config.yaml`),
`mode` (as read), `verdict` (`hit` or `clean`), `rules` (one {`rule`,
`result`, `reasons`} per evaluated rule, in the order above, `reasons` empty
when clean) and `not_evaluated` (the identifiers skipped, in order). One
line per evaluated rule is printed, then `floor: hit` or `floor: clean`.

Exits 0 when clean, 1 on any hit, 2 on unusable input: bad arguments, an
unresolvable base ref, a partial clone, a failed fetch into the object store
(a forged or missing object, a shallow history), no config at the base, an
invalid config, an unreadable or non-UTF-8 plan, issue document, analysis,
screen record or result record that exists, git or yq missing or failing
(reading the history too), a stale verdict that cannot be deleted, or any
unexpected error. Unusable
input leaves no verdict file, unless a stale one could not be deleted. The
interpreter itself exits 1 when it crashes outside main(), for example at
import time, so exit 0 or 1 counts only together with a verdict file whose
`plan.sha256`, `base.commit` and document bindings match the request; exit 2
never comes with a valid verdict. Standard library only, apart from running
git and yq."""

import contextlib
import hashlib
import json
import os
import re
import shlex
import shutil
import subprocess
import sys
import tempfile
import unicodedata
import urllib.parse
from pathlib import Path

VERDICT = Path(".mintswarm/job/outputs/floor.json")
CONFIG = ".mintswarm/config.yaml"
TOOL = "mintswarm-plan-floor"
VERSION = 3
DOCS = "docs"
MODES = ("always", "shadow", "judged", "never")
LIMITS = ("files", "lines", "new_files")
MIN_CONFIDENCE = 90  # the default and lowest pipeline.plan_approval.min_confidence
FOOTPRINT_KEYS = (
    "files",
    "new_files",
    "estimated_lines",
    "new_dependencies",
    "external_steps",
    "gate_changes",
    "overridden_assumptions",
)
# The eight-key footprint: production_lines right after estimated_lines.
FOOTPRINT_KEYS_V2 = FOOTPRINT_KEYS[:3] + ("production_lines",) + FOOTPRINT_KEYS[3:]
# pipeline.plan_approval.non_production when absent: paths whose lines are not production lines.
NON_PRODUCTION = (
    "**/tests/**",
    "**/*selftest*",
    "**/test_*.py",
    "**/*_test.*",
    "**/*.test.*",
    "**/*.spec.*",
    "**/fixtures/**",
    "**/Cargo.lock",
    "**/pnpm-lock.yaml",
)
# The screen record that dependencies/screen.py writes, as the dependency-screen rule reads it.
SCREEN_TOOL = "dependency-screen"
SCREEN_VERSION = 1
SCREEN_MODE = "plan"
SCREEN_STATUSES = ("screened", "unscreened")
DISQUALIFIERS = ("advisory", "yanked", "deprecated", "typosquat")
RULES = (
    "footprint",
    "require-human-paths",
    "require-human-components",
    "component-scope",
    "no-component",
    "limit-files",
    "limit-lines",
    "limit-new-files",
    "dependency-screen",
    "external-steps",
    "gate-changes",
    "tests-none",
    "overridden-assumptions",
    "blocking-questions",
    "acceptance-criteria",
    "first-of-kind",
)
FOOTPRINT_RULES = RULES[1:13]  # the rules that read the footprint, skipped after a `footprint` hit
REQUIRED = ("--plan", "--component", "--base")
OPTIONAL = ("--issue-document", "--analysis", "--screen")
ARGS = REQUIRED + OPTIONAL
USAGE = (
    "usage: floor.py --plan PATH --component NAME --base REF "
    "[--issue-document PATH] [--analysis PATH] [--screen PATH]"
)
FENCE = re.compile(r"^[ \t]*(?:`{3,}|~{3,})json", re.IGNORECASE)
REASON_LIMIT = 200
MUST_DECIDE = "- **Must decide:**"
RECOMMENDED = "- **Recommended:**"
ASSUMPTION = "- **Assumption:**"
BLOCKING = "- **Blocking:**"  # the old marker, read as either blocking kind
NON_BLOCKING = "- **Non-blocking (assumed):**"  # the old marker, read as an assumption
# The `## Questions` markers and what each counts as: a question kind, or `blocking` for either blocking kind.
MARKERS = (
    (MUST_DECIDE, "must_decide"),
    (RECOMMENDED, "recommended"),
    (BLOCKING, "blocking"),
    (ASSUMPTION, "assumption"),
    (NON_BLOCKING, "assumption"),
)
BLOCKING_MARKERS = (MUST_DECIDE, RECOMMENDED, BLOCKING)
QUESTION_KINDS = ("must_decide", "recommended", "assumption")
KIND = "- **Kind:**"
COMPONENT = "- **Component:**"
ITEM = re.compile(r"(?:[0-9]+[.)]|[-*+])(?:[ \t]+|$)(.*)", re.DOTALL)
CHECKBOX = re.compile(r"\[[ x]\](?:[ \t]+|$)(.*)", re.DOTALL)
SEPARATOR_CELL = re.compile(r"[ \t]*:?-{3,}:?[ \t]*")
CELL_BAR = re.compile(r"(?<!\\)\|")
HASH = re.compile(r"[0-9a-f]{40}|[0-9a-f]{64}")
STORE_PREFIX = "msw-floor-objects-"
# Dropped by neutral_git_env(): config and diff settings passed through the environment, and the
# repository-local variables (readiness/check.py's GIT_LOCAL_ENV), so no variable points git elsewhere.
DROPPED_ENV = (
    "GIT_CONFIG_PARAMETERS",
    "GIT_CONFIG_COUNT",
    "GIT_EXTERNAL_DIFF",
    "GIT_DIFF_OPTS",
    "GIT_REPLACE_REF_BASE",
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_CONFIG",
    "GIT_OBJECT_DIRECTORY",
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_IMPLICIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_PREFIX",
    "GIT_SHALLOW_FILE",
    "GIT_COMMON_DIR",
    "GIT_NAMESPACE",
)
DROPPED_ENV_PREFIXES = ("GIT_CONFIG_KEY_", "GIT_CONFIG_VALUE_")
FALSE_WORDS = ("false", "no", "off", "0")
FETCH_OPTIONS = (
    "fetch",
    "--quiet",
    "--no-tags",
    "--no-write-commit-graph",
    "--no-auto-gc",
    "--no-auto-maintenance",
)
# What serves the fetch, run by git through the shell: the transport drops repository-local variables, so
# these are set again for it, as shell assignments, which need no `env` on PATH.
UPLOAD_PACK = (
    f"GIT_GRAFT_FILE={os.devnull} GIT_NO_REPLACE_OBJECTS=1 GIT_NO_LAZY_FETCH=1 "
    "git -c core.commitGraph=false -c advice.graftFileDeprecated=false upload-pack"
)


class InputError(Exception):
    """Unusable input: exit 2, no verdict."""


class FootprintError(Exception):
    """The footprint rule is a hit."""


class DocumentError(Exception):
    """A document rule is a hit; the message is its reason."""


class ScreenError(Exception):
    """The screen record is not usable; the message is the dependency-screen rule's fixed reason."""


class RecordError(Exception):
    """A result record is not usable; the message is its fixed cause."""


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
        "screen": args.get("--screen"),
    }


# --- base-ref configuration ---------------------------------------------


def run(argv: list, data: bytes = None) -> bytes:
    try:
        proc = subprocess.run(argv, input=data, capture_output=True)
    except OSError as exc:
        raise InputError(f"cannot run {argv[0]}: {exc}") from exc
    if proc.returncode != 0:
        detail = proc.stderr.decode("utf-8", errors="replace").strip().splitlines()
        raise InputError(f"{command_name(argv)} failed: {detail[-1] if detail else f'exit {proc.returncode}'}")
    return proc.stdout


def command_name(argv: list) -> str:
    """The program and its first word after git's --git-dir and -c options, such as `git fetch`."""
    i = 1
    while i < len(argv) and argv[0] == "git" and (argv[i].startswith("--git-dir=") or argv[i] == "-c"):
        i += 1 if argv[i].startswith("--git-dir=") else 2
    return " ".join(argv[:1] + argv[i : i + 1])


def git_argv(git_dir, *args: str) -> list:
    """`git --git-dir=<git_dir> args...`, or `git args...` in the working directory when git_dir is None."""
    return ["git", f"--git-dir={git_dir}", *args] if git_dir is not None else ["git", *args]


# --- the object store ----------------------------------------------------


def neutral_git_env() -> None:
    """Keep system, global and environment config, the repository location, replace objects, grafts, the
    commit-graph and lazy fetches away from every git command this process runs."""
    for name in list(os.environ):
        if name in DROPPED_ENV or name.startswith(DROPPED_ENV_PREFIXES):
            del os.environ[name]
    os.environ.update(
        {
            "GIT_CONFIG_NOSYSTEM": "1",
            "GIT_CONFIG_GLOBAL": os.devnull,
            "GIT_GRAFT_FILE": os.devnull,
            "GIT_ATTR_NOSYSTEM": "1",
            "GIT_NO_REPLACE_OBJECTS": "1",
            "GIT_NO_LAZY_FETCH": "1",  # git 2.45 and later; older git ignores it
            "GIT_CONFIG_COUNT": "2",
            "GIT_CONFIG_KEY_0": "core.commitGraph",
            "GIT_CONFIG_VALUE_0": "false",
            # GIT_GRAFT_FILE names an existing file, so git would print its grafts deprecation hint first
            "GIT_CONFIG_KEY_1": "advice.graftFileDeprecated",
            "GIT_CONFIG_VALUE_1": "false",
        }
    )


def check_no_promisor() -> None:
    """Refuse a partial clone: a missing object would be fetched lazily, running whatever command its config
    names. git 2.44 makes a remote a promisor through extensions.partialClone, a true remote.<name>.promisor or
    any remote.<name>.partialCloneFilter; `git config --list` gives those names in lower case."""
    found = []
    for entry in run(["git", "config", "-z", "--list"]).decode("utf-8", errors="replace").split("\0"):
        key, newline, value = entry.partition("\n")
        if key == "extensions.partialclone":
            found.append(f"extensions.partialClone {value}")
        elif (
            key.startswith("remote.")
            and key.endswith(".promisor")
            and (not newline or value.lower() not in FALSE_WORDS)
        ):
            found.append(f"{key} {value or 'true'}")
        elif key.startswith("remote.") and key.endswith(".partialclonefilter"):
            found.append(f"{key} {value}")
    if found:
        raise InputError(f"the repository is a partial clone ({', '.join(found)}): its objects are not read")


def resolve_id(label: str, ref: str) -> str:
    """The full id of the object ref names in the job repository, which must hold it; not peeled."""
    try:
        proc = subprocess.run(
            ["git", "rev-parse", "--verify", "--quiet", "--end-of-options", f"{ref}^{{object}}"], capture_output=True
        )
    except OSError as exc:
        raise InputError(f"cannot run git: {exc}") from exc
    oid = proc.stdout.decode("ascii", errors="replace").strip()
    if proc.returncode != 0 or not HASH.fullmatch(oid):
        raise InputError(f"{label} ref {ref!r} does not resolve to a commit")
    return oid


def fetch(argv: list) -> None:
    """Run one step of filling the object store; a failure is the fetch's, with all of git's error output."""
    try:
        proc = subprocess.run(argv, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
    except OSError as exc:
        raise InputError(f"cannot run {argv[0]}: {exc}") from exc
    if proc.returncode != 0:
        lines = proc.stderr.decode("utf-8", errors="replace").splitlines()
        detail = "; ".join(line.strip() for line in lines if line.strip())  # all of it, on one line
        raise InputError(
            "the fetch from the job repository into a fresh object store failed "
            f"({command_name(argv)}, exit {proc.returncode})" + (f": {detail}" if detail else "")
        )


@contextlib.contextmanager
def object_store(refs: dict):
    """Yield (store, {label: commit}) for refs ({label: ref}): a fresh bare store in the system temp directory,
    filled by a by-hash fetch from the job repository that re-hashes every object, its closure from every
    fetched id then walked, and removed afterwards.

    Ref names are resolved in the job repository without peeling; the store peels each id to its commit.
    Run under neutral_git_env(); job_object_store() sees to that."""
    ids = {label: resolve_id(label, ref) for label, ref in refs.items()}
    common = run(["git", "rev-parse", "--path-format=absolute", "--git-common-dir"]).decode("utf-8")
    common = common.rstrip("\n")
    if not common.startswith("/"):
        raise InputError(f"the job repository's git directory {common!r} is not an absolute path")
    store = tempfile.mkdtemp(prefix=STORE_PREFIX)
    try:
        run(["git", "init", "--bare", "--quiet", "--template=", store])
        wanted = sorted(set(ids.values()))
        fetch(
            git_argv(
                store,
                "-c",
                "protocol.version=2",
                *FETCH_OPTIONS,
                f"--upload-pack={UPLOAD_PACK}",
                "file://" + urllib.parse.quote(common),
                *wanted,
            )
        )
        shallow = run(git_argv(store, "rev-parse", "--is-shallow-repository")).decode("ascii").strip()
        if shallow != "false":
            raise InputError("the fetch from the job repository gave a shallow history")
        # A .git/shallow in the job repository cuts what upload-pack sends without marking the store shallow,
        # so the store's closure from every fetched id is walked: a missing object fails the fetch.
        fetch(git_argv(store, "rev-list", "--objects", *wanted, "--"))
        commits = {}
        for label, oid in ids.items():
            try:
                commits[label] = resolve_base(oid, git_dir=store)
            except InputError as exc:
                raise InputError(f"{label} ref {refs[label]!r} does not resolve to a commit") from exc
        yield store, commits
    finally:
        shutil.rmtree(store, ignore_errors=True)


@contextlib.contextmanager
def job_object_store(refs: dict):
    """object_store(refs) under the neutral git environment, a partial clone refused before any fetch."""
    neutral_git_env()
    check_no_promisor()
    with object_store(refs) as result:
        yield result


def resolve_base(ref: str, git_dir=None) -> str:
    try:
        proc = subprocess.run(
            git_argv(git_dir, "rev-parse", "--verify", "--quiet", "--end-of-options", f"{ref}^{{commit}}"),
            capture_output=True,
        )
    except OSError as exc:
        raise InputError(f"cannot run git: {exc}") from exc
    commit = proc.stdout.decode("ascii", errors="replace").strip()
    if proc.returncode != 0 or not HASH.fullmatch(commit):
        raise InputError(f"base ref {ref!r} does not resolve to a commit")
    return commit


def load_config(commit: str, git_dir=None) -> dict:
    try:
        proc = subprocess.run(git_argv(git_dir, "show", f"{commit}:{CONFIG}"), capture_output=True)
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
    unknown = sorted(
        set(approval) - {"mode", "require_human", "limits", "min_confidence", "non_production", "limits_by_component"}
    )
    if unknown:
        raise InputError(f"config: unknown keys in pipeline.plan_approval: {', '.join(unknown)}")
    mode = approval.get("mode")
    if mode not in MODES:
        raise InputError(f"config: pipeline.plan_approval.mode must be one of {', '.join(MODES)}, not {mode!r}")

    min_confidence = approval.get("min_confidence", MIN_CONFIDENCE)
    if (
        not isinstance(min_confidence, int)
        or isinstance(min_confidence, bool)
        or not MIN_CONFIDENCE <= min_confidence <= 100
    ):
        raise InputError(
            f"config: pipeline.plan_approval.min_confidence must be an integer from {MIN_CONFIDENCE} to 100, "
            f"not {min_confidence!r}"
        )

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

    non_production = approval.get("non_production", list(NON_PRODUCTION))
    string_list(non_production, "pipeline.plan_approval.non_production", optional=False)

    by_component = approval.get("limits_by_component", {})
    if not isinstance(by_component, dict):
        raise InputError("config: pipeline.plan_approval.limits_by_component must be a mapping")
    for name, overrides in by_component.items():
        where = f"pipeline.plan_approval.limits_by_component.{name}"
        if name not in components:
            raise InputError(f"config: {where} names an undefined component")
        if not isinstance(overrides, dict):
            raise InputError(f"config: {where} must be a mapping")
        unknown = sorted(set(overrides) - set(LIMITS))
        if unknown:
            raise InputError(f"config: unknown keys in {where}: {', '.join(map(str, unknown))}")
        for key, value in overrides.items():
            if not isinstance(value, int) or isinstance(value, bool) or value < 0:
                raise InputError(f"config: {where}.{key} must be a non-negative integer, not {value!r}")

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
        directory = []
        for index, word in enumerate(words):
            while word.startswith("./"):
                word = word[2:]
            gate_words.append((name, word))
            if directory:
                gate_words.append((name, "/".join([*directory, word])))
            if index > 0 and words[index - 1] == "cd":
                directory += [segment for segment in word.split("/") if segment not in ("", ".")]

    if component not in components:
        raise InputError(f"--component {component!r} is not a component in the config")
    overrides = by_component.get(component, {})
    return {
        "paths": paths,
        "shared": shared,
        "mode": mode,
        "min_confidence": min_confidence,
        "rh_components": rh_components,
        "rh_paths": rh_paths,
        "limits": {key: overrides.get(key, limits[key]) for key in LIMITS},
        "limit_sources": {key: f"limits_by_component.{component}" for key in LIMITS if key in overrides},
        "non_production": list(non_production),
        "tests": tests,
        "gate_words": gate_words,
    }


def prepare(commit: str, component: str, plan: bytes, git_dir=None) -> tuple:
    """(config, cfg): the config at commit, validated for component. The plan is not read here."""
    config = load_config(commit, git_dir=git_dir)
    return config, validate_config(config, component)


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
    return "\n".join(lines[start + 1 : end])


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
    if tuple(footprint) not in (FOOTPRINT_KEYS, FOOTPRINT_KEYS_V2):
        raise FootprintError(
            f"footprint keys must be exactly {', '.join(FOOTPRINT_KEYS)}, in that order, "
            "optionally with production_lines right after estimated_lines"
        )
    lines_ = footprint["estimated_lines"]
    if not isinstance(lines_, int) or isinstance(lines_, bool) or lines_ < 0:
        raise FootprintError(f"estimated_lines must be an integer of 0 or more, not {lines_!r}")
    production = footprint.get("production_lines", lines_)
    if not isinstance(production, int) or isinstance(production, bool) or not 0 <= production <= lines_:
        raise FootprintError(f"production_lines must be an integer from 0 to estimated_lines, not {production!r}")
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
    if "production_lines" not in footprint:  # a seven-key footprint: every line is a production line
        footprint = {key: production if key == "production_lines" else footprint[key] for key in FOOTPRINT_KEYS_V2}
    return footprint


# --- rules ---------------------------------------------------------------


def parse_screen(text: str) -> dict:
    """The screen record in text; raises ScreenError with a fixed cause when it is not one this rule reads."""
    try:
        record = strict_json(text)
    except (ValueError, RecursionError) as exc:
        raise ScreenError("the screen record is not strict JSON") from exc
    if not isinstance(record, dict):
        raise ScreenError("the screen record is not a JSON object")
    if record.get("tool") != SCREEN_TOOL:
        raise ScreenError(f"the screen record's tool is not {SCREEN_TOOL}")
    if type(record.get("version")) is not int or record["version"] != SCREEN_VERSION:
        raise ScreenError(f"the screen record's version is not {SCREEN_VERSION}")
    if record.get("mode") != SCREEN_MODE:
        raise ScreenError(f"the screen record's mode is not {SCREEN_MODE}")
    packages = record.get("packages")
    if not isinstance(packages, list):
        raise ScreenError("the screen record's packages is not a list")
    for package in packages:
        if not isinstance(package, dict) or not all(
            isinstance(package.get(key), str) for key in ("name", "manifest", "reason")
        ):
            raise ScreenError("a package of the screen record has no string name, manifest or reason")
        if package.get("status") not in SCREEN_STATUSES:
            raise ScreenError("a package of the screen record has a status other than screened or unscreened")
        found = package.get("disqualifiers")
        if (
            not isinstance(found, list)
            or not all(isinstance(item, str) and item in DISQUALIFIERS for item in found)
            or len(set(found)) != len(found)
        ):
            raise ScreenError(
                "a package of the screen record has disqualifiers that are not unique values of "
                + ", ".join(DISQUALIFIERS)
            )
    return record


def screen_reasons(footprint: dict, text) -> list:
    """The dependency-screen rule's reasons; text is the screen record's, or None when none is given."""
    declared = [(dep["name"], dep["manifest"]) for dep in footprint["new_dependencies"]]
    if text is None:
        if not declared:
            return []
        count = len(declared)
        return [f"{count} declared package{'' if count == 1 else 's'} and no screen record (--screen)"]
    try:
        record = parse_screen(text)
    except ScreenError as exc:
        return [str(exc)]
    reasons = []
    remaining = list(declared)
    for package in record["packages"]:
        pair = (package["name"], package["manifest"])
        label = f"{flat(package['name'])} ({flat(package['manifest'])})"
        if pair not in remaining:
            reasons.append(f"the screen record lists {label}, which the footprint does not declare: unbound")
            continue
        remaining.remove(pair)
        if package["status"] == "unscreened":
            reasons.append(f"{label} is unscreened: {flat(package['reason']) or 'no reason given'}")
        version = package.get("version")
        version = flat(version) if isinstance(version, str) else "of an unknown version"
        for disqualifier in package["disqualifiers"]:
            reasons.append(
                f"{flat(package['name'])} {version} ({flat(package['manifest'])}) is disqualified: {disqualifier}"
            )
    for name, manifest in remaining:
        reasons.append(
            f"{flat(name)} ({flat(manifest)}) is declared but not screened: the screen record does not list it"
        )
    return reasons


def evaluate(footprint: dict, component: str, cfg: dict, screen=None) -> list:
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
    sources = cfg.get("limit_sources", {})
    split = any(matching(cfg["non_production"], path) for path in footprint["files"])
    lines = ("production_lines", footprint["production_lines"]) if split else ("lines", footprint["estimated_lines"])
    for name, key, label, value in (
        ("limit-files", "files", "files", len(footprint["files"])),
        ("limit-lines", "lines", lines[0], lines[1]),
        ("limit-new-files", "new_files", "new_files", len(footprint["new_files"])),
    ):
        source = f" ({sources[key]})" if key in sources else ""
        over = value > limits[key]
        rule(name, [f"{label} is {value}, over the limit of {limits[key]}{source}"] if over else [])

    rule("dependency-screen", screen_reasons(footprint, screen))

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

    rule(
        "overridden-assumptions",
        [
            f"overridden assumption: {item['assumption']} ({item['reason']})"
            for item in footprint["overridden_assumptions"]
        ],
    )
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
            if (
                run
                and run.group()[0] == fence[0]
                and len(run.group()) >= fence[1]
                and not stripped[len(run.group()) :].strip()
            ):
                fence = None
            continue
        if run:
            fence = (run.group()[0], len(run.group()))
        elif line.startswith("## "):
            found.append((i, 2))
        elif line.startswith("# "):
            found.append((i, 1))
    return found


def section(lines: list, title: str, what: str, ends: tuple = (1, 2)) -> tuple:
    """(index of the first body line, body lines) of the one `## <title>` section; raises DocumentError.

    The section runs to the next heading of a level in ends."""
    marks = headings(lines)
    starts = [i for i, level in marks if level == 2 and lines[i].rstrip() == f"## {title}"]
    if not starts:
        raise DocumentError(f"{what}: no '## {title}' section")
    if len(starts) > 1:
        raise DocumentError(f"{what}: {len(starts)} '## {title}' sections")
    start = starts[0]
    end = next((i for i, level in marks if i > start and level in ends), len(lines))
    return start + 1, lines[start + 1 : end]


def issue_body(text: str) -> list:
    """The issue document's lines before its first `## Comments` heading."""
    lines = document_lines(text)
    comments = [i for i, level in headings(lines) if level == 2 and lines[i].rstrip() == "## Comments"]
    return lines[: comments[0]] if comments else lines


def flat(text: str) -> str:
    """Document text on one line, cut to REASON_LIMIT characters."""
    return " ".join(text.split())[:REASON_LIMIT]


def record_path(path: str) -> Path:
    """The result record beside a document, named as save-output.py names it: `.md` replaced by `.result.json`."""
    document = Path(path)
    stem = document.name[:-3] if document.name.endswith(".md") else document.name
    return document.with_name(stem + ".result.json")


def read_record(path: str):
    """The bytes of the result record beside the document at path, or None when there is none."""
    record = record_path(path)
    try:
        data = record.read_bytes()
        data.decode("utf-8")
    except FileNotFoundError:
        return None
    except (OSError, UnicodeDecodeError) as exc:
        raise InputError(f"cannot read the result record {record}: {exc}") from exc
    return data


def record_questions(text: str) -> list:
    """The (kind, question) pairs of a result record; raises RecordError with a fixed cause when it is not usable."""
    try:
        record = strict_json(text)
    except (ValueError, RecursionError) as exc:
        raise RecordError("it is not strict JSON") from exc
    if not isinstance(record, dict):
        raise RecordError("it is not a JSON object")
    questions = record.get("questions")
    if not isinstance(questions, list):
        raise RecordError("its questions is not a list")
    pairs = []
    for question in questions:
        if not isinstance(question, dict):
            raise RecordError("a question is not a JSON object")
        if question.get("kind") not in QUESTION_KINDS:
            raise RecordError(f"a question's kind is not {', '.join(QUESTION_KINDS)}")
        if not isinstance(question.get("question"), str):
            raise RecordError("a question's question is not a string")
        pairs.append((question["kind"], question["question"]))
    return pairs


def question_reasons(text: str, what: str, record=None) -> list:
    """The blocking-questions reasons of one document; record is the text of its result record, or None."""
    reasons = []
    questions = None  # the record's (kind, question) pairs when it is usable
    if record is not None:
        try:
            questions = record_questions(record)
        except RecordError as exc:
            # The section-level rules still run; the bullets are neither hits nor compared.
            reasons.append(f"{what}: the result record is not usable: {exc}")
        else:
            for kind, question in questions:
                if kind != "assumption":
                    reasons.append(f"{what}: blocking question ({kind}): {flat(question)}")
    lines = document_lines(text)
    try:
        # Questions runs to the next `## ` heading, so a `# ` heading cannot end it early.
        start, body = section(lines, "Questions", what, ends=(2,))
    except DocumentError as exc:
        return reasons + [str(exc)]
    bullets = {"must_decide": 0, "recommended": 0, "blocking": 0, "assumption": 0}
    for offset, line in enumerate(body):
        number = start + offset + 1
        entry = line.lstrip(" \t")
        if not entry:
            continue
        indented = line[0] in " \t"
        marker = next(((m, kind) for m, kind in MARKERS if entry.startswith(m)), None)
        if marker is not None and (marker[0] in BLOCKING_MARKERS or not indented):
            # A blocking marker counts at column 0, indented, nested or after a tab; an assumption at column 0.
            if record is None and marker[0] in BLOCKING_MARKERS:  # the transition: Markdown alone
                reasons.append(f"{what}: blocking question: {flat(entry[len(marker[0]):])}")
            bullets[marker[1]] += 1
        elif indented:
            continue  # a continuation or a sub-bullet
        elif line.startswith("# "):
            reasons.append(f"{what}: line {number} of '## Questions' is a '# ' heading inside the section")
        else:
            reasons.append(f"{what}: line {number} of '## Questions' is not an entry with a known marker")
    if questions is not None:
        kinds = [kind for kind, _ in questions]
        m, r, a = (kinds.count(kind) for kind in QUESTION_KINDS)
        if not (
            bullets["assumption"] == a
            and bullets["must_decide"] <= m
            and bullets["recommended"] <= r
            and (m - bullets["must_decide"]) + (r - bullets["recommended"]) == bullets["blocking"]
        ):
            reasons.append(
                f"{what}: the '## Questions' bullets (must decide {bullets['must_decide']}, "
                f"recommended {bullets['recommended']}, blocking {bullets['blocking']}, "
                f"assumption {bullets['assumption']}) disagree with the result record "
                f"(must decide {m}, recommended {r}, assumption {a})"
            )
    return reasons


def issue_criteria(text: str) -> list:
    _, body = section(issue_body(text), "Acceptance criteria", "the issue document")
    criteria = []
    current = None
    gap = False  # a blank line since the item's last line
    for line in body:
        if not line.strip():
            gap = True
            continue
        item = ITEM.fullmatch(line)
        if item:
            rest = item.group(1)
            box = CHECKBOX.fullmatch(rest)
            current = [box.group(1) if box else rest]
            criteria.append(current)
        elif current is not None and (not gap or line[0] in " \t"):
            current.append(line)  # a wrapped line, or an indented later paragraph of the item
        else:
            current = None  # prose at column 0 after a blank line ends the item
        gap = False
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
    return found[0][len(marker) :].strip(), None


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
        reasons.append(
            f"first of its kind: no commit reachable from the base commit has a subject " f"scoped ({component})"
        )
    return reasons


def history(commit: str, git_dir=None) -> list:
    """The subjects of every commit reachable from commit, in git_dir (the object store) when given."""
    argv = git_argv(git_dir, "-c", "log.showSignature=false", "log", "-z", "--format=%s", commit, "--")
    try:
        proc = subprocess.run(argv, capture_output=True)
    except OSError as exc:
        raise InputError(f"cannot run git: {exc}") from exc
    if proc.returncode != 0:
        detail = proc.stderr.decode("utf-8", errors="replace").strip().splitlines()
        raise InputError(f"git log failed: {detail[-1] if detail else f'exit {proc.returncode}'}")
    return [s for s in proc.stdout.decode("utf-8", errors="replace").split("\0") if s]


def evaluate_documents(
    issue, analysis, plan_text: str, component: str, subjects: list, analysis_record=None, plan_record=None
) -> list:
    """The three document rules; issue and analysis are text, or None when not given, and the records the text of
    the analysis's and the plan's result records, or None when there is none."""
    results = []

    def rule(name: str, reasons: list) -> None:
        results.append({"rule": name, "result": "hit" if reasons else "clean", "reasons": reasons})

    if analysis is None:
        reasons = ["no analysis given (--analysis)"]
    else:
        reasons = question_reasons(analysis, "analysis", analysis_record)
    rule("blocking-questions", reasons + question_reasons(plan_text, "plan", plan_record))

    no_issue = ["no issue document given (--issue-document)"]
    rule("acceptance-criteria", no_issue if issue is None else criteria_reasons(issue, plan_text))
    rule("first-of-kind", no_issue if issue is None else first_of_kind_reasons(issue, component, subjects))
    return results


def conclude(
    plan_text: str,
    issue,
    analysis,
    component: str,
    cfg: dict,
    subjects: list,
    screen=None,
    analysis_record=None,
    plan_record=None,
) -> tuple:
    """What the floor concludes: (rules, not_evaluated, footprint or None); screen is the screen record's text,
    or None when none is given, and the records the result records' texts beside the analysis and the plan, or
    None when there is none."""
    try:
        footprint = parse_footprint(plan_text)
    except FootprintError as exc:
        footprint = None
        rules = [{"rule": "footprint", "result": "hit", "reasons": [str(exc)]}]
    else:
        rules = [{"rule": "footprint", "result": "clean", "reasons": []}]
        rules += evaluate(footprint, component, cfg, screen)
    rules += evaluate_documents(issue, analysis, plan_text, component, subjects, analysis_record, plan_record)
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
    with job_object_store({"base": args["base"]}) as (store, commits):
        commit = commits["base"]
        data = read_plan(args["plan"])
        _, cfg = prepare(commit, args["component"], data, git_dir=store)
        documents = {}
        for key, what in (("issue_document", "issue document"), ("analysis", "analysis"), ("screen", "screen record")):
            path = args[key]
            documents[key] = None if path is None else (path, read_document(path, what))
        records = {"analysis": None if args["analysis"] is None else read_record(args["analysis"])}
        records["plan"] = read_record(args["plan"])
        subjects = history(commit, git_dir=store)

    texts = {key: None if doc is None else doc[1].decode("utf-8") for key, doc in documents.items()}
    results, skipped, _ = conclude(
        data.decode("utf-8"),
        texts["issue_document"],
        texts["analysis"],
        args["component"],
        cfg,
        subjects,
        texts["screen"],
        *(None if records[key] is None else records[key].decode("utf-8") for key in ("analysis", "plan")),
    )
    hit = any(r["result"] == "hit" for r in results)
    bound = {
        key: None if doc is None else {"path": doc[0], "sha256": hashlib.sha256(doc[1]).hexdigest()}
        for key, doc in documents.items()
    }
    verdict = {
        "tool": TOOL,
        "version": VERSION,
        "plan": {"path": args["plan"], "sha256": hashlib.sha256(data).hexdigest()},
        "issue_document": bound["issue_document"],
        "analysis": bound["analysis"],
        "screen": bound["screen"],
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
