#!/usr/bin/env python3
"""Offline self-test of floor.py, decision.py and size.py, the scripts of plan approval.

    selftest.py

Needs `git` and `yq` on PATH; nothing else, and no network. It works in a
temporary directory with its own HOME, no system or global git config and a
fixed identity, and runs `git init` on a throwaway repository with no
remote. Base configs are commits there: `real`, the checkout's
.mintswarm/config.yaml byte for byte, and variants of it, read with
`yq -o=json`, changed here and committed as JSON text at the same path.
The commit messages are the fixture history the first-of-kind rule reads:
`nodocs`, the real config with no `(docs)` commit before it, then empty
commits with seed subjects for `docs`, `agents`, `tooling`, `web`, `api`
(with `!`), and a multi-scope, a revert and a differently cased subject,
then the variants, and `feat(cli): ...` after the `base` tag. floor.py and
decision.py run as `sys.executable SCRIPT ...` with that repository as its
working directory, and plans, issue documents, analyses and assessments
written under `cases/`; their TMPDIR is an empty directory of the sandbox,
where the floors make their object stores. Unless a case says otherwise,
floor.py gets a generated issue document for the case's component, whose criteria the
plan's Verification table quotes and whose comments hold a decoy criteria
section, and an analysis with no questions, and no --screen.

Checks:

- config: the checkout's config has no pipeline.approve_plan and has the
  expected pipeline.plan_approval;
- verdict shape, on every case that exits 0 or 1: the verdict's keys in
  order, version 3, the 16 rule identifiers in order across `rules` and
  `not_evaluated`, which is empty or the footprint rules after a `footprint`
  hit, results and reasons, `verdict` against the rules and the exit code,
  `plan`, `issue_document`, `analysis` and `screen` against the files given
  (null when not given),
  `base.commit`, `mode` and the printed lines;
- clean base: a footprint modelled on #20 is clean on every rule, also
  with a `## TL;DR` section after the summary line, and is accepted with
  `mode: always`, which the verdict records;
- single-rule hits: each rule hit by a case derived from the clean base,
  and each limit clean at its value and a hit one over; tests/** and
  xtask/** in the tooling component are clean on both require-human
  rules, and .github/workflows/**, also differently cased, is a
  `require-human-paths` hit; crates/rules/** and crates/github/** have a
  component, both in domain;
- footprint unusable: missing, misplaced, malformed and badly shaped
  footprints, and each bad path form, are `footprint` hits with the
  footprint rules not evaluated and the document rules evaluated;
- base ref: with a worktree config and a HEAD commit that drop a
  require_human path, `--base` at the earlier commit still gives the hit;
- historic fixtures: fixtures/plan-{9,12,14,19,20,10}.md, with a
  Verification table and Questions appended, against the real config give
  the expected 16 results and reasons, plan-10.md a `require-human-paths`
  hit naming AGENTS.md; at `nodocs`, plan-20.md and plan-10.md are also
  `first-of-kind` hits;
- blocking-questions: a blocking question in the analysis or the plan, at
  column 0, indented, tab-indented or nested, or after a `# ` heading
  inside Questions, with continuation lines named once, a variant marker,
  prose, a missing or repeated Questions section and no --analysis are
  hits; non-blocking entries only, and a Questions heading inside a fence,
  are clean; without a result record, the new Must decide and Recommended
  markers, at column 0 or indented and in the analysis or the plan, are
  hits naming the question, and Assumption entries only are clean; with a
  result record beside the analysis or the plan, each `must_decide` or
  `recommended` object is one hit naming it and an `assumption` none, with
  the new and with the old markers (an old Blocking bullet counting for
  either blocking kind), and the shared `needs-input` fixture gives its
  `q1`, `q3` and `q4`; records of assumptions only, matched by Assumption
  or by Non-blocking bullets, and an empty record with no bullets are
  clean; a missing bullet, an extra bullet, a swapped kind, a Blocking or
  indented Must decide bullet against no blocking object give the reason
  naming both sets of counts, an unknown column-0 marker the known-marker
  reason, and a missing Questions section its own reason besides the
  object hits; a record that is not strict JSON (truncated, a duplicate
  key, NaN), not an object, without a `questions` list, with a string
  question, an unknown kind or a question that is not a string is a hit
  naming its fixed cause, the section rules still applied;
- acceptance-criteria: a dropped, added, reworded or repeated criterion, a
  criterion's indented later paragraph left out, no criteria, no criteria
  section, two Verification sections, a table without its `---` row and no
  --issue-document are hits naming the criterion or the cause; whitespace
  differences, wrapped criteria, a later paragraph quoted, `\\|`, other
  list markers and task boxes, reordered rows, the decoy in comments and
  CRLF are clean;
- first-of-kind: `cli` at `real`, `core` (multi-scope only), `runner`
  (revert only) and `domain` (other case only) are hits, and so are an
  issue kind other than implementation, another component, a repeated or
  missing header line and no --issue-document; `cli` at `dropped`, `api`
  (`feat(api)!:`) and `docs` are clean;
- production lines: an eight-key footprint with a fixture among its files
  is measured by production_lines, clean at the lines limit and a hit one
  over, whatever estimated_lines; without a non-production file it is
  measured by estimated_lines; production_lines above estimated_lines,
  negative or `true` is a `footprint` hit;
- dependency screen, with records shaped as dependencies/screen.py writes
  them: no packages with no record or with an empty one, and a package
  screened without a disqualifier but with alarming evidence (a large size,
  900 transitive dependencies, a changed publisher, one day old) are clean;
  five packages, all screened, exit 0 with no `limit-new-dependencies` rule
  anywhere in the verdict; each of the four disqualifiers, two together, an
  `unscreened` package with its reason, packages with no --screen, a
  declared package the record does not list, a record package the
  footprint does not declare or declares in another manifest, a package
  listed twice, and a record that is not strict JSON, not an object, of
  another tool, `version` true or 2, `mode` review, `packages` not a list,
  or a package without a name, with another status, an unknown or a
  repeated disqualifier, are `dependency-screen` hits with their exact
  reasons; an unusable record is a hit without packages declared too;
  record text in a reason is one line, cut to 200 characters; and after a
  `footprint` hit, `dependency-screen` is not evaluated;
- per-component limits, at `component-limits` (docs lines 400): a docs plan
  of 350 lines is clean, of 401 a hit naming the override, a tooling plan
  of 350 lines a hit, and the docs plan's files limit stays 10;
- unusable input: bad arguments, plans, documents (missing, unreadable, not
  UTF-8, given twice or with an empty value), refs and configs (among them
  a `min_confidence` of 89, 101, `true` or `"95"`, a
  limits_by_component or non_production of the wrong shape, and the removed
  `new_dependencies` limit in `limits` or in limits_by_component), screen
  records (missing, unreadable, not UTF-8, given twice or with an empty
  value), result records beside the analysis or the plan (a directory, not
  UTF-8) and a missing `yq`, exit 2 and leave no verdict file, even a stale
  one;
- verdict errors: a verdict path that is a directory, and a read-only
  outputs directory with or without a stale verdict, exit 2 with a one-line
  error and no traceback;
- glob semantics: floor.py's glob_to_regex matches a pinned table of
  patterns by paths, and pinned edge cases;
- decision record shape, on every decision run that exits 0 or 1: the
  record's keys (version 2) and each layer's keys in order, the subject, the
  mode, `min_confidence` (90 unless the config sets it), the floor layer's
  rules equal to the verdict's, the confidence, band, hard triggers, doubts
  and reason consistent with each other and equal to the envelope's,
  harness and model equal to the arguments, `as_configured` against the
  config, `judged_decision` and `decision` recomputed from the layers, the
  threshold, `as_configured` and the mode, the judgement's reasons (its
  confidence and band, then the unusable triggers as `hard trigger`, the
  authority triggers as `requires the maintainer`, or, with no trigger, a
  confidence below `min_confidence`), the exit code, and stdout: the
  auto-approval comment for `auto`, the shadow-record comment in `shadow`
  mode, empty otherwise;
- bands under `judged` with the default threshold of 90: `blocked` 0 with
  each unusable hard trigger, `uncertain` 1 and 49, `needs-review` 50
  and 79 and `likely-safe` 80 and 89 are `human`; `likely-safe` 90 and 94
  and `safe` 95 and 100 are `auto`; a floor hit with `safe` 100 is `human`;
  each authority trigger, and all four together, at `safe` 100 with a clean
  floor are `human` with a usable judgement and the reason `requires the
  maintainer`, and `weakens-safeguard` at `needs-review` 62 is `human`;
  each unusable trigger at `safe` 100 and `needs-review` 60, and `blocked`
  with only `credentials`, are unusable, and `blocked` with
  `criterion-not-met` and `credentials` is usable; under `never`, `blocked`
  with an unusable and an authority trigger shows the unusable one in the
  Hard triggers row and the authority one in the Confidence row;
  AUTHORITY_TRIGGERS here are decision.py's and a subset of HARD_TRIGGERS;
- thresholds: at `min_confidence` 95, 94 is `human` and 95 `auto`; at 100,
  99 is `human` and 100 `auto`;
- malformed envelopes, each `human` with the judgement unusable: a missing
  or non-UTF-8 assessment, no envelope, two envelopes, an unclosed or a
  four-backtick fence, not JSON, a lone surrogate escape in the reason or a
  doubt, a list, a duplicate key, NaN, a missing,
  extra or reordered key, a confidence that is a boolean, float or string or
  is -1 or 101, an unknown band, a range (`80-94`) or a display name
  (`Likely safe`) as the band, a confidence outside its band's range,
  `blocked` without a trigger, an unusable trigger outside `blocked`, an
  unknown or repeated trigger, triggers that are not a list, an empty,
  format-only or non-string reason, a doubt missing a field or with an
  empty one, 11
  doubts, `safe` with a doubt, `likely-safe` without one, a `Risk:` line, a
  summary confidence that differs from the envelope's, a summary in the old
  risk form, without a reason or missing, and no title; trailing spaces on
  the fence lines and CRLF still give `auto`;
- decision under `judged`: a floor hit, a missing or unparsable verdict, a
  verdict edited to break each
  binding (the documents' sha256 too), a verdict made without the documents
  or with another issue document or analysis, a forged verdict that is
  consistent in itself but differs from what floor.py gives for the plan
  and documents (a `first-of-kind` hit set clean) are `human`, with the
  floor layer unusable; a harness or model other than the config's
  sets `as_configured` false and gives `human`; a `blocking-questions` hit
  and a `footprint` hit give a usable floor layer equal to the verdict; with
  result records beside the analysis and the plan, a `must_decide` object
  gives a usable floor layer holding its hit and records of assumptions
  only give `auto`, while a record rewritten after the floor ran makes the
  floor layer unusable;
- decision under `always`, `shadow` and `never`: `human`, `human` with
  `judged_decision` recorded, and `auto` whatever the layers say;
- the comments: the auto-approval comment's heading, floor line,
  Confidence row (the band's display name and score, the authority triggers
  listed, the threshold and the reason), Hard triggers row, one Doubt row
  per doubt and veto paragraph; the shadow-record comment's heading and
  rows, also for an unusable
  judgement; and in both, model and session text, a hostile reason and
  doubt among it, only inside code spans of at most 600 characters, a long
  doubt's and reason's marked `… (truncated)`;
- decision unusable input: bad arguments, plans, refs and configs, a
  missing, unreadable or non-UTF-8 issue document, analysis or screen
  record, a screen record given twice or with an empty value, a missing
  `yq` and a record path that is a directory exit 2, print nothing
  to stdout and leave no record, even a stale one;
- decision and the screen: in `shadow` mode a two-package record gives a
  `dependency-screen` hit and, right after the Floor row, one Package row
  per declared package with its version, status, disqualifiers (advisory
  ids, the name a typosquat imitates) and evidence, and the footprint in
  brief counts declared packages; a hostile package name and version stay
  inside code spans; under `judged` a screened package with the matching
  --screen gives a usable, clean floor layer, `auto` and its Package row in
  the auto-approval comment; a verdict made without --screen but decided
  with it, made with it but decided without, or decided with another
  record give `human`, the floor layer unusable and only the screen binding
  named;
- object store, with every run's TMPDIR an empty sandbox directory: a git
  on PATH that logs its arguments shows the store made there and the base,
  the config and the history read through `--git-dir` at it; a replace ref
  on the base (a looser config), info/grafts, GIT_GRAFT_FILE,
  `git replace --graft`, GIT_REPLACE_REF_BASE and a commit-graph patched in
  its CDAT chunk to give the base no parents (cutting off its only `(cli)`
  commit) each fool a plain git command and leave floor.py's verdict the
  untampered one, and so do GIT_DIR and GIT_OBJECT_DIRECTORY at a decoy
  repository; a loose config blob of the base overwritten with another
  blob, which a plain `git show` then shows, makes floor.py and decision.py
  exit 2 with one line naming the fetch and no verdict or record, even a
  stale one; a .git/shallow forged in the job repository below the base's
  `(cli)` commit makes floor.py exit 2 with one line naming the fetch and no
  verdict; a copy of the repository at a path holding `%41`, next to a
  decoy repository at the path git would decode that to, gives the
  untampered verdict; a repository with extensions.partialClone exits 2; and no
  `msw-floor-objects-*` store is left after clean and exit 2 runs;
- sizes, of size.py: the lines floor at every boundary of design §5's
  bands; in `plan` mode, a plan judged S with production_lines 300 prints
  `size: L` and the raised-size line, one judged XL or L only its size, a
  seven-key footprint's estimated_lines raises the same, and a missing or
  malformed `## Size` (no section, two, one only inside a fence, the wrong
  position, `size: l`, `Size: L`, `size: XXL`, trailing text, a leading
  space, no or four reasons, an empty bullet, prose, a continuation line,
  a nested bullet, a fence) prints the lines floor and the no-usable-size
  line; in `analysis` mode the same sections exit 1, and so do a json
  fence in the section and a plan's Size after `## Footprint`; blank lines
  between reasons, CRLF, one or three reasons, trailing spaces and tabs on
  the heading, size line and reasons, and a section ended right by the next
  heading are usable in both; bad arguments, an unknown mode, a
  missing or non-UTF-8 file, an unusable footprint and a json fence in a
  plan's Size exit 2 with nothing on stdout; and the clean base with a
  `## Size` section after the footprint is clean on every floor rule;
- standard library: every import of floor.py, decision.py, size.py and
  selftest.py is in the standard library.

Prints one `ok` or `FAIL` line per check and exits 0 only when all passed.
"""

import ast
import copy
import hashlib
import importlib.util
import json
import os
import re
import shlex
import shutil
import subprocess
import sys
import tempfile
import unicodedata
import zlib
from pathlib import Path

HERE = Path(__file__).resolve().parent
FLOOR = HERE / "floor.py"
DECISION = HERE / "decision.py"
SIZE = HERE / "size.py"
FIXTURES = HERE / "fixtures"
ROOT = HERE.parents[2]
CONFIG = ROOT / ".mintswarm" / "config.yaml"
CONFIG_PATH = ".mintswarm/config.yaml"
VERDICT = Path(".mintswarm/job/outputs/floor.json")
RECORD = Path(".mintswarm/job/outputs/decision.json")

RULES = [
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
]
FOOTPRINT_RULES = RULES[1:13]
DOCUMENT_RULES = RULES[13:]
VERDICT_KEYS = [
    "tool",
    "version",
    "plan",
    "issue_document",
    "analysis",
    "screen",
    "component",
    "base",
    "config",
    "mode",
    "verdict",
    "rules",
    "not_evaluated",
]
SEEDS = [
    "docs(docs): seed",
    "feat(agents): seed",
    "chore(tooling): seed",
    "feat(web): seed",
    "feat(api)!: seed",
    "feat(core,web): seed",
    'Revert "feat(runner): seed"',
    "feat(Domain): seed",
]
CRITERIA = [
    "The floor reads the issue document and the analysis.",
    "A reason names the criterion it is about.",
    "A cell may quote `a | b` with its bar escaped.",
]
EXPECTED_APPROVAL = {
    "mode": "shadow",
    "require_human": {
        "components": ["agents"],
        "paths": [
            ".mintswarm/config.yaml",
            ".mintswarm/dogfood/github/**",
            "AGENTS.md",
            "CLAUDE.md",
            ".github/workflows/**",
        ],
    },
    "limits": {"files": 10, "lines": 300, "new_files": 3},
    "limits_by_component": {
        "domain": {"lines": 2000, "files": 15, "new_files": 12},
        "runner": {"lines": 2000, "files": 15, "new_files": 12},
    },
    "non_production": [
        "**/tests/**",
        "**/*selftest*",
        "**/test_*.py",
        "**/*_test.*",
        "**/*.test.*",
        "**/*.spec.*",
        "**/fixtures/**",
        "**/Cargo.lock",
        "**/pnpm-lock.yaml",
        ".claude/agents/msw-*.md",
    ],
}
CLEAN_BASE = {
    "files": ["CONTRIBUTING.md", "docs/architecture/repository-layout.md"],
    "new_files": [],
    "estimated_lines": 6,
    "new_dependencies": [],
    "external_steps": [],
    "gate_changes": [],
    "overridden_assumptions": [],
}
TLDR = (
    "## TL;DR\n\n"
    "- Changes: `CONTRIBUTING.md` and `docs/architecture/repository-layout.md`.\n"
    "- Approach: one step with files: as declared, documenting the layout.\n"
    "- Left out: no code, no gate and no change to `.mintswarm/config.yaml`.\n"
    "- Maintainer: approval only.\n\n"
)
HISTORIC = {
    "plan-9.md": (
        "agents",
        {
            "require-human-components",
            "limit-files",
            "limit-lines",
            "gate-changes",
        },
    ),
    "plan-12.md": (
        "agents",
        {
            "require-human-paths",
            "require-human-components",
            "limit-lines",
            "gate-changes",
        },
    ),
    "plan-14.md": (
        "agents",
        {
            "require-human-paths",
            "require-human-components",
            "limit-files",
            "limit-lines",
            "limit-new-files",
            "external-steps",
            "gate-changes",
        },
    ),
    "plan-19.md": ("tooling", {"external-steps"}),
    "plan-20.md": ("docs", set()),
    "plan-10.md": ("docs", {"require-human-paths"}),
}
DEFAULT = object()  # Runner.run: generate the document


class SelfTest:
    def __init__(self):
        self.failed = 0
        self.passed = 0

    def check(self, name: str, condition: bool, detail: str = "") -> bool:
        if condition:
            self.passed += 1
            print(f"ok   {name}")
        else:
            self.failed += 1
            print(f"FAIL {name}" + (f": {detail}" if detail else ""))
        return condition


T = SelfTest()


# --- sandbox --------------------------------------------------------------


class Sandbox:
    def __init__(self, tmp: Path):
        self.tmp = tmp
        self.repo = tmp / "repo"
        self.cases = self.repo / "cases"
        home = tmp / "home"
        home.mkdir()
        env = {k: v for k, v in os.environ.items() if not k.startswith("GIT_")}
        env.update(
            {
                "HOME": str(home),
                "GIT_CONFIG_NOSYSTEM": "1",
                "GIT_CONFIG_GLOBAL": os.devnull,
                "GIT_AUTHOR_NAME": "floor selftest",
                "GIT_AUTHOR_EMAIL": "selftest@example.invalid",
                "GIT_COMMITTER_NAME": "floor selftest",
                "GIT_COMMITTER_EMAIL": "selftest@example.invalid",
                "GIT_AUTHOR_DATE": "2026-01-01T00:00:00Z",
                "GIT_COMMITTER_DATE": "2026-01-01T00:00:00Z",
            }
        )
        self.store_tmp = tmp / "tmpdir"  # TMPDIR of every run: the floors' object stores go here
        self.store_tmp.mkdir()
        env["TMPDIR"] = str(self.store_tmp)
        self.env = env
        self.repo.mkdir()
        self.cases.mkdir()
        self.git("init", "-q", "-b", "master")
        self.counter = 0

    def git(self, *args: str) -> str:
        proc = subprocess.run(["git", *args], cwd=self.repo, env=self.env, capture_output=True, text=True)
        if proc.returncode != 0:
            raise RuntimeError(f"git {' '.join(args)} failed: {proc.stderr.strip()}")
        return proc.stdout.strip()

    def commit_config(self, data: bytes, message: str) -> str:
        path = self.repo / CONFIG_PATH
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(data)
        self.git("add", CONFIG_PATH)
        self.git("commit", "-q", "--allow-empty", "-m", message)
        return self.git("rev-parse", "HEAD")

    def commit_empty(self, message: str) -> str:
        self.git("commit", "-q", "--allow-empty", "-m", message)
        return self.git("rev-parse", "HEAD")

    def write_plan(self, name: str, content) -> str:
        self.counter += 1
        rel = f"cases/{self.counter:03d}-{name.replace(' ', '-').replace('/', '_')[:40]}.md"
        data = content.encode("utf-8") if isinstance(content, str) else content
        (self.repo / rel).write_bytes(data)
        return rel

    def exec(self, args: list, env: dict = None, script: Path = FLOOR):
        return subprocess.run(
            [sys.executable, str(script), *args],
            cwd=self.repo,
            env=env or self.env,
            capture_output=True,
            text=True,
        )

    def floor(self, args: list, env: dict = None):
        stale = self.repo / VERDICT
        stale.parent.mkdir(parents=True, exist_ok=True)
        stale.write_text('{"stale": true}\n', encoding="utf-8")
        proc = self.exec(args, env)
        verdict = None
        if stale.exists():
            try:
                verdict = json.loads(stale.read_text(encoding="utf-8"))
            except ValueError:
                verdict = "unparsable"
        return proc, verdict


def yq_json(path: Path) -> dict:
    proc = subprocess.run(["yq", "-o=json", ".", str(path)], capture_output=True, text=True)
    if proc.returncode != 0:
        raise RuntimeError(f"yq failed on {path}: {proc.stderr.strip()}")
    return json.loads(proc.stdout)


def as_json(config: dict) -> bytes:
    return (json.dumps(config, indent=2) + "\n").encode("utf-8")


# --- plans ----------------------------------------------------------------


def footprint_block(footprint: dict) -> str:
    return json.dumps({"footprint": footprint}, indent=2)


def plan_tail(rows: list = None, questions: str = "", extra: str = "") -> str:
    """The plan's Verification table, one row per criterion (bars escaped), and its Questions."""
    rows = [c.replace("|", "\\|") for c in CRITERIA] if rows is None else rows
    table = "".join(f"| {row} | checked by selftest.py |\n" for row in rows)
    return (
        f"\n## Verification\n\n| Acceptance criterion | Test or check |\n| --- | --- |\n{table}{extra}\n"
        f"## Questions\n\n{questions}"
    )


def plan_text(
    block: str = None,
    footprint: dict = None,
    approach: str = "",
    between: str = "",
    section: str = "## Footprint\n\n",
    opener: str = "```json",
    closer: str = "```\n",
    after: str = "",
    tail: str = None,
    tldr: str = "",
) -> str:
    if block is None:
        block = footprint_block(footprint if footprint is not None else CLEAN_BASE)
    return (
        "# Plan: selftest case\n\nGenerated by selftest.py.\n\n"
        f"{tldr}## Approach\n\nA generated case.\n{approach}\n"
        "## Steps\n\n1. **Change** — files: as declared.\n\n"
        f"{between}{section}{opener}\n{block}\n{closer}{after}"
        f"{plan_tail() if tail is None else tail}"
    )


def criteria_list(criteria: list = None) -> str:
    return "".join(f"{i}. {c}\n" for i, c in enumerate(CRITERIA if criteria is None else criteria, 1))


def issue_document(
    component: str = "docs", kind: str = "implementation", criteria: str = None, header: str = None
) -> str:
    """An issue document as the session writes it, with a decoy criteria section in its comments."""
    if header is None:
        header = f"- **Issue:** #99\n- **Kind:** {kind}\n- **Component:** {component}\n- **Labels:** mintswarm\n"
    return (
        f"# Selftest issue\n\n{header}\n"
        "## Problem\n\nA generated issue.\n\n"
        f"## Acceptance criteria\n\n{criteria_list() if criteria is None else criteria}\n"
        "## Comments\n\n### someone, 2026-01-01\n\nA comment with its own heading.\n\n"
        "## Acceptance criteria\n\n1. A decoy criterion that only a comment states.\n"
    )


def analysis_document(questions: str = "", heading: str = "## Questions") -> str:
    return (
        "# Analysis: selftest case\n\nGenerated by selftest.py.\n\n"
        "## What is asked\n\nA generated case.\n\n"
        f"{heading}\n\n{questions}"
    )


def result_record(questions: list) -> str:
    """A result record as save-output.py writes it, with one question object per (kind, question) pair."""
    objects = [
        {
            "id": f"q{i}",
            "kind": kind,
            "question": question,
            "why": "A generated case.",
            "shape": "yes_no",
            "options": [],
            "recommendation": None,
            "references": [],
        }
        for i, (kind, question) in enumerate(questions, 1)
    ]
    status = "complete" if all(kind == "assumption" for kind, _ in questions) else "needs_input"
    record = {
        "status": status,
        "artefact": "analysis",
        "outputs": [".mintswarm/job/outputs/analysis.md"],
        "questions": objects,
        "reason": "",
        "commit": None,
        "pr_title": None,
    }
    return json.dumps(record, indent=2) + "\n"


def write_record(document: Path, content) -> Path:
    """Write the result record beside document, named as save-output.py names it; content is text or bytes."""
    path = document.with_name(document.name[: -len(".md")] + ".result.json")
    path.write_bytes(content.encode("utf-8") if isinstance(content, str) else content)
    return path


def historic(fixture: str) -> bytes:
    """A historic fixture with the Verification table and Questions the plan stage now adds."""
    return (FIXTURES / fixture).read_bytes() + plan_tail().encode("utf-8")


def base(**changes) -> dict:
    footprint = copy.deepcopy(CLEAN_BASE)
    footprint.update(changes)
    return footprint


def replaced(text: str, old: str, new: str) -> str:
    if old not in text:
        raise RuntimeError(f"selftest bug: {old!r} not in the generated block")
    return text.replace(old, new, 1)


# --- verdict checks -------------------------------------------------------


def check_shape(name: str, proc, verdict, args: dict, commit: str, mode: str) -> bool:
    problems = []
    if not isinstance(verdict, dict):
        return T.check(
            f"{name}: verdict shape", False, f"no verdict file (exit {proc.returncode}): {proc.stderr.strip()}"
        )
    if list(verdict) != VERDICT_KEYS:
        problems.append(f"keys {list(verdict)}")
    rules = verdict.get("rules", [])
    skipped = verdict.get("not_evaluated", [])
    names = [r.get("rule") for r in rules]
    if skipped not in ([], FOOTPRINT_RULES) or names != [n for n in RULES if n not in skipped]:
        problems.append(f"rule identifiers not the {len(RULES)} in order, with none or the footprint rules skipped")
    elif skipped and rules[0].get("result") != "hit":
        problems.append("footprint rules skipped without a footprint hit")
    for r in rules:
        if list(r) != ["rule", "result", "reasons"]:
            problems.append(f"rule keys {list(r)}")
        elif r["result"] not in ("hit", "clean"):
            problems.append(f"{r['rule']}: result {r['result']!r}")
        elif not isinstance(r["reasons"], list) or not all(isinstance(x, str) for x in r["reasons"]):
            problems.append(f"{r['rule']}: reasons not strings")
        elif (r["result"] == "hit") != bool(r["reasons"]):
            problems.append(f"{r['rule']}: {r['result']} with {len(r['reasons'])} reasons")
    hit = any(r.get("result") == "hit" for r in rules)
    if verdict.get("verdict") != ("hit" if hit else "clean"):
        problems.append(f"verdict {verdict.get('verdict')!r} against the rules")
    if proc.returncode != (1 if hit else 0):
        problems.append(f"exit {proc.returncode} with verdict {verdict.get('verdict')!r}")
    if verdict.get("tool") != "mintswarm-plan-floor" or verdict.get("version") != 3:
        problems.append("tool or version")
    for key in ("plan", "issue_document", "analysis", "screen"):
        path = args.get(key)
        bound = (
            None
            if path is None
            else {"path": path, "sha256": hashlib.sha256((args["cwd"] / path).read_bytes()).hexdigest()}
        )
        if verdict.get(key) != bound:
            problems.append(f"{key} {verdict.get(key)}, expected {bound}")
    if verdict.get("component") != args["component"]:
        problems.append("component")
    if verdict.get("base") != {"ref": args["base"], "commit": commit}:
        problems.append(f"base {verdict.get('base')}, expected commit {commit}")
    if verdict.get("config") != CONFIG_PATH:
        problems.append("config")
    if verdict.get("mode") != mode:
        problems.append(f"mode {verdict.get('mode')!r}, expected {mode!r}")
    lines = proc.stdout.splitlines()
    expected = []
    for r in rules:
        line = f"{r.get('result')!s:7} {r.get('rule')}"
        if r.get("reasons"):
            line += f": {r['reasons'][0]}"
        expected.append(line)
    expected.append(f"floor: {verdict.get('verdict')}")
    if lines != expected:
        problems.append(f"stdout {lines!r}")
    return T.check(f"{name}: verdict shape", not problems, "; ".join(problems))


class Runner:
    def __init__(self, sb: Sandbox, commits: dict):
        self.sb = sb
        self.commits = commits

    def run(
        self,
        name: str,
        content,
        component: str = "docs",
        config: str = "real",
        ref: str = None,
        commit: str = None,
        mode: str = None,
        issue=DEFAULT,
        analysis=DEFAULT,
        screen=None,
        analysis_record=None,
        plan_record=None,
    ):
        """issue and analysis: DEFAULT (generated, clean), None (not given), or the content to give; screen:
        None (not given) or the content of the screen record to give; analysis_record and plan_record: None (no
        record) or the content of the result record written beside the analysis or the plan."""
        mode = mode or EXPECTED_APPROVAL["mode"]
        plan = self.sb.write_plan(name, content)
        ref = ref or self.commits[config]
        commit = commit or self.commits[config]
        args = ["--plan", plan, "--component", component, "--base", ref]
        documents = {}
        for key, flag, value, generated in (
            ("issue_document", "--issue-document", issue, lambda: issue_document(component=component)),
            ("analysis", "--analysis", analysis, analysis_document),
        ):
            if value is None:
                documents[key] = None
                continue
            documents[key] = self.sb.write_plan(f"{name} {key}", generated() if value is DEFAULT else value)
            args += [flag, documents[key]]
        for path, record in ((documents["analysis"], analysis_record), (plan, plan_record)):
            if record is not None:
                write_record(self.sb.repo / path, record)
        documents["screen"] = None
        if screen is not None:
            documents["screen"] = self.sb.write_plan(f"{name} screen", screen)
            args += ["--screen", documents["screen"]]
        proc, verdict = self.sb.floor(args)
        check_shape(
            name,
            proc,
            verdict,
            {"cwd": self.sb.repo, "plan": plan, "component": component, "base": ref, **documents},
            commit,
            mode,
        )
        return proc, verdict


def results_of(verdict) -> dict:
    if not isinstance(verdict, dict):
        return {}
    return {r["rule"]: r["result"] for r in verdict.get("rules", [])}


def reasons_of(verdict, rule: str) -> list:
    if not isinstance(verdict, dict):
        return []
    for r in verdict.get("rules", []):
        if r["rule"] == rule:
            return r["reasons"]
    return []


def expect_hit(
    r: Runner,
    name: str,
    footprint: dict,
    rule: str,
    only: bool = False,
    reason: str = None,
    reasons: list = None,
    tail: str = None,
    **kwargs,
):
    """reason: a text some reason holds; reasons: the rule's exact reasons."""
    proc, verdict = r.run(name, plan_text(footprint=footprint, tail=tail), **kwargs)
    results = results_of(verdict)
    ok = proc.returncode == 1 and results.get(rule) == "hit"
    detail = f"exit {proc.returncode}, {rule} {results.get(rule)!r}: {reasons_of(verdict, rule)}"
    if ok and only:
        others = [k for k, v in results.items() if v == "hit" and k != rule]
        ok = not others
        detail = f"other hits {others}"
    if ok and reason is not None:
        ok = any(reason in x for x in reasons_of(verdict, rule))
        detail = f"no reason naming {reason!r}: {reasons_of(verdict, rule)}"
    if ok and reasons is not None:
        ok = reasons_of(verdict, rule) == reasons
        detail = f"reasons {reasons_of(verdict, rule)}, expected {reasons}"
    T.check(f"{name}: {rule} hit", ok, detail)
    return verdict


def expect_clean(r: Runner, name: str, footprint: dict, rules: list, tail: str = None, **kwargs):
    proc, verdict = r.run(name, plan_text(footprint=footprint, tail=tail), **kwargs)
    results = results_of(verdict)
    bad = [rule for rule in rules if results.get(rule) != "clean"]
    T.check(
        f"{name}: {', '.join(rules)} clean",
        proc.returncode in (0, 1) and not bad,
        f"not clean: {bad}: {[reasons_of(verdict, rule) for rule in bad]}",
    )
    return verdict


# --- check groups ---------------------------------------------------------


def check_config() -> None:
    config = yq_json(CONFIG)
    pipeline = config.get("pipeline") or {}
    T.check("config: no pipeline.approve_plan", "approve_plan" not in pipeline)
    T.check(
        "config: pipeline.plan_approval values",
        pipeline.get("plan_approval") == EXPECTED_APPROVAL,
        json.dumps(pipeline.get("plan_approval")),
    )


def check_clean_base(r: Runner) -> None:
    proc, verdict = r.run("clean base", plan_text())
    T.check(
        "clean base: exit 0, every rule clean",
        proc.returncode == 0
        and set(results_of(verdict).values()) == {"clean"}
        and len(results_of(verdict)) == len(RULES),
        proc.stdout,
    )
    proc, verdict = r.run("clean base with a TL;DR", plan_text(tldr=TLDR))
    T.check(
        "clean base with a TL;DR: exit 0, every rule clean",
        proc.returncode == 0
        and set(results_of(verdict).values()) == {"clean"}
        and len(results_of(verdict)) == len(RULES),
        proc.stdout,
    )
    proc, verdict = r.run("always mode", plan_text(), config="always", mode="always")
    T.check(
        "always mode: accepted, rules evaluated, mode recorded",
        proc.returncode == 0
        and len(results_of(verdict)) == len(RULES)
        and isinstance(verdict, dict)
        and verdict.get("mode") == "always",
        f"exit {proc.returncode}: {proc.stderr.strip()}",
    )


def check_single_rules(r: Runner, limits: dict) -> None:
    expect_hit(r, "CLAUDE.md", base(files=["CLAUDE.md"]), "require-human-paths", only=True, reason="CLAUDE.md")
    expect_hit(r, "agents.md", base(files=["agents.md"]), "require-human-paths", reason="AGENTS.md")
    human_rules = ["require-human-paths", "require-human-components"]
    expect_clean(r, "tests/e2e.rs", base(files=["tests/e2e.rs"]), human_rules, component="tooling")
    expect_clean(r, "xtask/src/main.rs", base(files=["xtask/src/main.rs"]), human_rules, component="tooling")
    expect_hit(
        r,
        ".github/workflows/ci.yml",
        base(files=[".github/workflows/ci.yml"]),
        "require-human-paths",
        reason=".github/workflows/**",
        component="tooling",
    )
    expect_clean(
        r,
        ".github/workflows/ci.yml component",
        base(files=[".github/workflows/ci.yml"]),
        ["require-human-components"],
        component="tooling",
    )
    expect_hit(r, "web/index.html", base(files=["web/index.html"]), "component-scope")
    expect_clean(r, "Cargo.toml", base(files=["Cargo.toml"]), ["component-scope", "no-component"])
    expect_clean(
        r,
        "crates/rules and crates/github",
        base(files=["crates/rules/src/lib.rs", "crates/github/src/lib.rs"]),
        ["no-component"],
    )
    scope_rules = ["component-scope", "no-component"]
    expect_clean(r, "crates/rules/src/lib.rs", base(files=["crates/rules/src/lib.rs"]), scope_rules, component="domain")
    expect_clean(
        r, "crates/github/src/lib.rs", base(files=["crates/github/src/lib.rs"]), scope_rules, component="domain"
    )
    expect_clean(r, "LICENSE.md", base(files=["LICENSE.md"]), ["component-scope", "no-component"])
    expect_hit(r, "Makefile", base(files=["Makefile"]), "no-component")

    def docs_files(n: int) -> list:
        return [f"docs/limit-{i}.md" for i in range(n)]

    n = limits["files"]
    expect_clean(r, f"files at {n}", base(files=docs_files(n)), ["limit-files"])
    expect_hit(r, f"files at {n + 1}", base(files=docs_files(n + 1)), "limit-files", only=True)
    n = limits["lines"]
    expect_clean(r, f"lines at {n}", base(estimated_lines=n), ["limit-lines"])
    expect_hit(r, f"lines at {n + 1}", base(estimated_lines=n + 1), "limit-lines", only=True)
    n = limits["new_files"]
    expect_clean(r, f"new files at {n}", base(files=docs_files(n), new_files=docs_files(n)), ["limit-new-files"])
    expect_hit(
        r,
        f"new files at {n + 1}",
        base(files=docs_files(n + 1), new_files=docs_files(n + 1)),
        "limit-new-files",
        only=True,
    )

    expect_hit(
        r,
        "external step",
        base(external_steps=["the maintainer flips a setting"]),
        "external-steps",
        only=True,
        reason="flips a setting",
    )
    expect_hit(
        r,
        "gate_changes entry",
        base(gate_changes=["whitespace: new base"]),
        "gate-changes",
        only=True,
        reason="whitespace: new base",
    )
    expect_hit(
        r,
        "render-agents.py",
        base(files=[".mintswarm/dogfood/render-agents.py"]),
        "gate-changes",
        reason="agents-drift",
        component="agents",
    )
    expect_hit(r, "config.yaml", base(files=[".mintswarm/config.yaml"]), "gate-changes", reason="gates.run")
    expect_hit(
        r,
        ".GitHub/Workflows/CI.yml",
        base(files=[".GitHub/Workflows/CI.yml"]),
        "require-human-paths",
        reason=".github/workflows/**",
    )
    expect_hit(r, "Config.yaml case", base(files=[".MintSwarm/Config.yaml"]), "gate-changes", reason="gates.run")
    expect_hit(
        r,
        "Render-Agents.py case",
        base(files=[".mintswarm/dogfood/Render-Agents.py"]),
        "gate-changes",
        reason="agents-drift",
        component="agents",
    )
    expect_hit(
        r, "tests none", base(files=["web/index.html"]), "tests-none", only=True, component="web", config="untested"
    )
    expect_clean(r, "tests none, docs only", base(), ["tests-none"], config="untested")
    expect_hit(
        r,
        "tests none, docs and web",
        base(files=["CONTRIBUTING.md", "web/index.html"]),
        "tests-none",
        reasons=["gates.tests is none and web/index.html is outside component docs"],
        component="web",
        config="untested",
    )
    expect_clean(r, "tests cargo", base(files=["web/index.html"]), ["tests-none"], component="web", config="tested")
    expect_hit(
        r,
        "a script after cd",
        base(files=["web/x.sh"]),
        "gate-changes",
        reason="web/x.sh is named by gate web-script",
        component="web",
        config="cd-gates",
    )
    expect_hit(
        r,
        "a script after two cds",
        base(files=["web/apps/y.sh"]),
        "gate-changes",
        reason="web/apps/y.sh is named by gate nested-script",
        component="web",
        config="cd-gates",
    )
    expect_clean(
        r, "a script beside the cd", base(files=["web/y.sh"]), ["gate-changes"], component="web", config="cd-gates"
    )
    expect_hit(
        r,
        "overridden assumption",
        base(overridden_assumptions=[{"assumption": "A1", "reason": "the analysis was wrong"}]),
        "overridden-assumptions",
        only=True,
        reason="A1",
    )


def eight_keys(production_lines: int, **changes) -> dict:
    """A footprint from the clean base with production_lines right after estimated_lines."""
    footprint = base(**changes)
    result = {}
    for key, value in footprint.items():
        result[key] = value
        if key == "estimated_lines":
            result["production_lines"] = production_lines
    return result


def check_production_lines(r: Runner, limits: dict) -> None:
    n = limits["lines"]
    split = ["docs/guide.md", "docs/fixtures/big.md"]
    expect_clean(r, "production lines at the limit", eight_keys(n, files=split, estimated_lines=2000), ["limit-lines"])
    expect_hit(
        r,
        "production lines one over",
        eight_keys(n + 1, files=split, estimated_lines=2000),
        "limit-lines",
        only=True,
        reasons=[f"production_lines is {n + 1}, over the limit of {n}"],
    )
    expect_hit(
        r,
        "no non-production file",
        eight_keys(1, estimated_lines=n + 1),
        "limit-lines",
        only=True,
        reasons=[f"lines is {n + 1}, over the limit of {n}"],
    )
    for label, value in (("above estimated_lines", 7), ("negative", -1), ("true", True)):
        proc, verdict = r.run(f"production_lines {label}", plan_text(footprint=eight_keys(value)))
        T.check(
            f"production_lines {label}: footprint hit",
            proc.returncode == 1
            and results_of(verdict).get("footprint") == "hit"
            and any("production_lines" in x for x in reasons_of(verdict, "footprint")),
            proc.stdout,
        )


MANIFEST = "docs/requirements.txt"


def screened_package(name: str, manifest: str = MANIFEST, **changes) -> dict:
    """A package as dependencies/screen.py records it, screened with no disqualifier unless changes say so."""
    package = {
        "name": name,
        "manifest": manifest,
        "ecosystem": None,
        "version": "1.2.3",
        "status": "screened",
        "reason": "",
        "disqualifiers": [],
        "size": {"download_bytes": 2048, "unpacked_bytes": "unavailable", "file_count": "unavailable"},
        "dependencies": {"direct": 2, "transitive": 5, "approximate": False, "lockfile": None},
        "author": {
            "publisher": "alice",
            "owners": ["alice"],
            "publisher_first_release": "2020-01-01",
            "previous_publisher": "alice",
            "publisher_changed": False,
        },
        "establishment": {
            "created": "2020-01-01",
            "age_days": 2000,
            "releases": 40,
            "downloads": 100000,
            "repository": "https://example.invalid/repo",
            "last_release": "2025-01-01",
        },
        "problems": {"advisories": [], "yanked": False, "deprecated": None, "typosquat_of": None},
    }
    package.update(changes)
    return package


def screen_record(packages: list = (), **changes) -> str:
    """The text of a screen record as dependencies/screen.py writes it."""
    record = {
        "tool": "dependency-screen",
        "version": 1,
        "mode": "plan",
        "date": "2026-01-01",
        "advisory_source": "osv",
        "packages": list(packages),
    }
    record.update(changes)
    return json.dumps(record, indent=2) + "\n"


def declared(*names: str, manifest: str = MANIFEST) -> list:
    return [{"name": name, "manifest": manifest} for name in names]


def check_dependency_screen(r: Runner, sb: Sandbox, commits: dict) -> None:
    rule = "dependency-screen"
    expect_clean(r, "no packages, no screen record", base(), [rule])
    expect_clean(r, "no packages, an empty screen record", base(), [rule], screen=screen_record())
    alarming = screened_package(
        "alarming",
        size={"download_bytes": 900 * 1024 * 1024, "unpacked_bytes": 2**31, "file_count": 50000},
        dependencies={"direct": 60, "transitive": 900, "approximate": True, "lockfile": None},
        author=dict(screened_package("x")["author"], previous_publisher="mallory", publisher_changed=True),
        establishment=dict(screened_package("x")["establishment"], age_days=1, releases=1),
    )
    verdict = expect_clean(
        r,
        "screened with alarming evidence and no disqualifier",
        base(new_dependencies=declared("alarming")),
        [rule],
        screen=screen_record([alarming]),
    )
    T.check(
        "screened with alarming evidence and no disqualifier: every rule clean",
        set(results_of(verdict).values()) == {"clean"},
        str({k: v for k, v in results_of(verdict).items() if v != "clean"}),
    )

    five = base(new_dependencies=declared(*(f"dep-{i}" for i in range(5))))
    proc, verdict = r.run(
        "five packages, all screened",
        plan_text(footprint=five),
        screen=screen_record([screened_package(f"dep-{i}") for i in range(5)]),
    )
    named = [x.get("rule") for x in verdict.get("rules", [])] + list(verdict.get("not_evaluated", []))
    T.check(
        "five packages, all screened: exit 0, no count rule, dependency-screen clean",
        proc.returncode == 0 and "limit-new-dependencies" not in named and results_of(verdict).get(rule) == "clean",
        proc.stdout,
    )

    one = base(new_dependencies=declared("pkg"))
    for disqualifier in ("advisory", "yanked", "deprecated", "typosquat"):
        expect_hit(
            r,
            f"the {disqualifier} disqualifier",
            one,
            rule,
            only=True,
            reasons=[f"pkg 1.2.3 ({MANIFEST}) is disqualified: {disqualifier}"],
            screen=screen_record([screened_package("pkg", disqualifiers=[disqualifier])]),
        )
    expect_hit(
        r,
        "two disqualifiers",
        one,
        rule,
        reasons=[
            f"pkg 1.2.3 ({MANIFEST}) is disqualified: advisory",
            f"pkg 1.2.3 ({MANIFEST}) is disqualified: yanked",
        ],
        screen=screen_record([screened_package("pkg", disqualifiers=["advisory", "yanked"])]),
    )
    unscreened = screened_package("pkg", version=None, status="unscreened", reason="crates.io: HTTP 503")
    expect_hit(
        r,
        "an unscreened package",
        one,
        rule,
        only=True,
        reasons=[f"pkg ({MANIFEST}) is unscreened: crates.io: HTTP 503"],
        screen=screen_record([unscreened]),
    )
    expect_hit(
        r,
        "packages and no screen record",
        base(new_dependencies=declared("a", "b")),
        rule,
        only=True,
        reasons=["2 declared packages and no screen record (--screen)"],
    )
    expect_hit(
        r,
        "a declared package the record does not list",
        base(new_dependencies=declared("a", "b")),
        rule,
        only=True,
        reasons=[f"b ({MANIFEST}) is declared but not screened: the screen record does not list it"],
        screen=screen_record([screened_package("a")]),
    )
    expect_hit(
        r,
        "a package in another manifest",
        one,
        rule,
        reasons=[
            "the screen record lists pkg (docs/other.txt), which the footprint does not declare: unbound",
            f"pkg ({MANIFEST}) is declared but not screened: the screen record does not list it",
        ],
        screen=screen_record([screened_package("pkg", manifest="docs/other.txt")]),
    )
    expect_hit(
        r,
        "a record package not declared",
        base(),
        rule,
        only=True,
        reasons=[f"the screen record lists extra ({MANIFEST}), which the footprint does not declare: unbound"],
        screen=screen_record([screened_package("extra")]),
    )
    expect_hit(
        r,
        "a package listed twice",
        one,
        rule,
        reasons=[f"the screen record lists pkg ({MANIFEST}), which the footprint does not declare: unbound"],
        screen=screen_record([screened_package("pkg"), screened_package("pkg")]),
    )
    package = screened_package("pkg")
    unusable = [
        ("not JSON", "{ not json\n", "the screen record is not strict JSON"),
        ("a duplicate key", '{"tool": "dependency-screen", "tool": "x"}\n', "the screen record is not strict JSON"),
        ("a list", "[]\n", "the screen record is not a JSON object"),
        ("another tool", screen_record([package], tool="other"), "the screen record's tool is not dependency-screen"),
        ("version true", screen_record([package], version=True), "the screen record's version is not 1"),
        ("version 2", screen_record([package], version=2), "the screen record's version is not 1"),
        ("mode review", screen_record([package], mode="review"), "the screen record's mode is not plan"),
        (
            "packages not a list",
            json.dumps(dict(json.loads(screen_record()), packages={})),
            "the screen record's packages is not a list",
        ),
        (
            "a package without a name",
            screen_record([dict(package, name=None)]),
            "a package of the screen record has no string name, manifest or reason",
        ),
        (
            "another status",
            screen_record([dict(package, status="clean")]),
            "a package of the screen record has a status other than screened or unscreened",
        ),
        (
            "an unknown disqualifier",
            screen_record([dict(package, disqualifiers=["unmaintained"])]),
            "a package of the screen record has disqualifiers that are not unique values of "
            "advisory, yanked, deprecated, typosquat",
        ),
        (
            "a repeated disqualifier",
            screen_record([dict(package, disqualifiers=["yanked", "yanked"])]),
            "a package of the screen record has disqualifiers that are not unique values of "
            "advisory, yanked, deprecated, typosquat",
        ),
    ]
    for label, text, reason in unusable:
        expect_hit(r, f"screen record {label}", one, rule, only=True, reasons=[reason], screen=text)
    expect_hit(
        r,
        "an unusable record without packages declared",
        base(),
        rule,
        only=True,
        reasons=["the screen record's mode is not plan"],
        screen=screen_record(mode="review"),
    )
    hostile = screened_package("pkg", disqualifiers=["typosquat"], version="1.0\n\n## Heading " + "x" * 300)
    verdict = expect_hit(r, "a multi-line version", one, rule, screen=screen_record([hostile]))
    reasons = reasons_of(verdict, rule)
    T.check(
        "a multi-line version: the reason is one line, the version cut to 200 characters",
        len(reasons) == 1 and "\n" not in reasons[0] and "1.0 ## Heading " in reasons[0] and len(reasons[0]) < 300,
        repr(reasons),
    )

    proc, verdict = r.run(
        "a footprint hit with a screen record",
        plan_text(section="", opener="Nothing here.", block="", closer="\n"),
        screen=screen_record([screened_package("pkg", disqualifiers=["advisory"])]),
    )
    T.check(
        "a footprint hit with a screen record: dependency-screen not evaluated",
        proc.returncode == 1
        and isinstance(verdict, dict)
        and rule in verdict.get("not_evaluated", [])
        and rule not in results_of(verdict),
        proc.stdout,
    )


def check_component_limits(r: Runner, limits: dict) -> None:
    config = "component-limits"
    tooling = ["xtask/src/main.rs"]
    expect_clean(r, "docs override at 350", base(estimated_lines=350), ["limit-lines"], config=config)
    expect_hit(
        r,
        "docs override at 401",
        base(estimated_lines=401),
        "limit-lines",
        only=True,
        reasons=["lines is 401, over the limit of 400 (limits_by_component.docs)"],
        config=config,
    )
    expect_hit(
        r,
        "tooling default at 350",
        base(files=tooling, estimated_lines=350),
        "limit-lines",
        reasons=[f"lines is 350, over the limit of {limits['lines']}"],
        component="tooling",
        config=config,
    )
    n = limits["files"]
    docs = [f"docs/limit-{i}.md" for i in range(n + 1)]
    expect_clean(r, f"docs override, files at {n}", base(files=docs[:n]), ["limit-files"], config=config)
    expect_hit(
        r,
        f"docs override, files at {n + 1}",
        base(files=docs),
        "limit-files",
        only=True,
        reasons=[f"files is {n + 1}, over the limit of {n}"],
        config=config,
    )


def unusable_footprints() -> list:
    block = footprint_block(CLEAN_BASE)
    second = "\n```json\n{}\n```\n"
    cases = [
        ("no footprint section", plan_text(section="", opener="Nothing here.", block="", closer="\n")),
        ("two footprint sections", plan_text(after="\n## Footprint\n\nAgain.\n")),
        ("footprint after notes", plan_text(between="## Notes\n\nA note.\n\n")),
        ("prose in the section", plan_text(section="## Footprint\n\nSome prose.\n\n")),
        ("no closing fence", plan_text(closer="")),
        ("second json fence", plan_text(approach=second)),
        ("opener with trailing text", plan_text(opener="```json footprint")),
        ("text after the block", plan_text(after="\nTrailing prose.\n")),
        ("invalid json", plan_text(block="{ not json")),
        (
            "trailing comma",
            plan_text(block=replaced(block, '"overridden_assumptions": []', '"overridden_assumptions": [],')),
        ),
        (
            "duplicate key",
            plan_text(
                block=replaced(block, '"estimated_lines": 6,', '"estimated_lines": 6,\n    "estimated_lines": 6,')
            ),
        ),
        ("NaN", plan_text(block=replaced(block, '"estimated_lines": 6', '"estimated_lines": NaN'))),
    ]
    reordered = {"new_files": CLEAN_BASE["new_files"], **{k: v for k, v in CLEAN_BASE.items() if k != "new_files"}}
    missing = {k: v for k, v in CLEAN_BASE.items() if k != "gate_changes"}
    cases += [
        ("keys out of order", plan_text(footprint=reordered)),
        ("missing key", plan_text(footprint=missing)),
        ("extra key", plan_text(footprint=base(notes=[]))),
        ("null field", plan_text(footprint=base(external_steps=None))),
        ("estimated_lines true", plan_text(footprint=base(estimated_lines=True))),
        ("estimated_lines -1", plan_text(footprint=base(estimated_lines=-1))),
        ("repeated value", plan_text(footprint=base(files=["CONTRIBUTING.md", "CONTRIBUTING.md"]))),
        ("new file not in files", plan_text(footprint=base(new_files=["docs/new.md"]))),
        ("files empty", plan_text(footprint=base(files=[], estimated_lines=250))),
        (
            "dependency extra key",
            plan_text(
                footprint=base(new_dependencies=[{"name": "x", "manifest": "docs/requirements.txt", "version": "1"}])
            ),
        ),
        ("top level extra key", plan_text(block=json.dumps({"footprint": CLEAN_BASE, "more": 1}, indent=2))),
    ]
    for path in [
        "./a.md",
        "docs/../AGENTS.md",
        "/abs.md",
        "docs//a.md",
        "docs\\a.md",
        "docs/*.md",
        "docs/",
        ".mintswarm/job/x.md",
        ".MintSwarm/Job/x.md",
    ]:
        cases.append((f"path {path}", plan_text(footprint=base(files=[path]))))
    cases.append(
        (
            "manifest ../x",
            plan_text(footprint=base(new_dependencies=[{"name": "x", "manifest": "../requirements.txt"}])),
        )
    )
    return cases


def check_unusable_footprints(r: Runner) -> None:
    # An unclosed fence runs to the end of the plan, so Verification and Questions are inside it.
    fenced = {"no closing fence": {"blocking-questions", "acceptance-criteria"}}
    for name, text in unusable_footprints():
        proc, verdict = r.run(name, text)
        hits = fenced.get(name, set())
        expected = {"footprint": "hit", **{rule: "hit" if rule in hits else "clean" for rule in DOCUMENT_RULES}}
        ok = (
            proc.returncode == 1 and results_of(verdict) == expected and verdict.get("not_evaluated") == FOOTPRINT_RULES
        )
        label = ", ".join(sorted(hits)) + " hit" if hits else "clean"
        T.check(
            f"{name}: footprint hit, footprint rules not evaluated, document rules {label}",
            ok,
            f"exit {proc.returncode}: {proc.stdout.strip()} {proc.stderr.strip()}",
        )


def check_base_ref(r: Runner, sb: Sandbox) -> None:
    worktree = json.loads((sb.repo / CONFIG_PATH).read_text(encoding="utf-8"))
    T.check(
        "base ref: the worktree config drops the github path",
        ".mintswarm/dogfood/github/**" not in worktree["pipeline"]["plan_approval"]["require_human"]["paths"],
    )
    footprint = base(files=[".mintswarm/dogfood/github/push.sh"])
    proc, verdict = r.run(
        "base ref: --base base",
        plan_text(footprint=footprint),
        component="agents",
        ref="base",
        commit=r.commits["real"],
    )
    T.check(
        "base ref: --base base gives require-human-paths a hit",
        results_of(verdict).get("require-human-paths") == "hit"
        and any(".mintswarm/dogfood/github/**" in x for x in reasons_of(verdict, "require-human-paths")),
        proc.stdout,
    )
    proc, verdict = r.run(
        "base ref: --base HEAD",
        plan_text(footprint=footprint),
        component="agents",
        ref="HEAD",
        commit=r.commits["dropped"],
    )
    T.check(
        "base ref: --base HEAD gives require-human-paths clean",
        results_of(verdict).get("require-human-paths") == "clean",
        proc.stdout,
    )


def check_historic(r: Runner) -> None:
    fixture_reasons = {
        "plan-9.md": ("require-human-components", "the issue's component agents"),
        "plan-12.md": ("require-human-components", "the issue's component agents"),
        "plan-14.md": ("require-human-paths", ".mintswarm/dogfood/github/**"),
        "plan-19.md": ("external-steps", "Code Owner review"),
        "plan-10.md": ("require-human-paths", "AGENTS.md"),
    }
    cases = [(fixture, component, hits, "real") for fixture, (component, hits) in HISTORIC.items()]
    # At `nodocs` no commit is scoped (docs) yet, so a docs issue is the first of its kind.
    cases += [
        ("plan-20.md", "docs", {"first-of-kind"}, "nodocs"),
        ("plan-10.md", "docs", {"require-human-paths", "first-of-kind"}, "nodocs"),
    ]
    for fixture, component, hits, config in cases:
        name = f"fixture {fixture}" + (f" at {config}" if config != "real" else "")
        proc, verdict = r.run(name, historic(fixture), component=component, config=config)
        expected = {rule: ("hit" if rule in hits else "clean") for rule in RULES}
        got = results_of(verdict)
        diff = {k: got.get(k) for k in RULES if got.get(k) != expected[k]}
        T.check(
            f"{name}: the {len(RULES)} expected results",
            got == expected and proc.returncode == (1 if hits else 0),
            f"exit {proc.returncode}, differs on {diff}",
        )
        if fixture in fixture_reasons:
            rule, text = fixture_reasons[fixture]
            T.check(
                f"{name}: {rule} names {text!r}",
                any(text in x for x in reasons_of(verdict, rule)),
                str(reasons_of(verdict, rule)),
            )
        if "first-of-kind" in hits:
            T.check(
                f"{name}: first-of-kind names the component",
                any("scoped (docs)" in x for x in reasons_of(verdict, "first-of-kind")),
                str(reasons_of(verdict, "first-of-kind")),
            )


def check_blocking_questions(r: Runner) -> None:
    rule = "blocking-questions"
    question = "Must the floor read   tracker\thistory?"
    assumed = "- **Non-blocking (assumed):** Is the order free? → Yes.\n"
    expect_hit(
        r,
        "blocking question in the analysis",
        base(),
        rule,
        only=True,
        reasons=["analysis: blocking question: Must the floor read tracker history?"],
        analysis=analysis_document(assumed + f"- **Blocking:** {question}\n"),
    )
    expect_hit(
        r,
        "blocking question in the plan",
        base(),
        rule,
        only=True,
        reasons=["plan: blocking question: Which component owns it?"],
        tail=plan_tail(questions=f"{assumed}- **Blocking:** Which component owns it?\n"),
    )
    expect_hit(
        r,
        "blocking question with continuation lines",
        base(),
        rule,
        only=True,
        reasons=["analysis: blocking question: Which component owns it?"],
        analysis=analysis_document(
            "- **Blocking:** Which component owns it?\n  It could be agents.\n"
            "  - or docs, as a sub-bullet\n\n" + assumed
        ),
    )
    for label, entry in (
        ("indented", " - **Blocking:** Which component owns it?\n"),
        ("tab-indented", "\t- **Blocking:** Which component owns it?\n"),
        ("nested", assumed + "  - **Blocking:** Which component owns it?\n"),
    ):
        expect_hit(
            r,
            f"{label} blocking question",
            base(),
            rule,
            only=True,
            reasons=["analysis: blocking question: Which component owns it?"],
            analysis=analysis_document(entry),
        )
    expect_hit(
        r,
        "blocking question after a '# ' heading in Questions",
        base(),
        rule,
        only=True,
        reasons=[
            "analysis: line 13 of '## Questions' is a '# ' heading inside the section",
            "analysis: blocking question: Which component owns it?",
        ],
        analysis=analysis_document(assumed + "\n# Note\n\n- **Blocking:** Which component owns it?\n"),
    )
    expect_hit(
        r,
        "variant Blocking marker",
        base(),
        rule,
        only=True,
        reason="analysis: line 11 of '## Questions'",
        analysis=analysis_document("- **Blocking**: Which component owns it?\n"),
    )
    expect_hit(
        r,
        "prose in Questions",
        base(),
        rule,
        only=True,
        reason="analysis: line 11 of '## Questions' is not an entry with a known marker",
        analysis=analysis_document("None.\n"),
    )
    expect_hit(
        r,
        "blocking question past a long text",
        base(),
        rule,
        reasons=["analysis: blocking question: " + "x" * 200],
        analysis=analysis_document("- **Blocking:** " + "x" * 300 + "\n"),
    )
    expect_hit(
        r,
        "analysis without Questions",
        base(),
        rule,
        only=True,
        reason="analysis: no '## Questions' section",
        analysis=analysis_document(heading="## Remarks"),
    )
    expect_hit(
        r,
        "analysis with two Questions sections",
        base(),
        rule,
        only=True,
        reason="analysis: 2 '## Questions' sections",
        analysis=analysis_document(assumed + "\n## Questions\n\n" + assumed),
    )
    expect_hit(
        r,
        "plan without Questions",
        base(),
        rule,
        only=True,
        reason="plan: no '## Questions' section",
        tail=plan_tail().replace("## Questions", "## Notes"),
    )
    expect_hit(r, "no --analysis", base(), rule, only=True, reasons=["no analysis given (--analysis)"], analysis=None)
    expect_clean(
        r,
        "non-blocking questions only",
        base(),
        [rule],
        tail=plan_tail(questions=assumed),
        analysis=analysis_document(assumed + "  with a continuation\n" + assumed),
    )
    expect_clean(
        r,
        "Questions heading in a fence",
        base(),
        [rule],
        analysis=analysis_document(
            assumed + "\n## Next\n\n- **Blocking:** after the section\n",
            heading="```\n## Questions\n- **Blocking:** in a fence\n```\n\n" "## Questions",
        ),
    )
    check_question_markers(r)
    check_question_records(r)


SHARED_RECORD = ROOT / "crates/domain/tests/fixtures/result-blocks/needs-input/result.json"


def count_reason(what: str, bullets: tuple, record: tuple) -> str:
    """The blocking-questions reason for bullets (must decide, recommended, blocking, assumption and,
    optionally, unknown) that disagree with a record's (must decide, recommended, assumption) counts."""
    unknown = bullets[4] if len(bullets) > 4 else 0
    return (
        f"{what}: the '## Questions' bullets (must decide {bullets[0]}, recommended {bullets[1]}, "
        f"blocking {bullets[2]}, assumption {bullets[3]}, unknown {unknown}) disagree with the result record "
        f"(must decide {record[0]}, recommended {record[1]}, assumption {record[2]})"
    )


def check_question_markers(r: Runner) -> None:
    """The new markers in documents without a result record: the Markdown alone, as for the old markers."""
    rule = "blocking-questions"
    for label, marker in (("Must decide", "- **Must decide:**"), ("Recommended", "- **Recommended:**")):
        for where, entry in (("column 0", marker), ("indented", "  " + marker)):
            expect_hit(
                r,
                f"{label} question at {where} without a record",
                base(),
                rule,
                only=True,
                reasons=["analysis: blocking question: Which component owns it?"],
                analysis=analysis_document(
                    f"- **Assumption:** Is the order free? → Yes.\n{entry} Which component owns it?\n"
                ),
            )
    expect_hit(
        r,
        "Must decide question in the plan without a record",
        base(),
        rule,
        only=True,
        reasons=["plan: blocking question: Which component owns it?"],
        tail=plan_tail(questions="- **Must decide:** Which component owns it?\n"),
    )
    expect_clean(
        r,
        "assumptions only without a record",
        base(),
        [rule],
        tail=plan_tail(questions="- **Assumption:** Is the order free? → Yes.\n"),
        analysis=analysis_document(
            "- **Assumption:** Is the order free? → Yes.\n  with a continuation\n"
            "- **Non-blocking (assumed):** Is the name free? → Yes.\n"
        ),
    )


def check_question_records(r: Runner) -> None:
    """Documents with a result record: the question objects decide, the bullets are counted against them."""
    rule = "blocking-questions"
    each_kind = [("must_decide", "Which component owns it?"), ("recommended", "Is one PR enough?")]
    each_kind.append(("assumption", "Is the order free?"))
    new_bullets = (
        "- **Must decide:** Which component owns it?\n- **Recommended:** Is one PR enough? → Yes.\n"
        "- **Assumption:** Is the order free? → Yes.\n"
    )
    old_bullets = (
        "- **Blocking:** Which component owns it?\n- **Blocking:** Is one PR enough? → Yes.\n"
        "- **Non-blocking (assumed):** Is the order free? → Yes.\n"
    )
    hits = [
        "blocking question (must_decide): Which component owns it?",
        "blocking question (recommended): Is one PR enough?",
    ]
    for label, bullets in (("new", new_bullets), ("old", old_bullets)):
        expect_hit(
            r,
            f"record with each kind, {label} markers, in the analysis",
            base(),
            rule,
            only=True,
            reasons=[f"analysis: {hit}" for hit in hits],
            analysis=analysis_document(bullets),
            analysis_record=result_record(each_kind),
        )
        expect_hit(
            r,
            f"record with each kind, {label} markers, in the plan",
            base(),
            rule,
            only=True,
            reasons=[f"plan: {hit}" for hit in hits],
            tail=plan_tail(questions=bullets),
            plan_record=result_record(each_kind),
        )
    assumptions = result_record([("assumption", "Is the order free?"), ("assumption", "Is the name free?")])
    for label, marker in (("Assumption", "- **Assumption:**"), ("Non-blocking", "- **Non-blocking (assumed):**")):
        bullets = f"{marker} Is the order free? → Yes.\n  with a continuation\n{marker} Is the name free? → Yes.\n"
        expect_clean(
            r,
            f"record of assumptions only, {label} markers",
            base(),
            [rule],
            tail=plan_tail(questions=bullets),
            analysis=analysis_document(bullets),
            analysis_record=assumptions,
            plan_record=assumptions,
        )
    expect_clean(
        r,
        "record with no questions and no bullets",
        base(),
        [rule],
        analysis=analysis_document(""),
        analysis_record=result_record([]),
        plan_record=result_record([]),
    )
    expect_hit(
        r,
        "shared needs-input record",
        base(),
        rule,
        only=True,
        reasons=[
            "analysis: blocking question (must_decide): Which component owns the parser?",
            "analysis: blocking question (recommended): Which harnesses record replies?",
            "analysis: blocking question (must_decide): What is the parser's module called?",
        ],
        analysis=analysis_document(
            "- **Must decide:** Which component owns the parser?\n"
            "- **Assumption:** Are trailing comments allowed? → Yes.\n"
            "- **Recommended:** Which harnesses record replies? → Claude Code and Codex.\n"
            "- **Must decide:** What is the parser's module called? → result_block.\n"
        ),
        analysis_record=SHARED_RECORD.read_text(encoding="utf-8"),
    )
    must = "analysis: blocking question (must_decide): Which component owns it?"
    one_must = result_record([("must_decide", "Which component owns it?"), ("assumption", "Is the order free?")])
    assumed = "- **Assumption:** Is the order free? → Yes.\n"
    for label, bullets, record, reasons in (
        (
            "a missing bullet",
            assumed,
            one_must,
            [must, count_reason("analysis", (0, 0, 0, 1), (1, 0, 1))],
        ),
        (
            "an extra bullet",
            assumed + "- **Assumption:** Is the name free? → Yes.\n",
            result_record([("assumption", "Is the order free?")]),
            [count_reason("analysis", (0, 0, 0, 2), (0, 0, 1))],
        ),
        (
            "a swapped kind",
            "- **Recommended:** Which component owns it? → agents.\n" + assumed,
            one_must,
            [must, count_reason("analysis", (0, 1, 0, 1), (1, 0, 1))],
        ),
        (
            "a Blocking bullet and no blocking object",
            "- **Blocking:** Which component owns it?\n" + assumed,
            result_record([("assumption", "Is the order free?")]),
            [count_reason("analysis", (0, 0, 1, 1), (0, 0, 1))],
        ),
        (
            "an indented blocking bullet",
            assumed + "  - **Must decide:** Which component owns it?\n",
            result_record([("assumption", "Is the order free?")]),
            [count_reason("analysis", (1, 0, 0, 1), (0, 0, 1))],
        ),
        (
            "an unknown marker",
            assumed + "- **Question:** Which component owns it?\n",
            result_record([("assumption", "Is the order free?")]),
            [
                "analysis: line 12 of '## Questions' is not an entry with a known marker",
                count_reason("analysis", (0, 0, 0, 1, 1), (0, 0, 1)),
            ],
        ),
        (
            "no Questions section",
            None,
            one_must,
            [must, "analysis: no '## Questions' section"],
        ),
    ):
        expect_hit(
            r,
            f"record against {label}",
            base(),
            rule,
            only=True,
            reasons=reasons,
            analysis=analysis_document(heading="## Remarks") if bullets is None else analysis_document(bullets),
            analysis_record=record,
        )
    expect_hit(
        r,
        "record against a missing bullet in the plan",
        base(),
        rule,
        only=True,
        reasons=[must.replace("analysis", "plan", 1), count_reason("plan", (0, 0, 0, 1), (1, 0, 1))],
        tail=plan_tail(questions=assumed),
        plan_record=one_must,
    )
    question = {"id": "q1", "kind": "assumption", "question": "Is the order free?"}
    for cause, record in (
        ("it is not strict JSON", '{"questions": ['),
        ("it is not strict JSON", '{"questions": [], "questions": []}'),
        ("it is not strict JSON", '{"questions": [], "n": NaN}'),
        ("it is not a JSON object", "[]"),
        ("its questions is not a list", '{"status": "complete"}'),
        ("its questions is not a list", '{"questions": {}}'),
        ("a question is not a JSON object", '{"questions": ["Is the order free?"]}'),
        (
            "a question's kind is not must_decide, recommended or assumption",
            json.dumps({"questions": [dict(question, kind="blocking")]}),
        ),
        ("a question's question is not a string", json.dumps({"questions": [dict(question, question=None)]})),
    ):
        expect_hit(
            r,
            f"unusable record: {cause}: {record[:24]}",
            base(),
            rule,
            only=True,
            reasons=[f"analysis: the result record is not usable: {cause}"],
            analysis=analysis_document(assumed),
            analysis_record=record,
        )
    expect_hit(
        r,
        "unusable record with a '# ' heading in Questions",
        base(),
        rule,
        only=True,
        reasons=[
            "analysis: the result record is not usable: it is not a JSON object",
            "analysis: line 13 of '## Questions' is a '# ' heading inside the section",
        ],
        analysis=analysis_document(assumed + "\n# Note\n"),
        analysis_record="null",
    )


def check_acceptance_criteria(r: Runner) -> None:
    rule = "acceptance-criteria"
    escaped = [c.replace("|", "\\|") for c in CRITERIA]
    expect_hit(
        r,
        "criterion dropped",
        base(),
        rule,
        only=True,
        reasons=[f"the Verification table lacks the criterion: {CRITERIA[1]}"],
        tail=plan_tail(rows=[escaped[0], escaped[2]]),
    )
    expect_hit(
        r,
        "criterion added",
        base(),
        rule,
        only=True,
        reasons=["the Verification table has a row that is not a criterion of the issue: An extra row."],
        tail=plan_tail(rows=escaped + ["An extra row."]),
    )
    reworded = "A reason names the criterion it concerns."
    expect_hit(
        r,
        "criterion reworded",
        base(),
        rule,
        only=True,
        reasons=[
            f"the Verification table lacks the criterion: {CRITERIA[1]}",
            f"the Verification table has a row that is not a criterion of the issue: {reworded}",
        ],
        tail=plan_tail(rows=[escaped[0], reworded, escaped[2]]),
    )
    expect_hit(
        r,
        "criterion repeated",
        base(),
        rule,
        only=True,
        reason=f"not a criterion of the issue: {CRITERIA[0]}",
        tail=plan_tail(rows=escaped + escaped[:1]),
    )
    expect_hit(
        r,
        "issue without criteria",
        base(),
        rule,
        only=True,
        reason="holds no criteria",
        issue=issue_document(criteria="The plan should be good.\n"),
    )
    expect_hit(
        r,
        "issue without the criteria section",
        base(),
        rule,
        reason="no '## Acceptance criteria' section",
        issue=issue_document().replace("## Acceptance criteria", "## Done when"),
    )
    expect_hit(
        r,
        "two Verification sections",
        base(),
        rule,
        only=True,
        reason="2 '## Verification' sections",
        tail=plan_tail().replace("\n## Questions", "\n## Verification\n\nAgain.\n\n## Questions"),
    )
    expect_hit(
        r,
        "Verification without a table",
        base(),
        rule,
        only=True,
        reason="does not start with a table",
        tail=plan_tail().replace("| --- | --- |\n", ""),
    )
    expect_hit(
        r, "no --issue-document", base(), rule, reasons=["no issue document given (--issue-document)"], issue=None
    )
    spaced = ["  The floor reads  the issue\tdocument and the analysis.   ", escaped[1], escaped[2]]
    expect_clean(r, "criteria differing in whitespace", base(), [rule], tail=plan_tail(rows=spaced))
    wrapped = (
        f"1. {CRITERIA[0][:20]}\n   {CRITERIA[0][20:]}\n2. {CRITERIA[1]}\n"
        f"3. {CRITERIA[2][:10]}\n{CRITERIA[2][10:]}\n\nProse after the list.\n"
    )
    expect_clean(r, "wrapped issue criteria", base(), [rule], issue=issue_document(criteria=wrapped))
    paragraphs = (
        f"1. {CRITERIA[0]}\n\n   It reads them as UTF-8.\n\n2. {CRITERIA[1]}\n"
        f"3. {CRITERIA[2]}\n\nProse after the list.\n"
    )
    expect_hit(
        r,
        "criterion's later paragraph left out",
        base(),
        rule,
        only=True,
        reason=f"lacks the criterion: {CRITERIA[0]} It reads them as UTF-8.",
        issue=issue_document(criteria=paragraphs),
    )
    expect_clean(
        r,
        "criterion's later paragraph quoted",
        base(),
        [rule],
        issue=issue_document(criteria=paragraphs),
        tail=plan_tail(rows=[f"{CRITERIA[0]} It reads them as UTF-8.", *escaped[1:]]),
    )
    expect_clean(r, "escaped bar", base(), [rule], tail=plan_tail(rows=escaped))
    markers = f"Done when:\n\n- [ ] {CRITERIA[0]}\n* [x] {CRITERIA[1]}\n" f"1) {CRITERIA[2]}\n"
    expect_clean(r, "list markers and task boxes", base(), [rule], issue=issue_document(criteria=markers))
    expect_clean(r, "rows reordered", base(), [rule], tail=plan_tail(rows=list(reversed(escaped))))
    expect_clean(r, "decoy criteria in comments", base(), [rule], issue=issue_document())
    expect_clean(r, "CRLF issue document", base(), [rule], issue=issue_document().replace("\n", "\r\n"))


def check_first_of_kind(r: Runner) -> None:
    rule = "first-of-kind"
    for component, files in (
        ("cli", ["crates/cli/src/main.rs"]),
        ("core", ["crates/core/src/lib.rs"]),
        ("runner", ["crates/runner/src/lib.rs"]),
        ("domain", ["crates/domain/src/lib.rs"]),
    ):
        expect_hit(
            r,
            f"first {component} job",
            base(files=files),
            rule,
            component=component,
            reasons=[
                "first of its kind: no commit reachable from the base commit has a subject " f"scoped ({component})"
            ],
        )
    expect_clean(
        r,
        "cli job after a cli commit",
        base(files=["crates/cli/src/main.rs"]),
        [rule],
        component="cli",
        config="dropped",
    )
    expect_clean(r, "api job after a breaking api commit", base(files=["api/jobs.proto"]), [rule], component="api")
    expect_clean(r, "docs job after a docs commit", base(), [rule])
    expect_hit(
        r,
        "issue kind plan",
        base(),
        rule,
        only=True,
        reasons=["the issue's kind is 'plan', not implementation"],
        issue=issue_document(kind="plan"),
    )
    expect_hit(
        r,
        "issue component differs",
        base(),
        rule,
        reasons=["the issue's component is 'web', not --component docs"],
        issue=issue_document(component="web"),
    )
    header = "- **Kind:** implementation\n- **Kind:** implementation\n- **Component:** docs\n"
    expect_hit(
        r,
        "issue kind twice",
        base(),
        rule,
        only=True,
        reasons=["the issue document's header has 2 '- **Kind:**' lines, expected 1"],
        issue=issue_document(header=header),
    )
    expect_hit(
        r,
        "issue without a component line",
        base(),
        rule,
        only=True,
        reasons=["the issue document's header has 0 '- **Component:**' lines, expected 1"],
        issue=issue_document(header="- **Kind:** implementation\n"),
    )
    expect_hit(
        r,
        "no --issue-document for first-of-kind",
        base(),
        rule,
        reasons=["no issue document given (--issue-document)"],
        issue=None,
    )


def check_unusable_input(sb: Sandbox, commits: dict) -> None:
    good = sb.write_plan("good", plan_text())
    not_utf8 = sb.write_plan("not utf-8", b"# Plan\n\n\xff\xfe\n")
    real = commits["real"]
    cases = [
        ("no arguments", []),
        ("unknown argument", ["--plan", good, "--component", "docs", "--base", real, "--extra", "x"]),
        ("argument twice", ["--plan", good, "--plan", good, "--component", "docs", "--base", real]),
        ("missing value", ["--plan", good, "--component", "docs", "--base"]),
        ("base starting with -", ["--plan", good, "--component", "docs", "--base", "-HEAD"]),
        ("missing plan", ["--plan", "cases/missing.md", "--component", "docs", "--base", real]),
        ("plan not UTF-8", ["--plan", not_utf8, "--component", "docs", "--base", real]),
        ("unresolvable base", ["--plan", good, "--component", "docs", "--base", "no-such-ref"]),
        ("base without config", ["--plan", good, "--component", "docs", "--base", commits["noconfig"]]),
        ("unknown component", ["--plan", good, "--component", "nope", "--base", real]),
    ]
    for variant in (
        "no-plan-approval",
        "unknown-key",
        "mode-sometimes",
        "limit-negative",
        "limit-true",
        "undefined-component",
        "missing-limit",
        *MIN_CONFIDENCE_INVALID,
        *SIZE_INVALID,
        *REMOVED_LIMIT,
    ):
        cases.append((f"config {variant}", ["--plan", good, "--component", "docs", "--base", commits[variant]]))
    issue = sb.write_plan("unusable issue", issue_document())
    analysis = sb.write_plan("unusable analysis", analysis_document())
    plain = ["--plan", good, "--component", "docs", "--base", real]
    screen = sb.write_plan("unusable screen", screen_record())
    for flag, path in (("--issue-document", issue), ("--analysis", analysis), ("--screen", screen)):
        what = flag[2:]
        cases += [
            (f"{what} missing", plain + [flag, "cases/missing.md"]),
            (f"{what} unreadable", plain + [flag, "cases"]),
            (f"{what} not UTF-8", plain + [flag, not_utf8]),
            (f"{what} twice", plain + [flag, path, flag, path]),
            (f"{what} empty value", plain + [flag, ""]),
            (f"{what} without a value", plain + [flag]),
            (f"{what} value an argument name", plain + [flag, "--plan"]),
        ]
    for label, path, record in (
        ("analysis", analysis, None),
        ("plan", good, None),
        ("analysis", analysis, b'{"questions": []}\xff\n'),
        ("plan", good, b'{"questions": []}\xff\n'),
    ):
        target = sb.repo / path
        target = target.with_name(target.name[: -len(".md")] + ".result.json")
        if record is None:
            target.mkdir()
        else:
            write_record(sb.repo / path, record)
        args = plain + ["--analysis", analysis]
        proc, verdict = sb.floor(args)
        T.check(
            f"unusable {label} result record {'unreadable' if record is None else 'not UTF-8'}: exit 2, no verdict",
            proc.returncode == 2 and verdict is None and len(proc.stderr.strip().splitlines()) == 1,
            f"exit {proc.returncode}, verdict {'left' if verdict is not None else 'gone'}: {proc.stderr.strip()}",
        )
        if record is None:
            target.rmdir()
        else:
            target.unlink()
    for name, args in cases:
        proc, verdict = sb.floor(args)
        T.check(
            f"unusable {name}: exit 2, no verdict",
            proc.returncode == 2 and verdict is None and len(proc.stderr.strip().splitlines()) == 1,
            f"exit {proc.returncode}, verdict {'left' if verdict is not None else 'gone'}: {proc.stderr.strip()}",
        )

    bindir = sb.tmp / "git-only"
    bindir.mkdir()
    (bindir / "git").symlink_to(shutil.which("git"))
    env = dict(sb.env, PATH=str(bindir))
    proc, verdict = sb.floor(["--plan", good, "--component", "docs", "--base", real], env=env)
    T.check(
        "unusable yq missing: exit 2, no verdict",
        proc.returncode == 2 and verdict is None,
        f"exit {proc.returncode}: {proc.stderr.strip()}",
    )


def check_verdict_errors(sb: Sandbox, commits: dict) -> None:
    good = sb.write_plan("verdict errors good", plan_text())
    nofootprint = sb.write_plan(
        "verdict errors no footprint", plan_text(section="", opener="Nothing here.", block="", closer="\n")
    )
    args = [
        "--component",
        "docs",
        "--base",
        commits["real"],
        "--issue-document",
        sb.write_plan("verdict errors issue", issue_document()),
        "--analysis",
        sb.write_plan("verdict errors analysis", analysis_document()),
    ]
    verdict = sb.repo / VERDICT
    outputs = verdict.parent

    def exit_2(name: str, proc, unexpected: bool = False) -> None:
        lines = proc.stderr.strip().splitlines()
        ok = (
            proc.returncode == 2
            and len(lines) == 1
            and "Traceback" not in proc.stderr
            and (not unexpected or "unexpected error" in proc.stderr)
        )
        T.check(name, ok, f"exit {proc.returncode}: {proc.stderr.strip()}")

    outputs.mkdir(parents=True, exist_ok=True)
    verdict.unlink(missing_ok=True)
    verdict.mkdir()
    proc = sb.exec(["--plan", good, *args])
    exit_2("verdict a directory: exit 2, one-line error", proc)
    T.check(
        "verdict a directory: no verdict written",
        verdict.is_dir() and not any(verdict.iterdir()) and not list(outputs.glob(".floor.*")),
    )
    verdict.rmdir()

    if os.geteuid() == 0:
        print("ok   verdict errors: read-only outputs skipped, running as root")
        return
    proc = sb.exec(["--plan", good, *args])
    T.check("read-only outputs: a clean verdict first", proc.returncode == 0 and verdict.is_file(), proc.stderr)
    outputs.chmod(0o555)
    try:
        proc = sb.exec(["--plan", nofootprint, *args])
        exit_2("read-only outputs, stale verdict: exit 2, one-line error", proc)
    finally:
        outputs.chmod(0o755)
    verdict.unlink()
    outputs.chmod(0o555)
    try:
        proc = sb.exec(["--plan", good, *args])
        exit_2("read-only outputs, no stale verdict: exit 2 from the catch-all", proc, unexpected=True)
        T.check(
            "read-only outputs, no stale verdict: no verdict written",
            not verdict.exists() and not list(outputs.glob(".floor.*")),
        )
    finally:
        outputs.chmod(0o755)


# --- decision -------------------------------------------------------------

RECORD_KEYS = [
    "tool",
    "version",
    "approval",
    "subject",
    "mode",
    "min_confidence",
    "layers",
    "judged_decision",
    "decision",
    "reasons",
]
FLOOR_LAYER_KEYS = ["status", "problems", "path", "sha256", "verdict", "rules", "not_evaluated"]
JUDGEMENT_LAYER_KEYS = [
    "status",
    "problems",
    "stage",
    "path",
    "sha256",
    "artifact",
    "harness",
    "model",
    "expected",
    "as_configured",
    "confidence",
    "band",
    "hard_triggers",
    "doubts",
    "reasons",
]
EXPECTED_JUDGE = {"harness": "claude", "model": "sonnet"}
REASON = "the change stays within the docs component"
ARTIFACT = "https://github.com/mintswarm/mintswarm/issues/34/artifacts/7"
SPAN_LIMIT = 600
SPAN_MARK = " … (truncated)"


# The assess stage's named bands: identifier -> (display name, lowest, highest confidence).
BANDS = {
    "blocked": ("Blocked", 0, 0),
    "uncertain": ("Uncertain", 1, 49),
    "needs-review": ("Needs review", 50, 79),
    "likely-safe": ("Likely safe", 80, 94),
    "safe": ("Safe", 95, 100),
}
HARD_TRIGGERS = [
    "criterion-not-met",
    "outside-scope",
    "not-revertible",
    "credentials",
    "weakens-safeguard",
    "undeclared-high",
]
AUTHORITY_TRIGGERS = ["not-revertible", "credentials", "weakens-safeguard", "undeclared-high"]
UNUSABLE_TRIGGERS = [t for t in HARD_TRIGGERS if t not in AUTHORITY_TRIGGERS]
DOUBT = {
    "statement": "the docs index may list the page twice",
    "consequence": "a duplicate entry in the index",
    "resolution": "check the index after the change",
}
# The mode and min_confidence of each base config Decider runs at, where they are not the config's name and 90.
CONFIG_MODES = {"real": EXPECTED_APPROVAL["mode"], "judged-95": "judged", "min-100": "judged"}
CONFIG_MIN_CONFIDENCE = {"judged-95": 95, "min-100": 100}


def envelope(band: str = "safe", confidence: int = None, triggers: list = None, doubts: list = None, reason=REASON):
    """An envelope object: a band at its lowest value, with a trigger for blocked and a doubt for likely-safe."""
    if confidence is None:
        confidence = BANDS[band][1]
    if triggers is None:
        triggers = ["criterion-not-met"] if band == "blocked" else []
    if doubts is None:
        doubts = [dict(DOUBT)] if band == "likely-safe" else []
    return {"confidence": confidence, "band": band, "hard_triggers": triggers, "reason": reason, "doubts": doubts}


SAFE = envelope("safe", 95)


def assessment_text(
    content=None,
    summary: str = None,
    eol: str = "\n",
    title: str = "# Assessment: selftest case",
    fence: str = "```json",
    close: str = "```",
    extra: str = "",
) -> str:
    """An assessment document: content is an envelope object, or the envelope's raw text."""
    content = SAFE if content is None else content
    body = content if isinstance(content, str) else json.dumps(content, indent=2, ensure_ascii=False)
    if summary is None:
        number = content.get("confidence") if isinstance(content, dict) else None
        number = number if type(number) is int and 0 <= number <= 100 else 95
        summary = f"{number} confidence: {REASON}"
    lines = [title, "", summary, "", "## Envelope", "", fence, *body.split("\n")]
    if close is not None:
        lines.append(close)
    lines += ["", "## Reversibility", "", "A revert undoes it.", ""]
    return eol.join(lines) + extra


def confidence_line(band: str, confidence: int, threshold: int = 90, reason: str = REASON, authority=()) -> str:
    requires = f" · requires the maintainer: {', '.join(f'`{t}`' for t in authority)}" if authority else ""
    return f"`{BANDS[band][0]} · {confidence}`{requires} (min_confidence {threshold}), `{reason}`"


SAFE_LINE = confidence_line("safe", 95)


def code_spans(text: str) -> list:
    return re.findall(r"`([^`\n]*)`", text)


def outside_spans(text: str) -> str:
    return re.sub(r"`[^`\n]*`", "", text)


# The plans Decider prepares: (plan text, issue document, analysis, floor.py's exit code).
HOSTILE_PACKAGE = "evil`pkg @someone https://example.invalid/x"
DECIDER_PLANS = {
    "clean": (plan_text(), issue_document(), analysis_document(), 0),
    "hit": (plan_text(footprint=base(files=["CLAUDE.md"])), issue_document(), analysis_document(), 1),
    "blocking": (plan_text(), issue_document(), analysis_document("- **Blocking:** Which component owns it?\n"), 1),
    "no footprint": (
        plan_text(section="", opener="Nothing here.", block="", closer="\n"),
        issue_document(),
        analysis_document(),
        1,
    ),
    "kind plan": (plan_text(), issue_document(kind="plan"), analysis_document(), 1),
    "packages": (
        plan_text(footprint=base(new_dependencies=declared("good-pkg", "bad-pkg"))),
        issue_document(),
        analysis_document(),
        1,
    ),
    "screened": (
        plan_text(footprint=base(new_dependencies=declared("good-pkg"))),
        issue_document(),
        analysis_document(),
        0,
    ),
    "hostile package": (
        plan_text(footprint=base(new_dependencies=declared(HOSTILE_PACKAGE))),
        issue_document(),
        analysis_document(),
        0,
    ),
    "recorded": (
        plan_text(tail=plan_tail(questions="- **Assumption:** Is the order free? → Yes.\n")),
        issue_document(),
        analysis_document("- **Must decide:** Which component owns it?\n"),
        1,
    ),
    "assumed": (
        plan_text(tail=plan_tail(questions="- **Assumption:** Is the order free? → Yes.\n")),
        issue_document(),
        analysis_document("- **Non-blocking (assumed):** Is the name free? → Yes.\n"),
        0,
    ),
}
# The result records Decider writes beside the analysis and the plan: (analysis's, plan's).
DECIDER_RECORDS = {
    "recorded": (
        result_record([("must_decide", "Which component owns it?")]),
        result_record([("assumption", "Is the order free?")]),
    ),
    "assumed": (
        result_record([("assumption", "Is the name free?")]),
        result_record([("assumption", "Is the order free?")]),
    ),
}
# The screen records Decider gives floor.py, and decision.py by default, for a plan.
DECIDER_SCREENS = {
    "packages": screen_record(
        [
            screened_package("good-pkg"),
            screened_package(
                "bad-pkg",
                version="0.9.0",
                disqualifiers=["advisory", "typosquat"],
                problems={
                    "advisories": [{"id": "GHSA-0000-1111-2222", "summary": "bad", "source": "osv", "aliases": []}],
                    "yanked": False,
                    "deprecated": None,
                    "typosquat_of": "good-pkg",
                },
            ),
        ]
    ),
    "screened": screen_record([screened_package("good-pkg")]),
    "hostile package": screen_record([screened_package(HOSTILE_PACKAGE, version="1.0 @someone")]),
}


class Decider:
    """Runs decision.py in the sandbox after a real floor.py run at the same base commit and documents."""

    def __init__(self, sb: Sandbox, commits: dict):
        self.sb = sb
        self.commits = commits
        self.prepared = {}

    def floor_verdict(
        self, ctx: dict, issue: str = None, analysis: str = None, documents: bool = True, screen="ctx"
    ) -> bytes:
        """floor.py's verdict for ctx's plan, with ctx's documents, other ones, or none; screen: "ctx" (ctx's
        screen record, if any), None (none) or a path."""
        args = ["--plan", ctx["plan"], "--component", ctx["component"], "--base", ctx["commit"]]
        if documents:
            args += ["--issue-document", issue or ctx["issue"], "--analysis", analysis or ctx["analysis"]]
        screen = ctx["screen"] if screen == "ctx" else screen
        if screen is not None:
            args += ["--screen", screen]
        proc = self.sb.exec(args)
        if proc.returncode not in (0, 1):
            raise RuntimeError(f"selftest bug: floor.py exit {proc.returncode}: {proc.stderr.strip()}")
        return (self.sb.repo / VERDICT).read_bytes()

    def prepare(self, config: str, plan: str) -> dict:
        key = (config, plan)
        if key not in self.prepared:
            text, issue, analysis, code = DECIDER_PLANS[plan]
            ctx = {
                "plan": self.sb.write_plan(f"decision {config} {plan}", text),
                "issue": self.sb.write_plan(f"decision {config} {plan} issue", issue),
                "analysis": self.sb.write_plan(f"decision {config} {plan} analysis", analysis),
                "commit": self.commits[config],
                "component": "docs",
                "screen": None,
            }
            if plan in DECIDER_SCREENS:
                ctx["screen"] = self.sb.write_plan(f"decision {config} {plan} screen", DECIDER_SCREENS[plan])
            if plan in DECIDER_RECORDS:
                for document, record in zip(("analysis", "plan"), DECIDER_RECORDS[plan]):
                    ctx[f"{document}_record"] = write_record(self.sb.repo / ctx[document], record)
            proc = self.sb.exec(
                [
                    "--plan",
                    ctx["plan"],
                    "--component",
                    "docs",
                    "--base",
                    ctx["commit"],
                    "--issue-document",
                    ctx["issue"],
                    "--analysis",
                    ctx["analysis"],
                    *(["--screen", ctx["screen"]] if ctx["screen"] is not None else []),
                ]
            )
            if proc.returncode != code:
                raise RuntimeError(f"selftest bug: floor.py exit {proc.returncode} for {key}: {proc.stderr.strip()}")
            ctx["verdict"] = (self.sb.repo / VERDICT).read_bytes()
            self.prepared[key] = ctx
        return self.prepared[key]

    def run(
        self,
        name: str,
        config: str = "judged",
        plan: str = "clean",
        verdict="floor",
        assessment=SAFE,
        harness: str = "claude",
        model: str = "sonnet",
        artifact: str = ARTIFACT,
        screen="ctx",
    ):
        """verdict: "floor" (the real one), None (missing), a function editing the JSON, or bytes; screen:
        "ctx" (the plan's screen record, if any), None (no --screen) or a path."""
        ctx = self.prepare(config, plan)
        path = self.sb.repo / VERDICT
        path.unlink(missing_ok=True)
        if verdict == "floor":
            path.write_bytes(ctx["verdict"])
        elif callable(verdict):
            document = json.loads(ctx["verdict"])
            verdict(document)
            path.write_text(json.dumps(document, indent=2) + "\n", encoding="utf-8")
        elif verdict is not None:
            path.write_bytes(verdict)
        expected = assessment if isinstance(assessment, dict) else None
        if assessment is None:
            rel = "cases/no-such-assessment.md"
        else:
            content = assessment_text(assessment) if expected is not None else assessment
            rel = self.sb.write_plan(f"assessment {name}", content)
        args = [
            "--issue",
            "34",
            "--component",
            ctx["component"],
            "--plan",
            ctx["plan"],
            "--issue-document",
            ctx["issue"],
            "--analysis",
            ctx["analysis"],
            "--assessment",
            rel,
            "--assessment-artifact",
            artifact,
            "--base",
            ctx["commit"],
            "--harness",
            harness,
            "--model",
            model,
        ]
        screen = ctx["screen"] if screen == "ctx" else screen
        if screen is not None:
            args += ["--screen", screen]
        record_path = self.sb.repo / RECORD
        record_path.write_text('{"stale": true}\n', encoding="utf-8")
        proc = self.sb.exec(args, script=DECISION)
        record = None
        if record_path.exists():
            try:
                record = json.loads(record_path.read_text(encoding="utf-8"))
            except ValueError:
                record = "unparsable"
        expect = {
            "ctx": ctx,
            "config": config,
            "assessment": rel,
            "artifact": artifact,
            "harness": harness,
            "model": model,
            "envelope": expected,
        }
        check_record(name, proc, record, expect, self.sb.repo)
        return proc, record


def check_record(name: str, proc, record, expect: dict, repo: Path) -> bool:
    if not isinstance(record, dict):
        return T.check(f"{name}: record shape", False, f"no record (exit {proc.returncode}): {proc.stderr.strip()}")
    problems = []
    ctx = expect["ctx"]
    mode = CONFIG_MODES.get(expect["config"], expect["config"])
    threshold = CONFIG_MIN_CONFIDENCE.get(expect["config"], 90)
    if list(record) != RECORD_KEYS:
        return T.check(f"{name}: record shape", False, f"keys {list(record)}")
    if (record["tool"], record["version"], record["approval"]) != ("mintswarm-approval-decision", 2, "plan"):
        problems.append("tool, version or approval")
    plan_sha = hashlib.sha256((repo / ctx["plan"]).read_bytes()).hexdigest()
    subject = {
        "issue": 34,
        "component": ctx["component"],
        "plan": {"path": ctx["plan"], "sha256": plan_sha},
        "base": {"ref": ctx["commit"], "commit": ctx["commit"]},
    }
    if record["subject"] != subject:
        problems.append(f"subject {record['subject']}")
    if record["mode"] != mode:
        problems.append(f"mode {record['mode']!r}, expected {mode!r}")
    if type(record["min_confidence"]) is not int or record["min_confidence"] != threshold:
        problems.append(f"min_confidence {record['min_confidence']!r}, expected {threshold}")
    layers = record["layers"]
    if not isinstance(layers, dict) or list(layers) != ["floor", "judgement"]:
        return T.check(f"{name}: record shape", False, f"layers {layers!r}")
    fl, jd = layers["floor"], layers["judgement"]
    if list(fl) != FLOOR_LAYER_KEYS:
        return T.check(f"{name}: record shape", False, f"floor layer keys {list(fl)}")
    if list(jd) != JUDGEMENT_LAYER_KEYS:
        return T.check(f"{name}: record shape", False, f"judgement layer keys {list(jd)}")
    for label, layer in (("floor", fl), ("judgement", jd)):
        if layer["status"] not in ("usable", "unusable"):
            problems.append(f"{label} status {layer['status']!r}")
        if not isinstance(layer["problems"], list) or not all(isinstance(x, str) for x in layer["problems"]):
            problems.append(f"{label} problems not strings")
        elif (layer["status"] == "usable") == bool(layer["problems"]):
            problems.append(f"{label} {layer['status']} with {len(layer['problems'])} problems")

    verdict_path = repo / VERDICT
    verdict_bytes = verdict_path.read_bytes() if verdict_path.exists() else None
    if fl["path"] != str(VERDICT):
        problems.append("floor path")
    if fl["sha256"] != (hashlib.sha256(verdict_bytes).hexdigest() if verdict_bytes is not None else None):
        problems.append("floor sha256")
    if fl["status"] == "usable":
        verdict = json.loads(verdict_bytes)
        if (fl["verdict"], fl["rules"], fl["not_evaluated"]) != (
            verdict["verdict"],
            verdict["rules"],
            verdict["not_evaluated"],
        ):
            problems.append("floor layer differs from the verdict")
        if len(fl["rules"]) + len(fl["not_evaluated"]) != len(RULES) or not all(
            list(r) == ["rule", "result", "reasons"] for r in fl["rules"]
        ):
            problems.append(f"floor layer rules and not_evaluated are not the {len(RULES)} rules")
    elif (fl["verdict"], fl["rules"], fl["not_evaluated"]) != (None, [], []):
        problems.append("unusable floor layer holds content")

    assessment = repo / expect["assessment"]
    assessment_sha = hashlib.sha256(assessment.read_bytes()).hexdigest() if assessment.exists() else None
    as_configured = {"harness": expect["harness"], "model": expect["model"]} == EXPECTED_JUDGE
    fixed = {
        "stage": "assess",
        "path": expect["assessment"],
        "sha256": assessment_sha,
        "artifact": expect["artifact"],
        "harness": expect["harness"],
        "model": expect["model"],
        "expected": EXPECTED_JUDGE,
        "as_configured": as_configured,
    }
    wrong = [k for k, v in fixed.items() if jd[k] != v]
    if wrong:
        problems.append(f"judgement {', '.join(wrong)}")
    usable = jd["status"] == "usable"
    fields = (jd["confidence"], jd["band"], jd["hard_triggers"], jd["doubts"], jd["reasons"])
    if usable:
        band = BANDS.get(jd["band"]) if isinstance(jd["band"], str) else None
        if (
            band is None
            or type(jd["confidence"]) is not int
            or not band[1] <= jd["confidence"] <= band[2]
            or not all(t in HARD_TRIGGERS for t in jd["hard_triggers"])
            or any(t not in AUTHORITY_TRIGGERS for t in jd["hard_triggers"]) != (jd["band"] == "blocked")
            or len(jd["reasons"]) != 1
        ):
            problems.append(f"judgement {fields!r}")
        want = expect["envelope"]
        if want is not None and fields != (
            want["confidence"],
            want["band"],
            want["hard_triggers"],
            want["doubts"],
            [want["reason"]],
        ):
            problems.append(f"judgement {fields!r} differs from the envelope")
    elif fields != (None, None, [], [], []):
        problems.append("unusable judgement layer holds content")

    clean = fl["status"] == "usable" and fl["verdict"] == "clean"
    confident = usable and not jd["hard_triggers"] and jd["confidence"] >= threshold
    judged = "auto" if clean and confident and as_configured else "human"
    decision = {"always": "human", "shadow": "human", "never": "auto", "judged": judged}.get(mode)
    if record["judged_decision"] != judged:
        problems.append(f"judged_decision {record['judged_decision']!r}, expected {judged!r}")
    if record["decision"] != decision:
        problems.append(f"decision {record['decision']!r}, expected {decision!r}")
    if proc.returncode != (0 if record["decision"] == "auto" else 1):
        problems.append(f"exit {proc.returncode} with decision {record['decision']!r}")
    reasons = record["reasons"]
    if (
        not isinstance(reasons, list)
        or not all(isinstance(x, str) for x in reasons)
        or not reasons
        or not reasons[0].startswith(f"mode {mode}:")
    ):
        problems.append(f"reasons {reasons!r}")
    elif usable:
        triggers = jd["hard_triggers"]
        wanted = [f"judgement: confidence {jd['confidence']} (band {jd['band']})"]
        unusable = [t for t in triggers if t not in AUTHORITY_TRIGGERS]
        authority = [t for t in triggers if t in AUTHORITY_TRIGGERS]
        if unusable:
            wanted.append(f"judgement: hard trigger {', '.join(unusable)}")
        if authority:
            wanted.append(f"judgement: requires the maintainer: {', '.join(authority)}")
        if not triggers and jd["confidence"] < threshold:
            wanted.append(f"judgement: confidence below min_confidence {threshold}")
        listed = [x for x in reasons if x.startswith("judgement: ") and "as configured" not in x]
        if listed != wanted:
            problems.append(f"judgement reasons {listed!r}, expected {wanted!r}")
    if record["decision"] == "human" and mode == "shadow":
        if not proc.stdout.startswith("### Shadow record: plan approval\n"):
            problems.append("no shadow-record comment in shadow mode")
    elif record["decision"] == "human" and proc.stdout != "":
        problems.append("stdout not empty for human")
    if record["decision"] == "auto" and "approved without a human" not in proc.stdout:
        problems.append("no comment for auto")
    if len(proc.stderr.strip().splitlines()) != 1:
        problems.append(f"stderr {proc.stderr!r}")
    return T.check(f"{name}: record shape", not problems, "; ".join(problems))


def expect_decision(
    name: str, result, decision: str, judged: str = None, floor_status: str = None, judgement_status: str = None
) -> dict:
    proc, record = result
    if not isinstance(record, dict) or not isinstance(record.get("layers"), dict):
        T.check(f"{name}: decision {decision}", False, f"no record (exit {proc.returncode})")
        return {}
    got = {"decision": record["decision"]}
    want = {"decision": decision}
    if judged is not None:
        got["judged"], want["judged"] = record["judged_decision"], judged
    if floor_status is not None:
        got["floor"], want["floor"] = record["layers"]["floor"]["status"], floor_status
    if judgement_status is not None:
        got["judgement"], want["judgement"] = record["layers"]["judgement"]["status"], judgement_status
    T.check(
        f"{name}: decision {decision}" + "".join(f", {k} {v}" for k, v in want.items() if k != "decision"),
        got == want and proc.returncode == (0 if decision == "auto" else 1),
        f"got {got}, exit {proc.returncode}",
    )
    return record


def check_comment(name: str, proc, floor_line: str, confidence: str, triggers: str = "none") -> None:
    out = proc.stdout
    lines = out.splitlines()
    ok = (
        bool(lines)
        and lines[0] == "### Plan approved without a human"
        and f"- **Floor:** {floor_line}" in lines
        and f"- **Confidence:** {confidence}" in lines
        and (triggers is None or f"- **Hard triggers:** {triggers}" in lines)
        and any(
            line.startswith("**To veto:**") and "awaiting plan approval" in line and "rejection at PR approval" in line
            for line in lines
        )
        and "PR approval stays with a human in every mode." in lines
    )
    T.check(f"{name}: comment has the heading, floor, confidence, hard triggers and veto", ok, repr(out))


def check_shadow_comment(name: str, proc, floor_start: str, confidence: str, judged: str) -> None:
    lines = proc.stdout.splitlines()
    floor = next((line for line in lines if line.startswith("- **Floor:** ")), "")
    ok = (
        bool(lines)
        and lines[0] == "### Shadow record: plan approval"
        and any(line.startswith("- **Plan:** ") and "footprint: " in line for line in lines)
        and any(
            line.startswith("- **Assess:** stage `assess`, `claude` with `sonnet`, as configured") for line in lines
        )
        and f"- **Confidence:** {confidence}" in lines
        and "- **Hard triggers:** none" in lines
        and floor_start in floor
        and "clean: `" in floor
        and f"- **Would-be decision:** `{judged}` (`judged_decision`)" in lines
        and "- **Decision:** `human`" in lines
        and "- **Maintainer decision:** pending" in lines
    )
    T.check(f"{name}: shadow-record comment has the heading and rows", ok, repr(proc.stdout))


def check_decision_bands(d: Decider) -> None:
    proc, record = d.run("judged clean safe 95")
    expect_decision("judged clean safe 95", (proc, record), "auto", "auto", "usable", "usable")
    check_comment("judged clean safe 95", proc, f"clean, {len(RULES)} of {len(RULES)} rules", SAFE_LINE)
    expect_decision(
        "judged hit safe 100",
        d.run("judged hit safe 100", plan="hit", assessment=envelope("safe", 100)),
        "human",
        "human",
        "usable",
        "usable",
    )
    for trigger in UNUSABLE_TRIGGERS:
        name = f"judged blocked 0 {trigger}"
        proc, record = d.run(name, assessment=envelope("blocked", 0, [trigger]))
        expect_decision(name, (proc, record), "human", "human", "usable", "usable")
    for triggers in [[t] for t in AUTHORITY_TRIGGERS] + [AUTHORITY_TRIGGERS]:
        name = f"judged safe 100 {' '.join(triggers)}"
        proc, record = d.run(name, assessment=envelope("safe", 100, triggers))
        expect_decision(name, (proc, record), "human", "human", "usable", "usable")
        want = f"judgement: requires the maintainer: {', '.join(triggers)}"
        T.check(f"{name}: reason {want!r}", want in record.get("reasons", []), repr(record.get("reasons")))
    name = "judged needs-review 62 weakens-safeguard"
    expect_decision(
        name,
        d.run(name, assessment=envelope("needs-review", 62, ["weakens-safeguard"])),
        "human",
        "human",
        "usable",
        "usable",
    )
    for trigger in UNUSABLE_TRIGGERS:
        for band, confidence in (("safe", 100), ("needs-review", 60)):
            name = f"judged {band} {confidence} unusable {trigger}"
            expect_decision(
                name,
                d.run(name, assessment=envelope(band, confidence, [trigger])),
                "human",
                "human",
                "usable",
                "unusable",
            )
    name = "judged blocked 0 only credentials"
    expect_decision(
        name, d.run(name, assessment=envelope("blocked", 0, ["credentials"])), "human", "human", "usable", "unusable"
    )
    name = "judged blocked 0 criterion-not-met credentials"
    expect_decision(
        name,
        d.run(name, assessment=envelope("blocked", 0, ["criterion-not-met", "credentials"])),
        "human",
        "human",
        "usable",
        "usable",
    )
    proc, record = d.run(
        "never blocked two triggers",
        config="never",
        assessment=envelope("blocked", 0, ["criterion-not-met", "credentials"]),
    )
    expect_decision("never blocked two triggers", (proc, record), "auto", "human", "usable", "usable")
    check_comment(
        "never blocked two triggers",
        proc,
        f"clean, {len(RULES)} of {len(RULES)} rules",
        confidence_line("blocked", 0, authority=["credentials"]),
        "`criterion-not-met`",
    )
    edges = [
        ("uncertain", 1, "human"),
        ("uncertain", 49, "human"),
        ("needs-review", 50, "human"),
        ("needs-review", 79, "human"),
        ("likely-safe", 80, "human"),
        ("likely-safe", 89, "human"),
        ("likely-safe", 90, "auto"),
        ("likely-safe", 94, "auto"),
        ("safe", 95, "auto"),
        ("safe", 100, "auto"),
    ]
    for band, confidence, want in edges:
        name = f"judged {band} {confidence}"
        expect_decision(name, d.run(name, assessment=envelope(band, confidence)), want, want, "usable", "usable")
    for config, confidence, want in (
        ("judged-95", 94, "human"),
        ("judged-95", 95, "auto"),
        ("min-100", 99, "human"),
        ("min-100", 100, "auto"),
    ):
        band = "safe" if confidence >= 95 else "likely-safe"
        name = f"{config} {band} {confidence}"
        proc, record = d.run(name, config=config, assessment=envelope(band, confidence))
        expect_decision(name, (proc, record), want, want, "usable", "usable")
        if want == "auto":
            check_comment(
                name,
                proc,
                f"clean, {len(RULES)} of {len(RULES)} rules",
                confidence_line(band, confidence, CONFIG_MIN_CONFIDENCE[config]),
            )

    doubts = [DOUBT, {"statement": "a second doubt", "consequence": "a stale link", "resolution": "run the checker"}]
    proc, record = d.run("judged likely-safe two doubts", assessment=envelope("likely-safe", 92, doubts=doubts))
    expect_decision("judged likely-safe two doubts", (proc, record), "auto", "auto", "usable", "usable")
    rows = [line for line in proc.stdout.splitlines() if line.startswith("- **Doubt:** ")]
    T.check(
        "judged likely-safe two doubts: one Doubt row per doubt, in order",
        rows
        == [
            f"- **Doubt:** `{x['statement']}`; if wrong: `{x['consequence']}`; resolved by: `{x['resolution']}`"
            for x in doubts
        ]
        and f"- **Confidence:** {confidence_line('likely-safe', 92)}" in proc.stdout.splitlines(),
        repr(proc.stdout),
    )


def malformed_assessments() -> list:
    """(label, content): each makes the judgement unusable."""

    def keyed(change) -> str:
        document = dict(SAFE)
        change(document)
        return json.dumps(document, indent=2)

    def doubt_without(key: str) -> dict:
        return {k: v for k, v in DOUBT.items() if k != key}

    raw = json.dumps(SAFE, indent=2)
    return [
        ("missing assessment", None),
        ("assessment not UTF-8", b"# Assessment\n\n95 confidence: x\n\n```json\n{}\n```\n\xff\n"),
        ("no envelope", assessment_text(fence="Envelope:", close="")),
        ("two envelopes", assessment_text(extra="\n```json\n" + raw + "\n```\n")),
        ("unclosed fence", assessment_text(close=None)),
        ("a four-backtick fence", assessment_text(fence="````json", close="````")),
        ("envelope not JSON", assessment_text("{ not json")),
        ("a lone surrogate in the reason", assessment_text(raw.replace(json.dumps(REASON), '"x \\ud800 y"'))),
        (
            "a lone surrogate in a doubt",
            assessment_text(json.dumps(envelope("likely-safe", 92), indent=2).replace("twice", "\\udc00")),
        ),
        ("envelope a list", assessment_text("[" + raw + "]")),
        ("duplicate key", assessment_text(raw.replace('"band": "safe",', '"band": "safe",\n  "band": "safe",'))),
        ("NaN confidence", assessment_text(raw.replace('"confidence": 95', '"confidence": NaN'))),
        ("missing key", assessment_text(keyed(lambda e: e.pop("doubts")))),
        ("extra key", assessment_text(keyed(lambda e: e.update(samples=1)))),
        ("reordered keys", assessment_text(keyed(lambda e: e.update(band=e.pop("band"))))),
        ("confidence true", assessment_text(envelope("blocked", True))),
        ("confidence a float", assessment_text(envelope("safe", 95.0))),
        ("confidence a string", assessment_text(envelope("safe", "95"))),
        ("confidence -1", assessment_text(envelope("blocked", -1))),
        ("confidence 101", assessment_text(envelope("safe", 101))),
        ("unknown band", assessment_text(envelope("risky", 95))),
        ("band as a range", assessment_text(envelope("80-94", 92, doubts=[DOUBT]))),
        ("band as its display name", assessment_text(envelope("Likely safe", 92, doubts=[DOUBT]))),
        ("safe at 90", assessment_text(envelope("safe", 90))),
        ("likely-safe at 95", assessment_text(envelope("likely-safe", 95))),
        ("blocked at 1", assessment_text(envelope("blocked", 1))),
        ("blocked without a trigger", assessment_text(envelope("blocked", 0, []))),
        ("an unusable trigger outside blocked", assessment_text(envelope("needs-review", 60, ["criterion-not-met"]))),
        ("an unknown trigger", assessment_text(envelope("blocked", 0, ["risky"]))),
        ("a repeated trigger", assessment_text(envelope("blocked", 0, ["credentials", "credentials"]))),
        ("hard_triggers not a list", assessment_text(envelope("blocked", 0, "credentials"))),
        ("empty reason", assessment_text(envelope(reason=""))),
        ("reason of format characters", assessment_text(envelope(reason="\u200b\u2060 "))),
        ("reason not a string", assessment_text(envelope(reason=["a reason"]))),
        ("a doubt missing a field", assessment_text(envelope("likely-safe", 92, doubts=[doubt_without("resolution")]))),
        (
            "a doubt with an empty field",
            assessment_text(envelope("likely-safe", 92, doubts=[dict(DOUBT, statement="")])),
        ),
        ("11 doubts", assessment_text(envelope("likely-safe", 92, doubts=[DOUBT] * 11))),
        ("safe with a doubt", assessment_text(envelope("safe", 95, doubts=[DOUBT]))),
        ("likely-safe without a doubt", assessment_text(envelope("likely-safe", 92, doubts=[]))),
        ("a Risk: line", assessment_text(extra="Risk: low\n")),
        ("summary confidence differs", assessment_text(summary=f"96 confidence: {REASON}")),
        ("summary as a risk level", assessment_text(summary=f"low risk: {REASON}")),
        ("summary without reason", assessment_text(summary="95 confidence: ")),
        ("summary reason of format characters", assessment_text(summary="95 confidence: \u200b\u2060")),
        ("no summary line", assessment_text(summary="A plan.")),
        ("no title", assessment_text(title="Assessment: selftest case")),
    ]


def check_decision_judged(d: Decider) -> None:
    check_decision_bands(d)
    expect_decision("judged no verdict", d.run("judged no verdict", verdict=None), "human", "human", "unusable")
    expect_decision(
        "judged unparsable verdict",
        d.run("judged unparsable verdict", verdict=b"{ not json\n"),
        "human",
        "human",
        "unusable",
    )
    duplicate = d.prepare("judged", "clean")["verdict"].replace(
        b'"mode": "judged",', b'"mode": "judged",\n  "mode": "judged",', 1
    )
    expect_decision(
        "judged duplicate key", d.run("judged duplicate key", verdict=duplicate), "human", "human", "unusable"
    )

    def setter(*path, value):
        def edit(document):
            target = document
            for key in path[:-1]:
                target = target[key]
            target[path[-1]] = value

        return edit

    def missing_rule(document):
        document["rules"].pop()

    def hit_in_clean(document):
        document["rules"][1] = {"rule": RULES[1], "result": "hit", "reasons": ["edited"]}

    def skipped(document):
        document["not_evaluated"] = [document["rules"].pop()["rule"]]

    def reordered(document):
        mode = document.pop("mode")
        document["mode"] = mode

    edits = [
        ("plan sha256", setter("plan", "sha256", value="0" * 64)),
        ("base commit", setter("base", "commit", value=d.commits["always"])),
        ("component", setter("component", value="web")),
        ("mode", setter("mode", value="never")),
        ("tool", setter("tool", value="another-floor")),
        ("version 1", setter("version", value=1)),
        ("issue_document sha256", setter("issue_document", "sha256", value="0" * 64)),
        ("analysis sha256", setter("analysis", "sha256", value="0" * 64)),
        ("issue_document null", setter("issue_document", value=None)),
        ("version true", setter("version", value=True)),
        ("a missing rule", missing_rule),
        ("clean verdict with a hit rule", hit_in_clean),
        ("non-empty not_evaluated", skipped),
        ("keys out of order", reordered),
    ]
    for label, edit in edits:
        expect_decision(
            f"judged verdict {label}",
            d.run(f"judged verdict {label}", verdict=edit),
            "human",
            "human",
            "unusable",
            "usable",
        )

    for label, content in malformed_assessments():
        expect_decision(
            f"judged {label}", d.run(f"judged {label}", assessment=content), "human", "human", "usable", "unusable"
        )

    for label, content in (
        ("trailing space", assessment_text(fence="```json \t", close="``` ")),
        ("CRLF", assessment_text(eol="\r\n")),
    ):
        proc, record = d.run(f"judged {label}", assessment=content)
        expect_decision(f"judged {label}", (proc, record), "auto", "auto", "usable", "usable")
        T.check(
            f"judged {label}: safe 95 with the reason",
            isinstance(record, dict)
            and record["layers"]["judgement"]["reasons"] == [REASON]
            and record["layers"]["judgement"]["confidence"] == 95,
        )

    for label, harness, model in (("other model", "claude", "opus"), ("other harness", "codex", "sonnet")):
        proc, record = d.run(f"judged {label}", harness=harness, model=model)
        expect_decision(f"judged {label}", (proc, record), "human", "human", "usable", "usable")
        T.check(
            f"judged {label}: as_configured false, with a reason",
            isinstance(record, dict)
            and record["layers"]["judgement"]["as_configured"] is False
            and "judgement: harness or model not as configured" in record["reasons"],
        )

    clean_verdict = json.loads(d.prepare("judged", "clean")["verdict"])
    hit_verdict = json.loads(d.prepare("judged", "hit")["verdict"])
    forged = dict(clean_verdict, plan=hit_verdict["plan"])
    proc, record = d.run(
        "judged forged clean verdict", plan="hit", verdict=(json.dumps(forged, indent=2) + "\n").encode("utf-8")
    )
    expect_decision("judged forged clean verdict", (proc, record), "human", "human", "unusable", "usable")
    T.check(
        "judged forged clean verdict: the recomputed rules differ",
        isinstance(record, dict)
        and record["layers"]["floor"]["problems"]
        == ["the floor verdict's rules differ from what floor.py gives for this plan and base"],
        str(record and record.get("layers", {}).get("floor", {}).get("problems")),
    )

    def other_reason(document):
        for rule in document["rules"]:
            if rule["result"] == "hit":
                rule["reasons"] = ["an edited reason"]

    expect_decision(
        "judged verdict reason edited",
        d.run("judged verdict reason edited", plan="hit", verdict=other_reason),
        "human",
        "human",
        "unusable",
    )


def floor_problems(record) -> list:
    if not isinstance(record, dict):
        return []
    return record.get("layers", {}).get("floor", {}).get("problems")


def check_decision_documents(d: Decider) -> None:
    proc, record = d.run("judged blocking question", plan="blocking")
    expect_decision("judged blocking question", (proc, record), "human", "human", "usable", "usable")
    floor_ = record.get("layers", {}).get("floor", {}) if isinstance(record, dict) else {}
    T.check(
        "judged blocking question: the floor layer holds the blocking-questions hit",
        [r["rule"] for r in floor_.get("rules", []) if r["result"] == "hit"] == ["blocking-questions"],
        str(floor_.get("rules")),
    )
    proc, record = d.run("judged recorded must_decide question", plan="recorded")
    expect_decision("judged recorded must_decide question", (proc, record), "human", "human", "usable", "usable")
    floor_ = record.get("layers", {}).get("floor", {}) if isinstance(record, dict) else {}
    T.check(
        "judged recorded must_decide question: the floor layer holds the record's blocking-questions hit",
        [(r["rule"], r["reasons"]) for r in floor_.get("rules", []) if r["result"] == "hit"]
        == [("blocking-questions", ["analysis: blocking question (must_decide): Which component owns it?"])],
        str(floor_.get("rules")),
    )
    proc, record = d.run("judged recorded assumptions", plan="assumed")
    expect_decision("judged recorded assumptions", (proc, record), "auto", "auto", "usable", "usable")
    assumed = d.prepare("judged", "assumed")
    kept = assumed["analysis_record"].read_bytes()
    assumed["analysis_record"].write_text(result_record([("recommended", "Is the name free?")]), encoding="utf-8")
    try:
        proc, record = d.run("judged record rewritten after the floor", plan="assumed")
    finally:
        assumed["analysis_record"].write_bytes(kept)
    expect_decision("judged record rewritten after the floor", (proc, record), "human", "human", "unusable", "usable")
    T.check(
        "judged record rewritten after the floor: the recomputed rules differ",
        floor_problems(record) == ["the floor verdict's rules differ from what floor.py gives for this plan and base"],
        str(floor_problems(record)),
    )
    proc, record = d.run("judged footprint hit", plan="no footprint")
    expect_decision("judged footprint hit", (proc, record), "human", "human", "usable", "usable")
    floor_ = record.get("layers", {}).get("floor", {}) if isinstance(record, dict) else {}
    T.check(
        "judged footprint hit: footprint rules not evaluated, document rules evaluated",
        floor_.get("not_evaluated") == FOOTPRINT_RULES
        and [r["rule"] for r in floor_.get("rules", [])] == ["footprint", *DOCUMENT_RULES],
        str(floor_.get("not_evaluated")),
    )
    proc, record = d.run("shadow blocking question", config="real", plan="blocking")
    expect_decision("shadow blocking question", (proc, record), "human", "human", "usable", "usable")
    check_shadow_comment(
        "shadow blocking question",
        proc,
        "hit on `blocking-questions` (`floor.py` exit 1)",
        SAFE_LINE,
        "human",
    )

    clean = d.prepare("judged", "clean")
    without = d.floor_verdict(clean, documents=False)
    proc, record = d.run("judged verdict without documents", verdict=without)
    expect_decision("judged verdict without documents", (proc, record), "human", "human", "unusable", "usable")
    T.check(
        "judged verdict without documents: both bindings named",
        floor_problems(record)
        == [
            "the floor verdict's issue_document does not match --issue-document",
            "the floor verdict's analysis does not match --analysis",
        ],
        str(floor_problems(record)),
    )
    other = d.sb.write_plan("decision other analysis", analysis_document("- **Non-blocking (assumed):** Other?\n"))
    proc, record = d.run("judged verdict with another analysis", verdict=d.floor_verdict(clean, analysis=other))
    expect_decision("judged verdict with another analysis", (proc, record), "human", "human", "unusable", "usable")
    T.check(
        "judged verdict with another analysis: the analysis binding named",
        floor_problems(record) == ["the floor verdict's analysis does not match --analysis"],
        str(floor_problems(record)),
    )
    other = d.sb.write_plan("decision other issue", issue_document() + "\nAnother comment.\n")
    proc, record = d.run("judged verdict with another issue", verdict=d.floor_verdict(clean, issue=other))
    expect_decision("judged verdict with another issue", (proc, record), "human", "human", "unusable", "usable")
    T.check(
        "judged verdict with another issue: the issue_document binding named",
        floor_problems(record) == ["the floor verdict's issue_document does not match --issue-document"],
        str(floor_problems(record)),
    )

    def first_of_kind_clean(document):
        for rule in document["rules"]:
            if rule["rule"] == "first-of-kind":
                rule.update(result="clean", reasons=[])
        document["verdict"] = "clean"

    expect_decision(
        "judged kind plan", d.run("judged kind plan", plan="kind plan"), "human", "human", "usable", "usable"
    )
    proc, record = d.run("judged forged first-of-kind clean", plan="kind plan", verdict=first_of_kind_clean)
    expect_decision("judged forged first-of-kind clean", (proc, record), "human", "human", "unusable", "usable")
    T.check(
        "judged forged first-of-kind clean: the recomputed rules differ",
        floor_problems(record) == ["the floor verdict's rules differ from what floor.py gives for this plan and base"],
        str(floor_problems(record)),
    )

    def footprint_skips_one_more(document):
        document["rules"] = [r for r in document["rules"] if r["rule"] != "first-of-kind"]
        document["not_evaluated"] = FOOTPRINT_RULES + ["first-of-kind"]

    def footprint_rules_skipped_on_clean(document):
        document["rules"] = [r for r in document["rules"] if r["rule"] not in FOOTPRINT_RULES]
        document["not_evaluated"] = FOOTPRINT_RULES

    for label, plan, edit, problem in (
        (
            "a document rule skipped",
            "no footprint",
            footprint_skips_one_more,
            "the floor verdict's rule identifiers are not floor.py's, in order, "
            "with none or only the footprint rules not evaluated",
        ),
        (
            "footprint rules skipped without a hit",
            "clean",
            footprint_rules_skipped_on_clean,
            "the floor verdict skips rules without a footprint hit",
        ),
    ):
        proc, record = d.run(f"judged verdict {label}", plan=plan, verdict=edit)
        expect_decision(f"judged verdict {label}", (proc, record), "human", "human", "unusable", "usable")
        T.check(
            f"judged verdict {label}: the identifier check names it",
            floor_problems(record) == [problem],
            str(floor_problems(record)),
        )


def check_decision_modes(d: Decider) -> None:
    blocked = envelope("blocked", 0, ["criterion-not-met", "credentials"])
    expect_decision("always clean safe", d.run("always clean safe", config="always"), "human", "auto")
    expect_decision(
        "always hit blocked",
        d.run("always hit blocked", config="always", plan="hit", assessment=blocked),
        "human",
        "human",
    )
    proc, record = d.run("shadow clean safe", config="real")
    expect_decision("shadow clean safe", (proc, record), "human", "auto")
    check_shadow_comment(
        "shadow clean safe",
        proc,
        f"clean, {len(RULES)} of {len(RULES)} rules (`floor.py` exit 0)",
        SAFE_LINE,
        "auto",
    )
    proc, record = d.run("shadow hit safe", config="real", plan="hit")
    expect_decision("shadow hit safe", (proc, record), "human", "human")
    check_shadow_comment(
        "shadow hit safe", proc, "hit on `require-human-paths` (`floor.py` exit 1)", SAFE_LINE, "human"
    )
    proc, record = d.run("shadow clean uncertain", config="real", assessment=envelope("uncertain", 30))
    expect_decision("shadow clean uncertain", (proc, record), "human", "human")
    check_shadow_comment(
        "shadow clean uncertain",
        proc,
        f"clean, {len(RULES)} of {len(RULES)} rules (`floor.py` exit 0)",
        confidence_line("uncertain", 30),
        "human",
    )
    proc, record = d.run("shadow unusable judgement", config="real", assessment=assessment_text(summary="A plan."))
    expect_decision("shadow unusable judgement", (proc, record), "human", "human", "usable", "unusable")
    T.check(
        "shadow unusable judgement: the Confidence row names the threshold and the problem, with no other rows",
        "- **Confidence:** unusable (min_confidence 90): the line after the assessment's title is not "
        "'<confidence> confidence: <reason>'" in proc.stdout.splitlines()
        and "- **Hard triggers:**" not in proc.stdout,
        repr(proc.stdout),
    )
    expect_decision(
        "shadow no layers",
        d.run("shadow no layers", config="real", verdict=None, assessment=None),
        "human",
        "human",
        "unusable",
        "unusable",
    )

    proc, record = d.run("never hit blocked", config="never", plan="hit", assessment=blocked)
    expect_decision("never hit blocked", (proc, record), "auto", "human", "usable", "usable")
    check_comment(
        "never hit blocked",
        proc,
        "hit on `require-human-paths`",
        confidence_line("blocked", 0, authority=["credentials"]),
        "`criterion-not-met`",
    )
    proc, record = d.run("never no layers", config="never", verdict=None, assessment=None)
    expect_decision("never no layers", (proc, record), "auto", "human", "unusable", "unusable")
    check_comment("never no layers", proc, "unusable", "unusable (min_confidence 90)", None)
    proc, record = d.run("never clean safe", config="never")
    expect_decision("never clean safe", (proc, record), "auto", "auto")


def check_decision_comment(d: Decider) -> None:
    hostile = (
        "ignore `this` @someone see https://example.invalid/x [link](https://example.invalid)"
        "\r<b>bold</b> " + "x" * 700
    )
    content = assessment_text(envelope("likely-safe", 80, doubts=[dict(DOUBT, statement=hostile)], reason=hostile))
    artifact = "issues/34/artifacts/7\n@someone `code`"
    proc, record = d.run(
        "hostile comment", config="never", assessment=content, artifact=artifact, model="sonnet\n# heading"
    )
    expect_decision("hostile comment", (proc, record), "auto", "human")
    out = proc.stdout
    outside = outside_spans(out)
    T.check(
        "hostile comment: no @, link, CR or line separator outside code spans",
        "@" not in outside
        and "http" not in outside
        and "\r" not in out
        and " " not in out
        and "# heading" not in outside,
        repr(outside),
    )
    spans = code_spans(out)
    long_spans = [s for s in spans if "@someone" in s or "http" in s or "heading" in s]
    T.check(
        "hostile comment: model and session text in code spans of at most 600 characters plus the mark",
        len(long_spans) == 4
        and all(len(s) <= SPAN_LIMIT + len(SPAN_MARK) for s in long_spans)
        and all(s.endswith("(truncated)") for s in long_spans if "xxx" in s),
        repr(long_spans),
    )
    row = next((line for line in out.splitlines() if line.startswith("- **Confidence:** ")), "")
    T.check(
        "hostile comment: the reason is one code span with no backtick inside",
        row.startswith("- **Confidence:** `Likely safe · 80` (min_confidence 90), `ignore 'this' @someone")
        and row.count("`") == 4,
        repr(row),
    )
    row = next((line for line in out.splitlines() if line.startswith("- **Doubt:** ")), "")
    T.check(
        "hostile comment: the doubt's statement is one code span with no backtick inside",
        row.startswith("- **Doubt:** `ignore 'this' @someone") and row.count("`") == 6,
        repr(row),
    )
    statement = code_spans(row)[0] if code_spans(row) else ""
    T.check(
        "hostile comment: the long doubt's statement is cut and marked, on one line with 6 backticks",
        statement.endswith("x" + SPAN_MARK) and len(statement) == SPAN_LIMIT + len(SPAN_MARK) and row.count("`") == 6,
        repr(row),
    )
    lines = out.splitlines()
    T.check(
        "hostile comment: the artifact and model stay on one line",
        any(line.startswith("- **Assessment:** `issues/34/artifacts/7 @someone 'code'`") for line in lines)
        and not any(line.startswith("# ") for line in lines),
        repr(lines),
    )

    proc, record = d.run(
        "hostile shadow comment", config="real", assessment=content, artifact=artifact, model="sonnet\n# heading"
    )
    expect_decision("hostile shadow comment", (proc, record), "human", "human")
    out = proc.stdout
    outside = outside_spans(out)
    T.check(
        "hostile shadow comment: no @, link, CR or line separator outside code spans",
        out.startswith("### Shadow record")
        and "@" not in outside
        and "http" not in outside
        and "\r" not in out
        and "\u2028" not in out
        and "# heading" not in outside,
        repr(outside),
    )
    spans = [s for s in code_spans(out) if "@someone" in s or "http" in s or "heading" in s]
    T.check(
        "hostile shadow comment: model and session text in code spans of at most 600 characters plus the mark",
        len(spans) == 4
        and all(len(s) <= SPAN_LIMIT + len(SPAN_MARK) for s in spans)
        and all(s.endswith("(truncated)") for s in spans if "xxx" in s),
        repr(spans),
    )
    lines = out.splitlines()
    T.check(
        "hostile shadow comment: no line starts a heading or list item from model text",
        not any(line.startswith("# ") for line in lines)
        and all(line.startswith(("### ", "Mode ", "- **")) or not line for line in lines),
        repr(lines),
    )


def check_decision_unusable(sb: Sandbox, d: Decider) -> None:
    ctx = d.prepare("judged", "clean")
    assessment = sb.write_plan("unusable assessment", assessment_text())
    not_utf8 = sb.write_plan("decision not utf-8", b"# Plan\n\n\xff\xfe\n")

    def args(**changes) -> list:
        values = {
            "--issue": "34",
            "--component": "docs",
            "--plan": ctx["plan"],
            "--issue-document": ctx["issue"],
            "--analysis": ctx["analysis"],
            "--assessment": assessment,
            "--assessment-artifact": ARTIFACT,
            "--base": ctx["commit"],
            "--harness": "claude",
            "--model": "sonnet",
        }
        values.update({f"--{k.replace('_', '-')}": v for k, v in changes.items()})
        out = []
        for key, value in values.items():
            if value is not None:
                out += [key, value]
        return out

    record = sb.repo / RECORD
    cases = [
        ("no arguments", []),
        ("unknown argument", args() + ["--extra", "x"]),
        ("argument twice", args() + ["--model", "opus"]),
        ("missing value", args()[:-1]),
        ("missing harness", args(harness=None)),
        ("issue 0", args(issue="0")),
        ("issue 034", args(issue="034")),
        ("issue -3", args(issue="-3")),
        ("issue abc", args(issue="abc")),
        ("base starting with -", args(base="-HEAD")),
        ("missing plan", args(plan="cases/missing.md")),
        ("plan not UTF-8", args(plan=not_utf8)),
        ("unresolvable base", args(base="no-such-ref")),
        ("base without config", args(base=d.commits["noconfig"])),
        ("unknown component", args(component="nope")),
        ("config mode-sometimes", args(base=d.commits["mode-sometimes"])),
        ("config unknown-key", args(base=d.commits["unknown-key"])),
        *((f"config {name}", args(base=d.commits[name])) for name in MIN_CONFIDENCE_INVALID),
        ("missing --issue-document", args(issue_document=None)),
        ("missing --analysis", args(analysis=None)),
        ("issue document missing", args(issue_document="cases/missing.md")),
        ("issue document unreadable", args(issue_document="cases")),
        ("issue document not UTF-8", args(issue_document=not_utf8)),
        ("analysis missing", args(analysis="cases/missing.md")),
        ("analysis unreadable", args(analysis="cases")),
        ("analysis not UTF-8", args(analysis=not_utf8)),
        ("screen record missing", args(screen="cases/missing.md")),
        ("screen record unreadable", args(screen="cases")),
        ("screen record not UTF-8", args(screen=not_utf8)),
        ("screen record twice", args(screen=assessment) + ["--screen", assessment]),
        ("screen record empty value", args() + ["--screen", ""]),
    ]
    for name, argv in cases:
        record.write_text('{"stale": true}\n', encoding="utf-8")
        proc = sb.exec(argv, script=DECISION)
        T.check(
            f"decision unusable {name}: exit 2, no record, empty stdout",
            proc.returncode == 2
            and not record.exists()
            and proc.stdout == ""
            and len(proc.stderr.strip().splitlines()) == 1,
            f"exit {proc.returncode}, record {'left' if record.exists() else 'gone'}: {proc.stderr.strip()}",
        )

    bindir = sb.tmp / "git-only-decision"
    bindir.mkdir()
    (bindir / "git").symlink_to(shutil.which("git"))
    record.write_text('{"stale": true}\n', encoding="utf-8")
    proc = sb.exec(args(), env=dict(sb.env, PATH=str(bindir)), script=DECISION)
    T.check(
        "decision unusable yq missing: exit 2, no record",
        proc.returncode == 2 and not record.exists(),
        f"exit {proc.returncode}: {proc.stderr.strip()}",
    )

    record.unlink(missing_ok=True)
    record.mkdir()
    try:
        proc = sb.exec(args(), script=DECISION)
        T.check(
            "decision record a directory: exit 2, one-line error, no record written",
            proc.returncode == 2
            and len(proc.stderr.strip().splitlines()) == 1
            and "Traceback" not in proc.stderr
            and record.is_dir()
            and not any(record.iterdir())
            and not list(record.parent.glob(".decision.*"))
            and proc.stdout == "",
            f"exit {proc.returncode}: {proc.stderr.strip()}",
        )
    finally:
        shutil.rmtree(record, ignore_errors=True)


def check_decision(sb: Sandbox, commits: dict) -> None:
    d = Decider(sb, commits)
    check_decision_judged(d)
    check_decision_documents(d)
    check_decision_modes(d)
    check_decision_comment(d)
    check_decision_unusable(sb, d)
    check_decision_screen(d)


def check_decision_screen(d: Decider) -> None:
    proc, record = d.run("shadow two packages", config="real", plan="packages")
    expect_decision("shadow two packages", (proc, record), "human", "human", "usable", "usable")
    floor_ = record.get("layers", {}).get("floor", {}) if isinstance(record, dict) else {}
    T.check(
        "shadow two packages: the floor layer holds the dependency-screen hit",
        [r["rule"] for r in floor_.get("rules", []) if r["result"] == "hit"] == ["dependency-screen"],
        str(floor_.get("rules")),
    )
    lines = proc.stdout.splitlines()
    rows = [line for line in lines if line.startswith("- **Package:** ")]
    floor_at = next((i for i, line in enumerate(lines) if line.startswith("- **Floor:** ")), None)
    evidence = (
        "dependencies `2` direct, `5` transitive; publisher `alice`, publisher_changed `false`; "
        "age_days `2000`, releases `40`"
    )
    T.check(
        "shadow two packages: one Package row per declared package, right after the Floor row",
        rows
        == [
            f"- **Package:** `good-pkg` in `{MANIFEST}`, version `1.2.3`; status `screened`; disqualifiers: none; "
            + evidence,
            f"- **Package:** `bad-pkg` in `{MANIFEST}`, version `0.9.0`; status `screened`; disqualifiers: "
            "`advisory` (`GHSA-0000-1111-2222`), `typosquat` of `good-pkg`; " + evidence,
        ]
        and floor_at is not None
        and lines[floor_at + 1 : floor_at + 3] == rows,
        repr(lines),
    )
    T.check(
        "shadow two packages: the footprint in brief counts declared packages",
        any(line.startswith("- **Plan:** ") and "2 declared packages" in line for line in lines),
        repr(lines),
    )

    proc, record = d.run("hostile package name", config="real", plan="hostile package")
    expect_decision("hostile package name", (proc, record), "human", "auto", "usable", "usable")
    out = proc.stdout
    outside = outside_spans(out)
    rows = [line for line in out.splitlines() if line.startswith("- **Package:** ")]
    T.check(
        "hostile package name: shown only inside code spans",
        len(rows) == 1
        and "@" not in outside
        and "http" not in outside
        and rows[0].startswith("- **Package:** `evil'pkg @someone https://example.invalid/x` in "),
        repr(rows),
    )

    proc, record = d.run("judged screened package", plan="screened")
    expect_decision("judged screened package", (proc, record), "auto", "auto", "usable", "usable")
    rows = [line for line in proc.stdout.splitlines() if line.startswith("- **Package:** ")]
    T.check(
        "judged screened package: the auto-approval comment has its Package row",
        len(rows) == 1 and rows[0].startswith("- **Package:** `good-pkg` in "),
        repr(proc.stdout),
    )

    ctx = d.prepare("judged", "packages")
    other = d.sb.write_plan("decision other screen", screen_record([screened_package("good-pkg")]))
    problem = ["the floor verdict's screen does not match --screen"]
    for label, verdict, screen in (
        ("made without --screen, decided with it", d.floor_verdict(ctx, screen=None), "ctx"),
        ("made with --screen, decided without it", "floor", None),
        ("made with --screen, decided with another record", "floor", other),
    ):
        proc, record = d.run(f"judged verdict {label}", plan="packages", verdict=verdict, screen=screen)
        expect_decision(f"judged verdict {label}", (proc, record), "human", "human", "unusable", "usable")
        T.check(
            f"judged verdict {label}: the screen binding named",
            floor_problems(record) == problem,
            str(floor_problems(record)),
        )


# --- object store ---------------------------------------------------------

STORE_PREFIX = "msw-floor-objects-"
GRAPH_PARENT_NONE = (0x70000000).to_bytes(4, "big")


def cut_parents_in_graph(git_dir: Path, commit: str) -> None:
    """Patch the commit-graph's CDAT entry for commit so that it reads as having no parents."""
    path = git_dir / "objects" / "info" / "commit-graph"
    data = bytearray(path.read_bytes())
    if data[:4] != b"CGPH":
        raise RuntimeError("selftest bug: not a commit-graph file")
    hash_len = 20 if data[5] == 1 else 32
    chunks = {}
    for i in range(data[6] + 1):
        at = 8 + 12 * i
        chunks[bytes(data[at : at + 4])] = int.from_bytes(data[at + 4 : at + 12], "big")
    count = int.from_bytes(data[chunks[b"OIDF"] + 255 * 4 : chunks[b"OIDF"] + 256 * 4], "big")
    oid = bytes.fromhex(commit)
    oids = chunks[b"OIDL"]
    index = next((i for i in range(count) if data[oids + i * hash_len : oids + (i + 1) * hash_len] == oid), None)
    if index is None:
        raise RuntimeError(f"selftest bug: {commit} is not in the commit-graph")
    entry = chunks[b"CDAT"] + index * (hash_len + 16) + hash_len
    data[entry : entry + 8] = GRAPH_PARENT_NONE * 2
    path.chmod(0o644)
    path.write_bytes(bytes(data))


def loose_path(git_dir: Path, oid: str) -> Path:
    return git_dir / "objects" / oid[:2] / oid[2:]


def forge_loose(git_dir: Path, oid: str, content: bytes) -> bytes:
    """Overwrite the loose object oid with a blob holding content; return the original bytes."""
    path = loose_path(git_dir, oid)
    original = path.read_bytes()
    path.chmod(0o644)
    path.write_bytes(zlib.compress(b"blob %d\0" % len(content) + content))
    return original


def stores_left(sb: Sandbox) -> list:
    return sorted(p.name for p in sb.store_tmp.iterdir() if p.name.startswith(STORE_PREFIX))


def check_object_store(sb: Sandbox, commits: dict) -> None:
    """The floors read a fresh, re-hashed object store: tampering with the job repository changes nothing."""
    git_dir = sb.repo / ".git"
    plain_env = sb.env  # no replace or graft settings: plain git sees the tampering

    def plain(*args: str, env: dict = None):
        return subprocess.run(["git", *args], cwd=sb.repo, env=env or plain_env, capture_output=True)

    def write(name: str, content: str) -> str:
        return sb.write_plan(f"store {name}", content)

    # A: the config at the base, read at `real`; a looser config is at `dropped`
    config_args = [
        "--plan",
        write("config plan", plan_text(footprint=base(files=[".mintswarm/dogfood/github/push.sh"]))),
        "--component",
        "agents",
        "--base",
        commits["real"],
        "--issue-document",
        write("config issue", issue_document(component="agents")),
        "--analysis",
        write("config analysis", analysis_document()),
    ]
    # B: the history, read at `dropped`, whose parent is the only `(cli)` commit
    history_args = [
        "--plan",
        write("history plan", plan_text(footprint=base(files=["crates/cli/src/main.rs"]))),
        "--component",
        "cli",
        "--base",
        commits["dropped"],
        "--issue-document",
        write("history issue", issue_document(component="cli")),
        "--analysis",
        write("history analysis", analysis_document()),
    ]
    expected = {}
    for key, args in (("config", config_args), ("history", history_args)):
        proc, verdict = sb.floor(args)
        expected[key] = (proc.returncode, verdict)
    T.check(
        "object store: the untampered config case is a require-human-paths hit",
        results_of(expected["config"][1]).get("require-human-paths") == "hit",
        str(expected["config"]),
    )
    T.check(
        "object store: the untampered history case is first-of-kind clean",
        results_of(expected["history"][1]).get("first-of-kind") == "clean",
        str(expected["history"]),
    )
    T.check("object store: no store is left after clean runs", not stores_left(sb), str(stores_left(sb)))

    # every read goes through --git-dir at a store in TMPDIR: a git on PATH that logs its arguments
    bindir = sb.tmp / "git-logging"
    bindir.mkdir()
    log = sb.tmp / "git-arguments.log"
    wrapper = bindir / "git"
    wrapper.write_text(
        f'#!/bin/sh\nprintf "%s\\n" "$*" >> {shlex.quote(str(log))}\nexec {shlex.quote(shutil.which("git"))} "$@"\n',
        encoding="utf-8",
    )
    wrapper.chmod(0o755)
    path = os.pathsep.join([str(bindir), sb.env.get("PATH", "")])
    proc, verdict = sb.floor(config_args, env=dict(sb.env, PATH=path))
    calls = log.read_text(encoding="utf-8").splitlines() if log.exists() else []
    in_store = [c for c in calls if c.startswith(f"--git-dir={sb.store_tmp}/{STORE_PREFIX}")]
    elsewhere = [c for c in calls if c not in in_store]
    T.check(
        "object store: the store is made in TMPDIR, and the base, config and history are read through it",
        (proc.returncode, verdict) == expected["config"]
        and all(any(f" {word} " in c for c in in_store) for word in ("fetch", "rev-parse", "show", "log"))
        and all(c.split(" ")[0] in ("config", "rev-parse", "init") for c in elsewhere)
        and all("--git-common-dir" in c or "^{object}" in c for c in elsewhere if c.startswith("rev-parse")),
        f"exit {proc.returncode}; calls {calls}",
    )

    dropped = commits["dropped"]
    looser = plain("show", f"{dropped}:{CONFIG_PATH}").stdout
    graft_file = sb.tmp / "store-grafts"
    graft_file.write_text(f"{dropped}\n", encoding="ascii")
    grafts = git_dir / "info" / "grafts"
    orphan = sb.git("commit-tree", f"{dropped}^{{tree}}", "-m", "dropped without parents")

    def fooled_config(env: dict) -> bool:
        return plain("show", f"{commits['real']}:{CONFIG_PATH}", env=env).stdout == looser

    def fooled_history(env: dict) -> bool:
        proc = plain("log", "--format=%s", dropped, env=env)
        return proc.returncode == 0 and "feat(cli)" not in proc.stdout.decode()

    def replace_base():
        sb.git("update-ref", f"refs/replace/{commits['real']}", dropped)
        return lambda: sb.git("update-ref", "-d", f"refs/replace/{commits['real']}")

    def info_grafts():
        grafts.parent.mkdir(parents=True, exist_ok=True)
        grafts.write_text(f"{dropped}\n", encoding="ascii")
        return grafts.unlink

    def replace_graft():
        sb.git("replace", "--graft", dropped)
        return lambda: sb.git("update-ref", "-d", f"refs/replace/{dropped}")

    def ref_base():
        sb.git("update-ref", f"refs/elsewhere/{dropped}", orphan)
        return lambda: sb.git("update-ref", "-d", f"refs/elsewhere/{dropped}")

    def commit_graph():
        sb.git("update-ref", "refs/selftest/graph", dropped)
        sb.git("commit-graph", "write", "--reachable", "--no-progress")
        cut_parents_in_graph(git_dir, dropped)

        def undo():
            sb.git("update-ref", "-d", "refs/selftest/graph")
            (git_dir / "objects" / "info" / "commit-graph").unlink()

        return undo

    cases = [
        ("a replace ref on the base", "config", replace_base, {}, fooled_config),
        ("info/grafts", "history", info_grafts, {}, fooled_history),
        ("GIT_GRAFT_FILE", "history", lambda: (lambda: None), {"GIT_GRAFT_FILE": str(graft_file)}, fooled_history),
        ("git replace --graft", "history", replace_graft, {}, fooled_history),
        ("GIT_REPLACE_REF_BASE", "history", ref_base, {"GIT_REPLACE_REF_BASE": "refs/elsewhere/"}, fooled_history),
        ("a patched commit-graph", "history", commit_graph, {}, fooled_history),
    ]
    for name, key, tamper, extra, fooled in cases:
        undo = tamper()
        env = dict(sb.env, **extra)
        try:
            T.check(f"object store, {name}: plain git is fooled", fooled(dict(plain_env, **extra)))
            proc, verdict = sb.floor(config_args if key == "config" else history_args, env=env)
            T.check(
                f"object store, {name}: the verdict is the untampered one",
                (proc.returncode, verdict) == expected[key],
                f"exit {proc.returncode}: {proc.stderr.strip()} {proc.stdout.strip()}",
            )
        finally:
            undo()

    decoy = sb.tmp / "decoy"
    subprocess.run(["git", "init", "-q", str(decoy)], env=plain_env, check=True, capture_output=True)
    decoy_env = {"GIT_DIR": str(decoy / ".git"), "GIT_OBJECT_DIRECTORY": str(decoy / ".git" / "objects")}
    seen = plain("rev-parse", "--verify", "--quiet", f"{commits['real']}^{{commit}}", env=dict(plain_env, **decoy_env))
    T.check("object store, GIT_DIR at a decoy: plain git does not see the base", seen.returncode != 0)
    proc, verdict = sb.floor(config_args, env=dict(sb.env, **decoy_env))
    T.check(
        "object store, GIT_DIR and GIT_OBJECT_DIRECTORY at a decoy: the verdict is the untampered one",
        (proc.returncode, verdict) == expected["config"],
        f"exit {proc.returncode}: {proc.stderr.strip()}",
    )

    # a forged config blob, reachable from a base of its own
    marked = yq_json(CONFIG)
    marked["selftest_forge_target"] = True
    target = sb.commit_config(as_json(marked), "forge target")
    blob = sb.git("rev-parse", f"{target}:{CONFIG_PATH}")
    forged_args = list(config_args)
    forged_args[forged_args.index(commits["real"])] = target
    proc, _ = sb.floor(forged_args)
    T.check("object store, forged object: the target base is usable before the forgery", proc.returncode in (0, 1))
    original = forge_loose(git_dir, blob, looser)
    try:
        T.check(
            "object store, forged object: plain git shows the forged content",
            plain("show", f"{target}:{CONFIG_PATH}").stdout == looser,
        )
        proc, verdict = sb.floor(forged_args)
        lines = proc.stderr.strip().splitlines()
        T.check(
            "object store, forged object: floor.py exits 2 naming the fetch, no verdict",
            proc.returncode == 2 and verdict is None and len(lines) == 1 and "fetch" in lines[0],
            f"exit {proc.returncode}, verdict {verdict!r}: {proc.stderr.strip()}",
        )
        T.check("object store: no store is left after an exit 2 run", not stores_left(sb), str(stores_left(sb)))
        record = sb.repo / RECORD
        record.write_text('{"stale": true}\n', encoding="utf-8")
        assessment = sb.write_plan("store assessment", assessment_text())
        decision_args = [
            "--issue",
            "87",
            "--component",
            "agents",
            "--plan",
            forged_args[1],
            "--issue-document",
            forged_args[7],
            "--analysis",
            forged_args[9],
            "--assessment",
            assessment,
            "--assessment-artifact",
            ARTIFACT,
            "--base",
            target,
            "--harness",
            "claude",
            "--model",
            "sonnet",
        ]
        proc = sb.exec(decision_args, script=DECISION)
        T.check(
            "object store, forged object: decision.py exits 2 naming the fetch, no record",
            proc.returncode == 2 and not record.exists() and "fetch" in proc.stderr and proc.stdout == "",
            f"exit {proc.returncode}, record {'left' if record.exists() else 'gone'}: {proc.stderr.strip()}",
        )
    finally:
        path = loose_path(git_dir, blob)
        path.write_bytes(original)
        path.chmod(0o444)
    T.check("object store, forged object: restored", plain("cat-file", "blob", blob).stdout == as_json(marked))

    # a .git/shallow cutting the history below the `(cli)` commit: upload-pack sends the cut history, and the
    # store is not marked shallow
    cli_commit = sb.git("rev-parse", f"{dropped}^")
    shallow = git_dir / "shallow"
    shallow.write_text(f"{cli_commit}\n", encoding="ascii")
    try:
        T.check(
            "object store, forged .git/shallow: plain git sees a shallow repository",
            plain("rev-parse", "--is-shallow-repository").stdout.decode().strip() == "true",
        )
        proc, verdict = sb.floor(history_args)
        lines = proc.stderr.strip().splitlines()
        T.check(
            "object store, forged .git/shallow: floor.py exits 2 naming the fetch, no verdict",
            proc.returncode == 2 and verdict is None and len(lines) == 1 and "the fetch" in lines[0],
            f"exit {proc.returncode}, verdict {verdict!r}: {proc.stderr.strip()}",
        )
        T.check(
            "object store, forged .git/shallow: the fetch error carries no git hint",
            "hint:" not in proc.stderr,
            proc.stderr.strip(),
        )
    finally:
        shallow.unlink()
    T.check("object store: no store is left after a shallow run", not stores_left(sb), str(stores_left(sb)))

    # a job repository whose path holds a percent sign: the file:// URL is percent-encoded, so a decoy at the
    # path git would decode it to is never read
    percent = sb.tmp / "pct%41x"
    shutil.copytree(sb.repo, percent, symlinks=True)
    subprocess.run(["git", "init", "-q", str(sb.tmp / "pctAx")], env=plain_env, check=True, capture_output=True)
    stale = percent / VERDICT
    stale.write_text('{"stale": true}\n', encoding="utf-8")
    proc = subprocess.run(
        [sys.executable, str(FLOOR), *config_args], cwd=percent, env=sb.env, capture_output=True, text=True
    )
    verdict = json.loads(stale.read_text(encoding="utf-8")) if stale.exists() else None
    T.check(
        "object store, a '%' in the repository path: the verdict is the untampered one",
        (proc.returncode, verdict) == expected["config"],
        f"exit {proc.returncode}: {proc.stderr.strip()}",
    )
    shutil.rmtree(percent)
    shutil.rmtree(sb.tmp / "pctAx")

    sb.git("config", "extensions.partialClone", "origin")
    try:
        proc, verdict = sb.floor(config_args)
        T.check(
            "object store, partial clone: exit 2, no verdict",
            proc.returncode == 2 and verdict is None and "partial clone" in proc.stderr,
            f"exit {proc.returncode}: {proc.stderr.strip()}",
        )
    finally:
        sb.git("config", "--unset", "extensions.partialClone")
    T.check("object store: no store is left after every run", not stores_left(sb), str(stores_left(sb)))


def load_module(name: str, path: Path):
    sys.dont_write_bytecode = True  # leave no __pycache__ next to the script
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def check_authority_triggers() -> None:
    decision = load_module("msw_approval_decision", DECISION)
    T.check(
        "authority triggers: decision.py's, in order",
        tuple(AUTHORITY_TRIGGERS) == decision.AUTHORITY_TRIGGERS and tuple(HARD_TRIGGERS) == decision.HARD_TRIGGERS,
        f"{decision.AUTHORITY_TRIGGERS}, {decision.HARD_TRIGGERS}",
    )
    T.check(
        "authority triggers: a subset of the hard triggers",
        set(AUTHORITY_TRIGGERS) <= set(HARD_TRIGGERS) and len(set(AUTHORITY_TRIGGERS)) == len(AUTHORITY_TRIGGERS),
        repr(AUTHORITY_TRIGGERS),
    )


def check_span() -> None:
    decision = load_module("msw_approval_decision_span", DECISION)
    span = decision.span
    T.check(
        "span: the limit and the mark are decision.py's",
        decision.SPAN_LIMIT == SPAN_LIMIT == 600 and decision.SPAN_MARK == SPAN_MARK,
        f"{decision.SPAN_LIMIT!r}, {decision.SPAN_MARK!r}",
    )
    under = "a" * (SPAN_LIMIT - 1)
    at = "b" * SPAN_LIMIT
    T.check("span: text under the limit is unchanged", span(under) == f"`{under}`", span(under))
    T.check("span: text exactly at the limit is unchanged", span(at) == f"`{at}`", span(at))
    T.check(
        "span: text at the limit padded with whitespace is unchanged and not marked",
        span(f"  \n{at}\n  ") == f"`{at}`",
        span(f"  \n{at}\n  "),
    )
    over = "c" * SPAN_LIMIT + "d"
    T.check(
        "span: text over the limit is its first 600 characters and the mark in one code span",
        span(over) == f"`{'c' * SPAN_LIMIT}{SPAN_MARK}`",
        span(over),
    )
    spaced = "e" * (SPAN_LIMIT - 1) + " fff"
    T.check(
        "span: a cut just after a space leaves no space before the mark",
        span(spaced) == f"`{'e' * (SPAN_LIMIT - 1)}{SPAN_MARK}`",
        span(spaced),
    )
    hostile = ("`a`\r\nb\u2028c\u200bd\u202e " * 100) + "tail"
    out = span(hostile)
    T.check(
        "span: hostile text over the limit is one line, one code span, inert and marked",
        "\n" not in out
        and "\r" not in out
        and out.count("`") == 2
        and out.startswith("`")
        and out.endswith(SPAN_MARK + "`")
        and len(out) <= SPAN_LIMIT + len(SPAN_MARK) + 2
        and not any(unicodedata.category(c) in ("Cc", "Cf", "Zl", "Zp") for c in out),
        repr(out),
    )
    T.check("span: empty text", span(" \n ") == "`(empty)`", span(" \n "))


def check_globs() -> None:
    floor = load_module("msw_floor", FLOOR)
    paths = [
        "docs",
        "docs/a.md",
        "docs/x/y.md",
        "x.md",
        "a/x.md",
        "a/b/x.md",
        "a/b",
        "a/c/d/b",
        "a/",
        "a.md",
        "ab.md",
        "abc",
        "a/c",
        "aXb",
        "a.b",
        "[ab].md",
        "a+b(c)",
        "README.md",
        "readme.md",
        ".mintswarm/dogfood/github/push.sh",
        ".mintswarm/dogfood/githubx",
        "crates/core/src/lib.rs",
        "crates/core/x/src/lib.rs",
    ]
    # Each pattern with exactly the paths above it matches; it matches none of the others.
    matches = {
        "**": paths,
        "docs/**": ["docs/a.md", "docs/x/y.md"],
        "**/x.md": ["x.md", "a/x.md", "a/b/x.md"],
        "a/**/b": ["a/b", "a/c/d/b"],
        "a/**": ["a/x.md", "a/b/x.md", "a/b", "a/c/d/b", "a/", "a/c"],
        "*.md": ["x.md", "a.md", "ab.md", "[ab].md", "README.md", "readme.md"],
        "docs/*.md": ["docs/a.md"],
        "?.md": ["x.md", "a.md"],
        "a?c": ["abc"],
        "a.b": ["a.b"],
        "[ab].md": ["[ab].md"],
        "a+b(c)": ["a+b(c)"],
        "README.md": ["README.md"],
        ".mintswarm/dogfood/github/**": [".mintswarm/dogfood/github/push.sh"],
        "crates/*/src/**": ["crates/core/src/lib.rs"],
    }
    differ = [
        (p, s) for p, want in matches.items() for s in paths if bool(floor.glob_to_regex(p).fullmatch(s)) != (s in want)
    ]
    T.check(
        f"glob semantics: floor.py matches as pinned on {len(matches) * len(paths)} pairs",
        not differ,
        str(differ[:5]),
    )
    pinned = [
        ("**/x.md", "x.md", True),
        ("**/x.md", "a/b/x.md", True),
        ("*.md", "docs/a.md", False),
        ("?.md", "a.md", True),
        ("a?c", "a/c", False),
        ("a.b", "aXb", False),
        ("[ab].md", "a.md", False),
        ("[ab].md", "[ab].md", True),
        ("docs/**", "docs", False),
    ]
    wrong = [(p, s) for p, s, want in pinned if bool(floor.glob_to_regex(p).fullmatch(s)) != want]
    T.check("glob semantics: pinned cases", not wrong, str(wrong))


def check_stdlib() -> None:
    for path in (FLOOR, DECISION, SIZE, Path(__file__).resolve()):
        tree = ast.parse(path.read_text(encoding="utf-8"))
        names = set()
        for node in ast.walk(tree):
            if isinstance(node, ast.Import):
                names.update(alias.name.split(".")[0] for alias in node.names)
            elif isinstance(node, ast.ImportFrom):
                names.add("." if node.level else node.module.split(".")[0])
        outside = sorted(n for n in names if n not in sys.stdlib_module_names)
        T.check(
            f"standard library: {path.name} imports only {', '.join(sorted(names))}",
            not outside,
            f"not standard library: {outside}",
        )


# --- sizes ----------------------------------------------------------------


SIZE_SECTION = "## Size\n\nsize: S\n- the change is one small script\n"
SIZE_ANALYSIS_SPLIT = (
    "## Split\n\n```json\n"
    '{"split": {"proposed": false, "reason": "none", "components": ["docs"], "finding": ""}}\n'
    "```\n\n"
)


def size_plan(section: str = SIZE_SECTION, footprint: dict = None) -> str:
    """A plan whose `## Size` section, when given, comes right after the footprint fence."""
    footprint = eight_keys(300, estimated_lines=400) if footprint is None else footprint
    return plan_text(footprint=footprint, after=f"\n{section}" if section else "")


def size_analysis(section: str = "## Size\n\nsize: M\n- one reason\n", before: str = SIZE_ANALYSIS_SPLIT) -> str:
    return (
        "# Analysis: selftest case\n\nGenerated by selftest.py.\n\n"
        f"## What is asked\n\nA generated case.\n\n{before}{section}\n## Approaches\n\n## Questions\n\n"
    )


def check_size_bands() -> None:
    size = load_module("msw_size", SIZE)
    pinned = [
        (0, "XS"),
        (25, "XS"),
        (26, "S"),
        (75, "S"),
        (76, "M"),
        (200, "M"),
        (201, "L"),
        (500, "L"),
        (501, "XL"),
        (1200, "XL"),
        (1201, "2XL"),
        (3000, "2XL"),
        (3001, "3XL"),
        (7000, "3XL"),
        (7001, "4XL"),
    ]
    wrong = [(lines, want, size.floor_size(lines)) for lines, want in pinned if size.floor_size(lines) != want]
    T.check(f"size bands: {len(pinned)} boundaries of design §5", not wrong, str(wrong))
    T.check(
        "size bands: the eight sizes in order",
        list(size.SIZES) == ["XS", "S", "M", "L", "XL", "2XL", "3XL", "4XL"],
        str(size.SIZES),
    )


def run_size(sb: Sandbox, name: str, args: list, content=None):
    """size.py run as `sys.executable -I`, on the content written under cases/ when given."""
    if content is not None:
        args = [*args, sb.write_plan(f"size {name}", content)]
    return subprocess.run(
        [sys.executable, "-I", str(SIZE), *args], cwd=sb.repo, env=sb.env, capture_output=True, text=True
    )


def expect_size(sb: Sandbox, name: str, mode: str, content, code: int, out: list = None) -> None:
    proc = run_size(sb, name, [mode], content)
    lines = proc.stdout.splitlines()
    T.check(
        f"size {mode}: {name}: exit {code}" + (f", prints {out}" if out is not None else ""),
        proc.returncode == code and (out is None or lines == out) and (code != 1 or len(lines) == 1),
        f"exit {proc.returncode}, stdout {lines}, stderr {proc.stderr.strip()!r}",
    )


def malformed_sizes() -> list:
    """(name, `## Size` section, or None for none) of sections that are not usable."""
    good = "size: L\n- a reason\n"
    return [
        ("no section", None),
        ("two sections", f"## Size\n\n{good}\n## Size\n\n{good}"),
        ("only inside a fence", f"## Notes\n\n~~~\n## Size\n\n{good}~~~\n"),
        ("wrong position", f"## Notes\n\nA note.\n\n## Size\n\n{good}"),
        ("lower-case value", "## Size\n\nsize: l\n- a reason\n"),
        ("capitalised key", "## Size\n\nSize: L\n- a reason\n"),
        ("unknown value", "## Size\n\nsize: XXL\n- a reason\n"),
        ("trailing text", "## Size\n\nsize: L (large)\n- a reason\n"),
        ("leading space", "## Size\n\n size: L\n- a reason\n"),
        ("no reasons", "## Size\n\nsize: L\n"),
        ("four reasons", "## Size\n\nsize: L\n- one\n- two\n- three\n- four\n"),
        ("empty bullet", "## Size\n\nsize: L\n- \n"),
        ("prose line", "## Size\n\nsize: L\n- a reason\nSome prose.\n"),
        ("continuation line", "## Size\n\nsize: L\n- a reason\n  continued\n"),
        ("nested bullet", "## Size\n\nsize: L\n- a reason\n  - nested\n"),
        ("fence", "## Size\n\nsize: L\n- a reason\n```text\nL\n```\n"),
    ]


def valid_sizes() -> list:
    """(name, `## Size` section, the size it states) of usable sections."""
    return [
        ("blank lines between reasons", "## Size\n\nsize: XL\n\n- one\n\n- two\n", "XL"),
        ("one reason", "## Size\n\nsize: XL\n- one\n", "XL"),
        ("three reasons", "## Size\n\nsize: XL\n- one\n- two\n- three\n", "XL"),
        ("ended by the next heading", "## Size\n\nsize: 2XL\n- one", "2XL"),
        ("trailing spaces and tabs", "## Size \n\nsize: XL \t\n- one\t\n- two \n", "XL"),
    ]


def check_size_cli(sb: Sandbox) -> None:
    raised = ["size: L", "size raised from S to L by planned lines"]
    no_size = ["size: L", "the plan states no usable size; the size shown is the lines floor"]
    expect_size(sb, "judged S, 300 production lines", "plan", size_plan(), 0, raised)
    expect_size(
        sb, "judged XL, 300 production lines", "plan", size_plan("## Size\n\nsize: XL\n- big\n"), 0, ["size: XL"]
    )
    expect_size(sb, "judged L, 300 production lines", "plan", size_plan("## Size\n\nsize: L\n- one\n"), 0, ["size: L"])
    expect_size(sb, "seven-key footprint", "plan", size_plan(footprint=base(estimated_lines=300)), 0, raised)
    by_production = size_plan(footprint=eight_keys(50, estimated_lines=400))
    expect_size(sb, "measured by production_lines", "plan", by_production, 0, ["size: S"])
    expect_size(sb, "CRLF", "plan", size_plan().replace("\n", "\r\n"), 0, raised)
    for name, section in malformed_sizes():
        expect_size(sb, name, "plan", size_plan(section), 0, no_size)
        expect_size(sb, name, "analysis", size_analysis(section or ""), 1)
    for name, section, value in valid_sizes():
        expect_size(sb, name, "plan", size_plan(section), 0, [f"size: {value}"])
        expect_size(sb, name, "analysis", size_analysis(section), 0, [f"size: {value}"])
    expect_size(sb, "valid analysis", "analysis", size_analysis(), 0, ["size: M"])
    expect_size(sb, "analysis with CRLF", "analysis", size_analysis().replace("\n", "\r\n"), 0, ["size: M"])
    expect_size(sb, "a plan is not an analysis", "analysis", size_plan(), 1)
    json_fence = '## Size\n\nsize: L\n- a reason\n```json\n{"size": "L"}\n```\n'
    expect_size(sb, "json fence", "analysis", size_analysis(json_fence), 1)
    unusable = [
        ("no arguments", []),
        ("mode only", ["plan"]),
        ("three arguments", ["plan", "a.md", "b.md"]),
        ("unknown mode", ["design", sb.write_plan("size unknown mode", size_plan())]),
        ("missing file", ["plan", "cases/no-such-plan.md"]),
        ("not UTF-8", ["plan", sb.write_plan("size not utf-8", b"\xff\xfe## Size\n")]),
        ("analysis not UTF-8", ["analysis", sb.write_plan("size analysis not utf-8", b"\xff\xfe## Size\n")]),
        ("unusable footprint", ["plan", sb.write_plan("size no footprint", size_analysis())]),
        ("json fence in a plan's Size", ["plan", sb.write_plan("size json fence", size_plan(json_fence))]),
    ]
    for name, args in unusable:
        proc = run_size(sb, name, args)
        T.check(
            f"size: {name}: exit 2, nothing on stdout, one line on stderr",
            proc.returncode == 2 and proc.stdout == "" and len(proc.stderr.splitlines()) == 1,
            f"exit {proc.returncode}, stdout {proc.stdout!r}, stderr {proc.stderr!r}",
        )


def check_size_floor(r: Runner) -> None:
    proc, verdict = r.run("plan with a Size section", plan_text(after=f"\n{SIZE_SECTION}"))
    T.check(
        "plan with a Size section: exit 0, every rule clean",
        proc.returncode == 0
        and set(results_of(verdict).values()) == {"clean"}
        and len(results_of(verdict)) == len(RULES),
        proc.stdout,
    )


# Config variants whose pipeline.plan_approval.min_confidence is unusable input.
MIN_CONFIDENCE_INVALID = {"min-89": 89, "min-101": 101, "min-true": True, "min-string": "95"}


# Config variants whose limits_by_component or non_production is unusable input.
SIZE_INVALID = {
    "lbc-undefined": {"limits_by_component": {"nowhere": {"lines": 1}}},
    "lbc-unknown-key": {"limits_by_component": {"docs": {"size": 1}}},
    "lbc-negative": {"limits_by_component": {"docs": {"lines": -1}}},
    "lbc-true": {"limits_by_component": {"docs": {"lines": True}}},
    "lbc-not-mapping": {"limits_by_component": {"docs": 5}},
    "lbc-list": {"limits_by_component": []},
    "non-production-string": {"non_production": "**/tests/**"},
    "non-production-numbers": {"non_production": [1]},
}


# Config variants that still give the removed new_dependencies limit: unusable input.
REMOVED_LIMIT = {
    "limit-new-dependencies": lambda approval: approval["limits"].update(new_dependencies=0),
    "lbc-new-dependencies": lambda approval: approval.update(limits_by_component={"docs": {"new_dependencies": 0}}),
}


def build_commits(sb: Sandbox) -> tuple:
    real_bytes = CONFIG.read_bytes()
    real = yq_json(CONFIG)
    commits = {"noconfig": sb.commit_empty("no config")}
    commits["nodocs"] = sb.commit_config(real_bytes, "chore(agents): config before any docs commit")
    for subject in SEEDS:
        sb.commit_empty(subject)

    def variant(name: str, change) -> None:
        config = copy.deepcopy(real)
        change(config)
        commits[name] = sb.commit_config(as_json(config), name)

    approval = lambda c: c["pipeline"]["plan_approval"]  # noqa: E731
    variant("no-plan-approval", lambda c: c["pipeline"].pop("plan_approval"))
    variant("unknown-key", lambda c: approval(c).update(extra=True))
    variant("mode-sometimes", lambda c: approval(c).update(mode="sometimes"))
    variant("limit-negative", lambda c: approval(c)["limits"].update(files=-1))
    variant("limit-true", lambda c: approval(c)["limits"].update(lines=True))
    variant("undefined-component", lambda c: approval(c)["require_human"]["components"].append("nowhere"))
    variant("missing-limit", lambda c: approval(c)["limits"].pop("new_files"))
    variant("always", lambda c: approval(c).update(mode="always"))
    variant("judged", lambda c: approval(c).update(mode="judged"))
    variant("never", lambda c: approval(c).update(mode="never"))
    variant("judged-95", lambda c: approval(c).update(mode="judged", min_confidence=95))
    variant("min-100", lambda c: approval(c).update(mode="judged", min_confidence=100))
    for name, value in MIN_CONFIDENCE_INVALID.items():
        variant(name, lambda c, value=value: approval(c).update(min_confidence=value))
    for name, change in SIZE_INVALID.items():
        variant(name, lambda c, change=change: approval(c).update(change))
    for name, change in REMOVED_LIMIT.items():
        variant(name, lambda c, change=change: change(approval(c)))
    variant("component-limits", lambda c: approval(c).update(limits_by_component={"docs": {"lines": 400}}))
    variant("tested", lambda c: c["gates"].update(tests="cargo test"))
    variant("untested", lambda c: c["gates"].update(tests="none"))
    variant(
        "cd-gates",
        lambda c: c["gates"]["run"].extend(
            [
                {"name": "web-script", "command": "cd ./web && bash x.sh"},
                {"name": "nested-script", "command": "cd ./web && cd ./apps && bash y.sh"},
            ]
        ),
    )
    commits["real"] = sb.commit_config(real_bytes, "real")
    sb.git("tag", "base")
    sb.commit_empty("feat(cli): seen after base")
    variant("dropped", lambda c: approval(c)["require_human"]["paths"].remove(".mintswarm/dogfood/github/**"))
    return commits, approval(real)["limits"]


def main() -> int:
    for tool in ("git", "yq"):
        if shutil.which(tool) is None:
            print(f"selftest.py: {tool} is not on PATH", file=sys.stderr)
            return 2
    check_config()
    with tempfile.TemporaryDirectory(prefix="msw-floor-") as tmp:
        sb = Sandbox(Path(tmp))
        commits, limits = build_commits(sb)
        runner = Runner(sb, commits)
        check_clean_base(runner)
        check_single_rules(runner, limits)
        check_production_lines(runner, limits)
        check_dependency_screen(runner, sb, commits)
        check_component_limits(runner, limits)
        check_unusable_footprints(runner)
        check_base_ref(runner, sb)
        check_historic(runner)
        check_blocking_questions(runner)
        check_acceptance_criteria(runner)
        check_first_of_kind(runner)
        check_unusable_input(sb, commits)
        check_verdict_errors(sb, commits)
        check_decision(sb, commits)
        check_object_store(sb, commits)
        check_size_floor(runner)
        check_size_cli(sb)
    check_globs()
    check_authority_triggers()
    check_span()
    check_size_bands()
    check_stdlib()
    print(f"{T.passed} passed, {T.failed} failed")
    return 0 if T.failed == 0 else 1


if __name__ == "__main__":
    sys.exit(main())
