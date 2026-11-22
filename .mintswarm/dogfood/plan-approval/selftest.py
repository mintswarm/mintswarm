#!/usr/bin/env python3
"""Offline self-test of floor.py and decision.py, the two scripts of plan approval.

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
written under `cases/`. Unless a case says otherwise, floor.py gets a
generated issue document for the case's component, whose criteria the
plan's Verification table quotes and whose comments hold a decoy criteria
section, and an analysis with no questions.

Checks:

- config: the checkout's config has no pipeline.approve_plan and has the
  expected pipeline.plan_approval;
- verdict shape, on every case that exits 0 or 1: the verdict's keys in
  order, version 2, the 16 rule identifiers in order across `rules` and
  `not_evaluated`, which is empty or the footprint rules after a `footprint`
  hit, results and reasons, `verdict` against the rules and the exit code,
  `plan`, `issue_document` and `analysis` against the files given,
  `base.commit`, `mode` and the printed lines;
- clean base: a footprint modelled on #20 is clean on every rule, and is
  accepted with `mode: always`, which the verdict records;
- single-rule hits: each rule hit by a case derived from the clean base,
  and each limit clean at its value and a hit one over;
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
- blocking-questions: a blocking question in the analysis or the plan,
  with continuation lines named once, a variant marker, prose, a missing or
  repeated Questions section and no --analysis are hits; non-blocking
  entries only, and a Questions heading inside a fence, are clean;
- acceptance-criteria: a dropped, added, reworded or repeated criterion, no
  criteria, no criteria section, two Verification sections, a table without
  its `---` row and no --issue-document are hits naming the criterion or
  the cause; whitespace differences, wrapped criteria, `\\|`, other list
  markers and task boxes, reordered rows, the decoy in comments and CRLF
  are clean;
- first-of-kind: `cli` at `real`, `core` (multi-scope only), `runner`
  (revert only) and `domain` (other case only) are hits, and so are an
  issue kind other than implementation, another component, a repeated or
  missing header line and no --issue-document; `cli` at `dropped`, `api`
  (`feat(api)!:`) and `docs` are clean;
- unusable input: bad arguments, plans, documents (missing, unreadable, not
  UTF-8, given twice or with an empty value), refs and configs, and a
  missing `yq`, exit 2 and leave no verdict file, even a stale one;
- verdict errors: a verdict path that is a directory, and a read-only
  outputs directory with or without a stale verdict, exit 2 with a one-line
  error and no traceback;
- glob semantics: floor.py's glob_to_regex agrees with the one in
  .mintswarm/dogfood/github/request-review.py;
- decision record shape, on every decision run that exits 0 or 1: the
  record's keys and each layer's keys in order, the subject, the mode, the
  floor layer's rules equal to the verdict's, the risk and reason equal to
  the assessment's, harness and model equal to the arguments,
  `as_configured` against the config, `judged_decision` and `decision`
  recomputed from the layers, `as_configured` and the mode, the exit code,
  and stdout: the auto-approval comment for `auto`, the shadow-record
  comment in `shadow` mode, empty otherwise;
- decision under `judged`: clean and `low` is `auto`; a floor hit, `medium`,
  `high`, a missing or unparsable verdict, a verdict edited to break each
  binding (the documents' sha256 too), a verdict made without the documents
  or with another issue document or analysis, a forged verdict that is
  consistent in itself but differs from what floor.py gives for the plan
  and documents (a `first-of-kind` hit set clean), and a missing, non-UTF-8
  or malformed
  assessment are `human`, with the failing layer unusable; a trailing space
  and CRLF still give `auto`; a harness or model other than the config's
  sets `as_configured` false and gives `human`; a `blocking-questions` hit
  and a `footprint` hit give a usable floor layer equal to the verdict;
- decision under `always`, `shadow` and `never`: `human`, `human` with
  `judged_decision` recorded, and `auto` whatever the layers say;
- the comments: the auto-approval comment's heading, floor line, risk and
  reason and veto paragraph; the shadow-record comment's heading and rows;
  and in both, model and session text only inside code spans of at most
  200 characters;
- decision unusable input: bad arguments, plans, refs and configs, a
  missing, unreadable or non-UTF-8 issue document or analysis, a missing
  `yq` and a record path that is a directory exit 2, print nothing
  to stdout and leave no record, even a stale one;
- standard library: every import of floor.py, decision.py and selftest.py is
  in the standard library.

Prints one `ok` or `FAIL` line per check and exits 0 only when all passed.
"""

import ast
import copy
import hashlib
import importlib.util
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
FLOOR = HERE / "floor.py"
DECISION = HERE / "decision.py"
FIXTURES = HERE / "fixtures"
ROOT = HERE.parents[2]
CONFIG = ROOT / ".mintswarm" / "config.yaml"
REQUEST_REVIEW = ROOT / ".mintswarm" / "dogfood" / "github" / "request-review.py"
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
    "limit-new-dependencies",
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
    "tool", "version", "plan", "issue_document", "analysis", "component", "base",
    "config", "mode", "verdict", "rules", "not_evaluated",
]
SEEDS = [
    "docs(docs): seed", "feat(agents): seed", "chore(tooling): seed", "feat(web): seed", "feat(api)!: seed",
    "feat(core,web): seed", 'Revert "feat(runner): seed"', "feat(Domain): seed",
]
CRITERIA = [
    "The floor reads the issue document and the analysis.",
    "A reason names the criterion it is about.",
    "A cell may quote `a | b` with its bar escaped.",
]
EXPECTED_APPROVAL = {
    "mode": "shadow",
    "require_human": {
        "components": ["agents", "tooling"],
        "paths": [".mintswarm/config.yaml", ".mintswarm/dogfood/github/**", "AGENTS.md", "CLAUDE.md"],
    },
    "limits": {"files": 10, "lines": 300, "new_files": 3, "new_dependencies": 0},
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
HISTORIC = {
    "plan-9.md": ("agents", {
        "require-human-components", "limit-files", "limit-lines", "gate-changes", "tests-none",
    }),
    "plan-12.md": ("agents", {
        "require-human-paths", "require-human-components", "limit-lines", "gate-changes", "tests-none",
    }),
    "plan-14.md": ("agents", {
        "require-human-paths", "require-human-components", "limit-files", "limit-lines",
        "limit-new-files", "external-steps", "gate-changes", "tests-none",
    }),
    "plan-19.md": ("tooling", {"require-human-components", "external-steps", "tests-none"}),
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
        env.update({
            "HOME": str(home),
            "GIT_CONFIG_NOSYSTEM": "1",
            "GIT_CONFIG_GLOBAL": os.devnull,
            "GIT_AUTHOR_NAME": "floor selftest",
            "GIT_AUTHOR_EMAIL": "selftest@example.invalid",
            "GIT_COMMITTER_NAME": "floor selftest",
            "GIT_COMMITTER_EMAIL": "selftest@example.invalid",
            "GIT_AUTHOR_DATE": "2026-01-01T00:00:00Z",
            "GIT_COMMITTER_DATE": "2026-01-01T00:00:00Z",
        })
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
            cwd=self.repo, env=env or self.env, capture_output=True, text=True,
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
    return (f"\n## Verification\n\n| Acceptance criterion | Test or check |\n| --- | --- |\n{table}{extra}\n"
            f"## Questions\n\n{questions}")


def plan_text(block: str = None, footprint: dict = None, approach: str = "", between: str = "",
              section: str = "## Footprint\n\n", opener: str = "```json", closer: str = "```\n",
              after: str = "", tail: str = None) -> str:
    if block is None:
        block = footprint_block(footprint if footprint is not None else CLEAN_BASE)
    return (
        "# Plan: selftest case\n\nGenerated by selftest.py.\n\n"
        f"## Approach\n\nA generated case.\n{approach}\n"
        "## Steps\n\n1. **Change** — files: as declared.\n\n"
        f"{between}{section}{opener}\n{block}\n{closer}{after}"
        f"{plan_tail() if tail is None else tail}"
    )


def criteria_list(criteria: list = None) -> str:
    return "".join(f"{i}. {c}\n" for i, c in enumerate(CRITERIA if criteria is None else criteria, 1))


def issue_document(component: str = "docs", kind: str = "implementation", criteria: str = None,
                   header: str = None) -> str:
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
        return T.check(f"{name}: verdict shape", False, f"no verdict file (exit {proc.returncode}): {proc.stderr.strip()}")
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
    if verdict.get("tool") != "mintswarm-plan-floor" or verdict.get("version") != 2:
        problems.append("tool or version")
    for key in ("plan", "issue_document", "analysis"):
        path = args.get(key)
        bound = None if path is None else {
            "path": path, "sha256": hashlib.sha256((args["cwd"] / path).read_bytes()).hexdigest()}
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

    def run(self, name: str, content, component: str = "docs", config: str = "real",
            ref: str = None, commit: str = None, mode: str = None, issue=DEFAULT, analysis=DEFAULT):
        """issue and analysis: DEFAULT (generated, clean), None (not given), or the content to give."""
        mode = mode or EXPECTED_APPROVAL["mode"]
        plan = self.sb.write_plan(name, content)
        ref = ref or self.commits[config]
        commit = commit or self.commits[config]
        args = ["--plan", plan, "--component", component, "--base", ref]
        documents = {}
        for key, flag, value, generated in (
                ("issue_document", "--issue-document", issue, lambda: issue_document(component=component)),
                ("analysis", "--analysis", analysis, analysis_document)):
            if value is None:
                documents[key] = None
                continue
            documents[key] = self.sb.write_plan(f"{name} {key}", generated() if value is DEFAULT else value)
            args += [flag, documents[key]]
        proc, verdict = self.sb.floor(args)
        check_shape(name, proc, verdict, {"cwd": self.sb.repo, "plan": plan, "component": component, "base": ref,
                                          **documents}, commit, mode)
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


def expect_hit(r: Runner, name: str, footprint: dict, rule: str, only: bool = False,
               reason: str = None, reasons: list = None, tail: str = None, **kwargs):
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
    T.check(f"{name}: {', '.join(rules)} clean", proc.returncode in (0, 1) and not bad,
            f"not clean: {bad}: {[reasons_of(verdict, rule) for rule in bad]}")
    return verdict


# --- check groups ---------------------------------------------------------

def check_config() -> None:
    config = yq_json(CONFIG)
    pipeline = config.get("pipeline") or {}
    T.check("config: no pipeline.approve_plan", "approve_plan" not in pipeline)
    T.check("config: pipeline.plan_approval values", pipeline.get("plan_approval") == EXPECTED_APPROVAL,
            json.dumps(pipeline.get("plan_approval")))


def check_clean_base(r: Runner) -> None:
    proc, verdict = r.run("clean base", plan_text())
    T.check("clean base: exit 0, every rule clean",
            proc.returncode == 0 and set(results_of(verdict).values()) == {"clean"}
            and len(results_of(verdict)) == len(RULES), proc.stdout)
    proc, verdict = r.run("always mode", plan_text(), config="always", mode="always")
    T.check("always mode: accepted, rules evaluated, mode recorded",
            proc.returncode == 0 and len(results_of(verdict)) == len(RULES)
            and isinstance(verdict, dict) and verdict.get("mode") == "always",
            f"exit {proc.returncode}: {proc.stderr.strip()}")


def check_single_rules(r: Runner, limits: dict) -> None:
    expect_hit(r, "CLAUDE.md", base(files=["CLAUDE.md"]), "require-human-paths", only=True, reason="CLAUDE.md")
    expect_hit(r, "agents.md", base(files=["agents.md"]), "require-human-paths", reason="AGENTS.md")
    expect_hit(r, "tests/e2e.rs", base(files=["tests/e2e.rs"]), "require-human-components", reason="tooling")
    expect_hit(r, "tooling component", base(files=[".github/CODEOWNERS"]), "require-human-components",
               reason="component tooling", component="tooling")
    expect_hit(r, "web/index.html", base(files=["web/index.html"]), "component-scope")
    expect_clean(r, "Cargo.toml", base(files=["Cargo.toml"]), ["component-scope", "no-component"])
    expect_hit(r, "LICENSE.md", base(files=["LICENSE.md"]), "no-component")

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
    expect_hit(r, f"new files at {n + 1}", base(files=docs_files(n + 1), new_files=docs_files(n + 1)),
               "limit-new-files", only=True)

    def deps(count: int) -> list:
        return [{"name": f"dep-{i}", "manifest": "docs/requirements.txt"} for i in range(count)]

    n = limits["new_dependencies"]
    expect_clean(r, f"new dependencies at {n}", base(new_dependencies=deps(n)), ["limit-new-dependencies"])
    expect_hit(r, f"new dependencies at {n + 1}", base(new_dependencies=deps(n + 1)),
               "limit-new-dependencies", only=True)

    expect_hit(r, "external step", base(external_steps=["the maintainer flips a setting"]), "external-steps",
               only=True, reason="flips a setting")
    expect_hit(r, "gate_changes entry", base(gate_changes=["whitespace: new base"]), "gate-changes",
               only=True, reason="whitespace: new base")
    expect_hit(r, "render-agents.py", base(files=[".mintswarm/dogfood/render-agents.py"]), "gate-changes",
               reason="agents-drift", component="agents")
    expect_hit(r, "config.yaml", base(files=[".mintswarm/config.yaml"]), "gate-changes", reason="gates.run")
    expect_hit(r, ".GitHub/CODEOWNERS", base(files=[".GitHub/CODEOWNERS"]), "require-human-components",
               reason="component tooling")
    expect_hit(r, "Config.yaml case", base(files=[".MintSwarm/Config.yaml"]), "gate-changes", reason="gates.run")
    expect_hit(r, "Render-Agents.py case", base(files=[".mintswarm/dogfood/Render-Agents.py"]), "gate-changes",
               reason="agents-drift", component="agents")
    expect_hit(r, "tests none", base(files=["web/index.html"]), "tests-none", only=True, component="web")
    expect_clean(r, "tests cargo", base(files=["web/index.html"]), ["tests-none"], component="web", config="tested")
    expect_hit(r, "overridden assumption",
               base(overridden_assumptions=[{"assumption": "A1", "reason": "the analysis was wrong"}]),
               "overridden-assumptions", only=True, reason="A1")


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
        ("trailing comma", plan_text(block=replaced(block, '"overridden_assumptions": []',
                                                    '"overridden_assumptions": [],'))),
        ("duplicate key", plan_text(block=replaced(block, '"estimated_lines": 6,',
                                                   '"estimated_lines": 6,\n    "estimated_lines": 6,'))),
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
        ("dependency extra key", plan_text(footprint=base(
            new_dependencies=[{"name": "x", "manifest": "docs/requirements.txt", "version": "1"}]))),
        ("top level extra key", plan_text(block=json.dumps({"footprint": CLEAN_BASE, "more": 1}, indent=2))),
    ]
    for path in ["./a.md", "docs/../AGENTS.md", "/abs.md", "docs//a.md", "docs\\a.md", "docs/*.md",
                 "docs/", ".mintswarm/job/x.md", ".MintSwarm/Job/x.md"]:
        cases.append((f"path {path}", plan_text(footprint=base(files=[path]))))
    cases.append(("manifest ../x", plan_text(footprint=base(
        new_dependencies=[{"name": "x", "manifest": "../requirements.txt"}]))))
    return cases


def check_unusable_footprints(r: Runner) -> None:
    # An unclosed fence runs to the end of the plan, so Verification and Questions are inside it.
    fenced = {"no closing fence": {"blocking-questions", "acceptance-criteria"}}
    for name, text in unusable_footprints():
        proc, verdict = r.run(name, text)
        hits = fenced.get(name, set())
        expected = {"footprint": "hit", **{rule: "hit" if rule in hits else "clean" for rule in DOCUMENT_RULES}}
        ok = (proc.returncode == 1 and results_of(verdict) == expected
              and verdict.get("not_evaluated") == FOOTPRINT_RULES)
        label = ", ".join(sorted(hits)) + " hit" if hits else "clean"
        T.check(f"{name}: footprint hit, footprint rules not evaluated, document rules {label}", ok,
                f"exit {proc.returncode}: {proc.stdout.strip()} {proc.stderr.strip()}")


def check_base_ref(r: Runner, sb: Sandbox) -> None:
    worktree = json.loads((sb.repo / CONFIG_PATH).read_text(encoding="utf-8"))
    T.check("base ref: the worktree config drops the github path",
            ".mintswarm/dogfood/github/**" not in worktree["pipeline"]["plan_approval"]["require_human"]["paths"])
    footprint = base(files=[".mintswarm/dogfood/github/push.sh"])
    proc, verdict = r.run("base ref: --base base", plan_text(footprint=footprint), component="agents",
                          ref="base", commit=r.commits["real"])
    T.check("base ref: --base base gives require-human-paths a hit",
            results_of(verdict).get("require-human-paths") == "hit"
            and any(".mintswarm/dogfood/github/**" in x for x in reasons_of(verdict, "require-human-paths")),
            proc.stdout)
    proc, verdict = r.run("base ref: --base HEAD", plan_text(footprint=footprint), component="agents",
                          ref="HEAD", commit=r.commits["dropped"])
    T.check("base ref: --base HEAD gives require-human-paths clean",
            results_of(verdict).get("require-human-paths") == "clean", proc.stdout)


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
    cases += [("plan-20.md", "docs", {"first-of-kind"}, "nodocs"),
              ("plan-10.md", "docs", {"require-human-paths", "first-of-kind"}, "nodocs")]
    for fixture, component, hits, config in cases:
        name = f"fixture {fixture}" + (f" at {config}" if config != "real" else "")
        proc, verdict = r.run(name, historic(fixture), component=component, config=config)
        expected = {rule: ("hit" if rule in hits else "clean") for rule in RULES}
        got = results_of(verdict)
        diff = {k: got.get(k) for k in RULES if got.get(k) != expected[k]}
        T.check(f"{name}: the {len(RULES)} expected results", got == expected and
                proc.returncode == (1 if hits else 0), f"exit {proc.returncode}, differs on {diff}")
        if fixture in fixture_reasons:
            rule, text = fixture_reasons[fixture]
            T.check(f"{name}: {rule} names {text!r}",
                    any(text in x for x in reasons_of(verdict, rule)), str(reasons_of(verdict, rule)))
        if "first-of-kind" in hits:
            T.check(f"{name}: first-of-kind names the component",
                    any("scoped (docs)" in x for x in reasons_of(verdict, "first-of-kind")),
                    str(reasons_of(verdict, "first-of-kind")))


def check_blocking_questions(r: Runner) -> None:
    rule = "blocking-questions"
    question = "Must the floor read   tracker\thistory?"
    assumed = "- **Non-blocking (assumed):** Is the order free? → Yes.\n"
    expect_hit(r, "blocking question in the analysis", base(), rule, only=True,
               reasons=["analysis: blocking question: Must the floor read tracker history?"],
               analysis=analysis_document(assumed + f"- **Blocking:** {question}\n"))
    expect_hit(r, "blocking question in the plan", base(), rule, only=True,
               reasons=["plan: blocking question: Which component owns it?"],
               tail=plan_tail(questions=f"{assumed}- **Blocking:** Which component owns it?\n"))
    expect_hit(r, "blocking question with continuation lines", base(), rule, only=True,
               reasons=["analysis: blocking question: Which component owns it?"],
               analysis=analysis_document("- **Blocking:** Which component owns it?\n  It could be agents.\n"
                                          "  - or docs, as a sub-bullet\n\n" + assumed))
    expect_hit(r, "variant Blocking marker", base(), rule, only=True, reason="analysis: line 11 of '## Questions'",
               analysis=analysis_document("- **Blocking**: Which component owns it?\n"))
    expect_hit(r, "prose in Questions", base(), rule, only=True, reason="is neither",
               analysis=analysis_document("None.\n"))
    expect_hit(r, "blocking question past a long text", base(), rule,
               reasons=["analysis: blocking question: " + "x" * 200],
               analysis=analysis_document("- **Blocking:** " + "x" * 300 + "\n"))
    expect_hit(r, "analysis without Questions", base(), rule, only=True,
               reason="analysis: no '## Questions' section", analysis=analysis_document(heading="## Remarks"))
    expect_hit(r, "analysis with two Questions sections", base(), rule, only=True,
               reason="analysis: 2 '## Questions' sections",
               analysis=analysis_document(assumed + "\n## Questions\n\n" + assumed))
    expect_hit(r, "plan without Questions", base(), rule, only=True, reason="plan: no '## Questions' section",
               tail=plan_tail().replace("## Questions", "## Notes"))
    expect_hit(r, "no --analysis", base(), rule, only=True, reasons=["no analysis given (--analysis)"],
               analysis=None)
    expect_clean(r, "non-blocking questions only", base(), [rule], tail=plan_tail(questions=assumed),
                 analysis=analysis_document(assumed + "  with a continuation\n" + assumed))
    expect_clean(r, "Questions heading in a fence", base(), [rule],
                 analysis=analysis_document(assumed + "\n## Next\n\n- **Blocking:** after the section\n",
                                            heading="```\n## Questions\n- **Blocking:** in a fence\n```\n\n"
                                                    "## Questions"))


def check_acceptance_criteria(r: Runner) -> None:
    rule = "acceptance-criteria"
    escaped = [c.replace("|", "\\|") for c in CRITERIA]
    expect_hit(r, "criterion dropped", base(), rule, only=True,
               reasons=[f"the Verification table lacks the criterion: {CRITERIA[1]}"],
               tail=plan_tail(rows=[escaped[0], escaped[2]]))
    expect_hit(r, "criterion added", base(), rule, only=True,
               reasons=["the Verification table has a row that is not a criterion of the issue: An extra row."],
               tail=plan_tail(rows=escaped + ["An extra row."]))
    reworded = "A reason names the criterion it concerns."
    expect_hit(r, "criterion reworded", base(), rule, only=True,
               reasons=[f"the Verification table lacks the criterion: {CRITERIA[1]}",
                        f"the Verification table has a row that is not a criterion of the issue: {reworded}"],
               tail=plan_tail(rows=[escaped[0], reworded, escaped[2]]))
    expect_hit(r, "criterion repeated", base(), rule, only=True,
               reason=f"not a criterion of the issue: {CRITERIA[0]}", tail=plan_tail(rows=escaped + escaped[:1]))
    expect_hit(r, "issue without criteria", base(), rule, only=True, reason="holds no criteria",
               issue=issue_document(criteria="The plan should be good.\n"))
    expect_hit(r, "issue without the criteria section", base(), rule, reason="no '## Acceptance criteria' section",
               issue=issue_document().replace("## Acceptance criteria", "## Done when"))
    expect_hit(r, "two Verification sections", base(), rule, only=True, reason="2 '## Verification' sections",
               tail=plan_tail().replace("\n## Questions", "\n## Verification\n\nAgain.\n\n## Questions"))
    expect_hit(r, "Verification without a table", base(), rule, only=True, reason="does not start with a table",
               tail=plan_tail().replace("| --- | --- |\n", ""))
    expect_hit(r, "no --issue-document", base(), rule, reasons=["no issue document given (--issue-document)"],
               issue=None)
    spaced = ["  The floor reads  the issue\tdocument and the analysis.   ", escaped[1], escaped[2]]
    expect_clean(r, "criteria differing in whitespace", base(), [rule], tail=plan_tail(rows=spaced))
    wrapped = (f"1. {CRITERIA[0][:20]}\n   {CRITERIA[0][20:]}\n2. {CRITERIA[1]}\n"
               f"3. {CRITERIA[2][:10]}\n{CRITERIA[2][10:]}\n\nProse after the list.\n")
    expect_clean(r, "wrapped issue criteria", base(), [rule], issue=issue_document(criteria=wrapped))
    expect_clean(r, "escaped bar", base(), [rule], tail=plan_tail(rows=escaped))
    markers = (f"Done when:\n\n- [ ] {CRITERIA[0]}\n* [x] {CRITERIA[1]}\n"
               f"1) {CRITERIA[2]}\n")
    expect_clean(r, "list markers and task boxes", base(), [rule], issue=issue_document(criteria=markers))
    expect_clean(r, "rows reordered", base(), [rule], tail=plan_tail(rows=list(reversed(escaped))))
    expect_clean(r, "decoy criteria in comments", base(), [rule], issue=issue_document())
    expect_clean(r, "CRLF issue document", base(), [rule], issue=issue_document().replace("\n", "\r\n"))


def check_first_of_kind(r: Runner) -> None:
    rule = "first-of-kind"
    for component, files in (("cli", ["crates/cli/src/main.rs"]), ("core", ["crates/core/src/lib.rs"]),
                             ("runner", ["crates/runner/src/lib.rs"]), ("domain", ["crates/domain/src/lib.rs"])):
        expect_hit(r, f"first {component} job", base(files=files), rule, component=component,
                   reasons=["first of its kind: no commit reachable from the base commit has a subject "
                            f"scoped ({component})"])
    expect_clean(r, "cli job after a cli commit", base(files=["crates/cli/src/main.rs"]), [rule],
                 component="cli", config="dropped")
    expect_clean(r, "api job after a breaking api commit", base(files=["api/jobs.proto"]), [rule],
                 component="api")
    expect_clean(r, "docs job after a docs commit", base(), [rule])
    expect_hit(r, "issue kind plan", base(), rule, only=True,
               reasons=["the issue's kind is 'plan', not implementation"], issue=issue_document(kind="plan"))
    expect_hit(r, "issue component differs", base(), rule,
               reasons=["the issue's component is 'web', not --component docs"],
               issue=issue_document(component="web"))
    header = "- **Kind:** implementation\n- **Kind:** implementation\n- **Component:** docs\n"
    expect_hit(r, "issue kind twice", base(), rule, only=True,
               reasons=["the issue document's header has 2 '- **Kind:**' lines, expected 1"],
               issue=issue_document(header=header))
    expect_hit(r, "issue without a component line", base(), rule, only=True,
               reasons=["the issue document's header has 0 '- **Component:**' lines, expected 1"],
               issue=issue_document(header="- **Kind:** implementation\n"))
    expect_hit(r, "no --issue-document for first-of-kind", base(), rule,
               reasons=["no issue document given (--issue-document)"], issue=None)


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
    for variant in ("no-plan-approval", "unknown-key", "mode-sometimes", "limit-negative", "limit-true",
                    "undefined-component", "missing-limit"):
        cases.append((f"config {variant}", ["--plan", good, "--component", "docs", "--base", commits[variant]]))
    issue = sb.write_plan("unusable issue", issue_document())
    analysis = sb.write_plan("unusable analysis", analysis_document())
    plain = ["--plan", good, "--component", "docs", "--base", real]
    for flag, path in (("--issue-document", issue), ("--analysis", analysis)):
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
    for name, args in cases:
        proc, verdict = sb.floor(args)
        T.check(f"unusable {name}: exit 2, no verdict", proc.returncode == 2 and verdict is None
                and len(proc.stderr.strip().splitlines()) == 1,
                f"exit {proc.returncode}, verdict {'left' if verdict is not None else 'gone'}: {proc.stderr.strip()}")

    bindir = sb.tmp / "git-only"
    bindir.mkdir()
    (bindir / "git").symlink_to(shutil.which("git"))
    env = dict(sb.env, PATH=str(bindir))
    proc, verdict = sb.floor(["--plan", good, "--component", "docs", "--base", real], env=env)
    T.check("unusable yq missing: exit 2, no verdict", proc.returncode == 2 and verdict is None,
            f"exit {proc.returncode}: {proc.stderr.strip()}")


def check_verdict_errors(sb: Sandbox, commits: dict) -> None:
    good = sb.write_plan("verdict errors good", plan_text())
    nofootprint = sb.write_plan("verdict errors no footprint",
                                plan_text(section="", opener="Nothing here.", block="", closer="\n"))
    args = ["--component", "docs", "--base", commits["real"],
            "--issue-document", sb.write_plan("verdict errors issue", issue_document()),
            "--analysis", sb.write_plan("verdict errors analysis", analysis_document())]
    verdict = sb.repo / VERDICT
    outputs = verdict.parent

    def exit_2(name: str, proc, unexpected: bool = False) -> None:
        lines = proc.stderr.strip().splitlines()
        ok = (proc.returncode == 2 and len(lines) == 1 and "Traceback" not in proc.stderr
              and (not unexpected or "unexpected error" in proc.stderr))
        T.check(name, ok, f"exit {proc.returncode}: {proc.stderr.strip()}")

    outputs.mkdir(parents=True, exist_ok=True)
    verdict.unlink(missing_ok=True)
    verdict.mkdir()
    proc = sb.exec(["--plan", good, *args])
    exit_2("verdict a directory: exit 2, one-line error", proc)
    T.check("verdict a directory: no verdict written", verdict.is_dir() and not any(verdict.iterdir())
            and not list(outputs.glob(".floor.*")))
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
        T.check("read-only outputs, no stale verdict: no verdict written",
                not verdict.exists() and not list(outputs.glob(".floor.*")))
    finally:
        outputs.chmod(0o755)


# --- decision -------------------------------------------------------------

RECORD_KEYS = [
    "tool", "version", "approval", "subject", "mode", "layers", "judged_decision", "decision", "reasons",
]
FLOOR_LAYER_KEYS = ["status", "problems", "path", "sha256", "verdict", "rules", "not_evaluated"]
JUDGEMENT_LAYER_KEYS = [
    "status", "problems", "stage", "path", "sha256", "artifact", "harness", "model", "expected",
    "as_configured", "risk", "reasons",
]
EXPECTED_JUDGE = {"harness": "claude", "model": "sonnet"}
REASON = "the change stays within the docs component"
ARTIFACT = "https://github.com/mintswarm/mintswarm/issues/34/artifacts/7"
SPAN_LIMIT = 200


def assessment_text(level: str = "low", reason: str = REASON, risk_line: str = None, summary: str = None,
                    eol: str = "\n", title: str = "# Assessment: selftest case") -> str:
    risk_line = f"Risk: {level}" if risk_line is None else risk_line
    summary = f"{level} risk: {reason}" if summary is None else summary
    lines = [title, "", summary, "", risk_line, "", "## Reversibility", "", "A revert undoes it.", ""]
    return eol.join(lines)


def code_spans(text: str) -> list:
    return re.findall(r"`([^`\n]*)`", text)


def outside_spans(text: str) -> str:
    return re.sub(r"`[^`\n]*`", "", text)


# The plans Decider prepares: (plan text, issue document, analysis, floor.py's exit code).
DECIDER_PLANS = {
    "clean": (plan_text(), issue_document(), analysis_document(), 0),
    "hit": (plan_text(footprint=base(files=["CLAUDE.md"])), issue_document(), analysis_document(), 1),
    "blocking": (plan_text(), issue_document(),
                 analysis_document("- **Blocking:** Which component owns it?\n"), 1),
    "no footprint": (plan_text(section="", opener="Nothing here.", block="", closer="\n"), issue_document(),
                     analysis_document(), 1),
    "kind plan": (plan_text(), issue_document(kind="plan"), analysis_document(), 1),
}


class Decider:
    """Runs decision.py in the sandbox after a real floor.py run at the same base commit and documents."""

    def __init__(self, sb: Sandbox, commits: dict):
        self.sb = sb
        self.commits = commits
        self.prepared = {}

    def floor_verdict(self, ctx: dict, issue: str = None, analysis: str = None, documents: bool = True) -> bytes:
        """floor.py's verdict for ctx's plan, with ctx's documents, other ones, or none."""
        args = ["--plan", ctx["plan"], "--component", ctx["component"], "--base", ctx["commit"]]
        if documents:
            args += ["--issue-document", issue or ctx["issue"], "--analysis", analysis or ctx["analysis"]]
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
            }
            proc = self.sb.exec(["--plan", ctx["plan"], "--component", "docs", "--base", ctx["commit"],
                                 "--issue-document", ctx["issue"], "--analysis", ctx["analysis"]])
            if proc.returncode != code:
                raise RuntimeError(f"selftest bug: floor.py exit {proc.returncode} for {key}: {proc.stderr.strip()}")
            ctx["verdict"] = (self.sb.repo / VERDICT).read_bytes()
            self.prepared[key] = ctx
        return self.prepared[key]

    def run(self, name: str, config: str = "judged", plan: str = "clean", verdict="floor", assessment="low",
            harness: str = "claude", model: str = "sonnet", artifact: str = ARTIFACT):
        """verdict: "floor" (the real one), None (missing), a function editing the JSON, or bytes."""
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
        if assessment is None:
            rel = "cases/no-such-assessment.md"
            level = None
        else:
            content = assessment_text(assessment) if assessment in ("low", "medium", "high") else assessment
            rel = self.sb.write_plan(f"assessment {name}", content)
            level = assessment if assessment in ("low", "medium", "high") else None
        args = ["--issue", "34", "--component", ctx["component"], "--plan", ctx["plan"],
                "--issue-document", ctx["issue"], "--analysis", ctx["analysis"], "--assessment", rel,
                "--assessment-artifact", artifact, "--base", ctx["commit"], "--harness", harness, "--model", model]
        record_path = self.sb.repo / RECORD
        record_path.write_text('{"stale": true}\n', encoding="utf-8")
        proc = self.sb.exec(args, script=DECISION)
        record = None
        if record_path.exists():
            try:
                record = json.loads(record_path.read_text(encoding="utf-8"))
            except ValueError:
                record = "unparsable"
        expect = {"ctx": ctx, "config": config, "assessment": rel, "artifact": artifact, "harness": harness,
                  "model": model, "level": level}
        check_record(name, proc, record, expect, self.sb.repo)
        return proc, record


def check_record(name: str, proc, record, expect: dict, repo: Path) -> bool:
    if not isinstance(record, dict):
        return T.check(f"{name}: record shape", False,
                       f"no record (exit {proc.returncode}): {proc.stderr.strip()}")
    problems = []
    ctx = expect["ctx"]
    mode = {"real": EXPECTED_APPROVAL["mode"]}.get(expect["config"], expect["config"])
    if list(record) != RECORD_KEYS:
        return T.check(f"{name}: record shape", False, f"keys {list(record)}")
    if (record["tool"], record["version"], record["approval"]) != ("mintswarm-approval-decision", 1, "plan"):
        problems.append("tool, version or approval")
    plan_sha = hashlib.sha256((repo / ctx["plan"]).read_bytes()).hexdigest()
    subject = {"issue": 34, "component": ctx["component"], "plan": {"path": ctx["plan"], "sha256": plan_sha},
               "base": {"ref": ctx["commit"], "commit": ctx["commit"]}}
    if record["subject"] != subject:
        problems.append(f"subject {record['subject']}")
    if record["mode"] != mode:
        problems.append(f"mode {record['mode']!r}, expected {mode!r}")
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
                verdict["verdict"], verdict["rules"], verdict["not_evaluated"]):
            problems.append("floor layer differs from the verdict")
        if (len(fl["rules"]) + len(fl["not_evaluated"]) != len(RULES)
                or not all(list(r) == ["rule", "result", "reasons"] for r in fl["rules"])):
            problems.append(f"floor layer rules and not_evaluated are not the {len(RULES)} rules")
    elif (fl["verdict"], fl["rules"], fl["not_evaluated"]) != (None, [], []):
        problems.append("unusable floor layer holds content")

    assessment = repo / expect["assessment"]
    assessment_sha = hashlib.sha256(assessment.read_bytes()).hexdigest() if assessment.exists() else None
    as_configured = {"harness": expect["harness"], "model": expect["model"]} == EXPECTED_JUDGE
    fixed = {"stage": "assess", "path": expect["assessment"], "sha256": assessment_sha,
             "artifact": expect["artifact"], "harness": expect["harness"], "model": expect["model"],
             "expected": EXPECTED_JUDGE, "as_configured": as_configured}
    wrong = [k for k, v in fixed.items() if jd[k] != v]
    if wrong:
        problems.append(f"judgement {', '.join(wrong)}")
    if jd["status"] == "usable":
        if jd["risk"] not in ("low", "medium", "high") or len(jd["reasons"]) != 1:
            problems.append(f"judgement risk {jd['risk']!r}, reasons {jd['reasons']!r}")
        if expect["level"] is not None and (jd["risk"], jd["reasons"]) != (expect["level"], [REASON]):
            problems.append(f"judgement risk {jd['risk']!r}, reasons {jd['reasons']!r}")
    elif (jd["risk"], jd["reasons"]) != (None, []):
        problems.append("unusable judgement layer holds content")

    clean = fl["status"] == "usable" and fl["verdict"] == "clean"
    low = jd["status"] == "usable" and jd["risk"] == "low"
    judged = "auto" if clean and low and as_configured else "human"
    decision = {"always": "human", "shadow": "human", "never": "auto", "judged": judged}.get(mode)
    if record["judged_decision"] != judged:
        problems.append(f"judged_decision {record['judged_decision']!r}, expected {judged!r}")
    if record["decision"] != decision:
        problems.append(f"decision {record['decision']!r}, expected {decision!r}")
    if proc.returncode != (0 if record["decision"] == "auto" else 1):
        problems.append(f"exit {proc.returncode} with decision {record['decision']!r}")
    reasons = record["reasons"]
    if (not isinstance(reasons, list) or not all(isinstance(x, str) for x in reasons)
            or not reasons or not reasons[0].startswith(f"mode {mode}:")):
        problems.append(f"reasons {reasons!r}")
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


def expect_decision(name: str, result, decision: str, judged: str = None, floor_status: str = None,
                    judgement_status: str = None) -> dict:
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
    T.check(f"{name}: decision {decision}" + "".join(f", {k} {v}" for k, v in want.items() if k != "decision"),
            got == want and proc.returncode == (0 if decision == "auto" else 1), f"got {got}, exit {proc.returncode}")
    return record


def check_comment(name: str, proc, floor_line: str, risk_line: str) -> None:
    out = proc.stdout
    lines = out.splitlines()
    ok = (bool(lines) and lines[0] == "### Plan approved without a human"
          and f"- **Floor:** {floor_line}" in lines and f"- **Risk:** {risk_line}" in lines
          and any(line.startswith("**To veto:**") and "awaiting plan approval" in line
                  and "rejection at PR approval" in line for line in lines)
          and "PR approval stays with a human in every mode." in lines)
    T.check(f"{name}: comment has the heading, floor, risk and veto", ok, repr(out))


def check_shadow_comment(name: str, proc, floor_start: str, risk_line: str, judged: str) -> None:
    lines = proc.stdout.splitlines()
    floor = next((line for line in lines if line.startswith("- **Floor:** ")), "")
    ok = (bool(lines) and lines[0] == "### Shadow record: plan approval"
          and any(line.startswith("- **Plan:** ") and "footprint: " in line for line in lines)
          and any(line.startswith("- **Assess:** stage `assess`, `claude` with `sonnet`, as configured") for line in lines)
          and f"- **Risk:** {risk_line}" in lines
          and floor_start in floor and "clean: `" in floor
          and f"- **Would-be decision:** `{judged}` (`judged_decision`)" in lines
          and "- **Decision:** `human`" in lines and "- **Maintainer decision:** pending" in lines)
    T.check(f"{name}: shadow-record comment has the heading and rows", ok, repr(proc.stdout))


def check_decision_judged(d: Decider) -> None:
    proc, record = d.run("judged clean low")
    expect_decision("judged clean low", (proc, record), "auto", "auto", "usable", "usable")
    check_comment("judged clean low", proc, f"clean, {len(RULES)} of {len(RULES)} rules", f"`low`, `{REASON}`")
    expect_decision("judged hit low", d.run("judged hit low", plan="hit"), "human", "human", "usable", "usable")
    for level in ("medium", "high"):
        expect_decision(f"judged clean {level}", d.run(f"judged clean {level}", assessment=level),
                        "human", "human", "usable", "usable")
    expect_decision("judged no verdict", d.run("judged no verdict", verdict=None), "human", "human", "unusable")
    expect_decision("judged unparsable verdict", d.run("judged unparsable verdict", verdict=b"{ not json\n"),
                    "human", "human", "unusable")
    duplicate = d.prepare("judged", "clean")["verdict"].replace(b'"mode": "judged",',
                                                                b'"mode": "judged",\n  "mode": "judged",', 1)
    expect_decision("judged duplicate key", d.run("judged duplicate key", verdict=duplicate),
                    "human", "human", "unusable")

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
        expect_decision(f"judged verdict {label}", d.run(f"judged verdict {label}", verdict=edit),
                        "human", "human", "unusable", "usable")

    bad_assessments = [
        ("missing assessment", None),
        ("assessment not UTF-8", b"# Assessment\n\nlow risk: x\n\nRisk: low\n\xff\n"),
        ("no Risk line", assessment_text(risk_line="Rating: low")),
        ("two Risk lines", assessment_text() + "Risk: low\n"),
        ("Risk: Low", assessment_text(risk_line="Risk: Low")),
        ("leading space", assessment_text(risk_line=" Risk: low")),
        ("Risk: none", assessment_text(risk_line="Risk: none")),
        ("Risk: low and more", assessment_text(risk_line="Risk: low, probably")),
        ("Risk:low", assessment_text(risk_line="Risk:low")),
        ("summary level differs", assessment_text(summary=f"medium risk: {REASON}")),
        ("summary without reason", assessment_text(summary="low risk: ")),
        ("summary reason of format characters", assessment_text(summary="low risk: \u200b\u2060")),
        ("no summary line", assessment_text(summary="A plan.")),
        ("no title", assessment_text(title="Assessment: selftest case")),
    ]
    for label, content in bad_assessments:
        expect_decision(f"judged {label}", d.run(f"judged {label}", assessment=content),
                        "human", "human", "usable", "unusable")

    for label, content in (("trailing space", assessment_text(risk_line="Risk: low \t")),
                           ("CRLF", assessment_text(eol="\r\n"))):
        proc, record = d.run(f"judged {label}", assessment=content)
        expect_decision(f"judged {label}", (proc, record), "auto", "auto", "usable", "usable")
        T.check(f"judged {label}: risk low with the reason", isinstance(record, dict)
                and record["layers"]["judgement"]["reasons"] == [REASON])

    for label, harness, model in (("other model", "claude", "opus"), ("other harness", "codex", "sonnet")):
        proc, record = d.run(f"judged {label}", harness=harness, model=model)
        expect_decision(f"judged {label}", (proc, record), "human", "human", "usable", "usable")
        T.check(f"judged {label}: as_configured false, with a reason", isinstance(record, dict)
                and record["layers"]["judgement"]["as_configured"] is False
                and "judgement: harness or model not as configured" in record["reasons"])

    clean_verdict = json.loads(d.prepare("judged", "clean")["verdict"])
    hit_verdict = json.loads(d.prepare("judged", "hit")["verdict"])
    forged = dict(clean_verdict, plan=hit_verdict["plan"])
    proc, record = d.run("judged forged clean verdict", plan="hit",
                         verdict=(json.dumps(forged, indent=2) + "\n").encode("utf-8"))
    expect_decision("judged forged clean verdict", (proc, record), "human", "human", "unusable", "usable")
    T.check("judged forged clean verdict: the recomputed rules differ", isinstance(record, dict)
            and record["layers"]["floor"]["problems"]
            == ["the floor verdict's rules differ from what floor.py gives for this plan and base"],
            str(record and record.get("layers", {}).get("floor", {}).get("problems")))

    def other_reason(document):
        for rule in document["rules"]:
            if rule["result"] == "hit":
                rule["reasons"] = ["an edited reason"]

    expect_decision("judged verdict reason edited", d.run("judged verdict reason edited", plan="hit",
                                                          verdict=other_reason), "human", "human", "unusable")


def floor_problems(record) -> list:
    if not isinstance(record, dict):
        return []
    return record.get("layers", {}).get("floor", {}).get("problems")


def check_decision_documents(d: Decider) -> None:
    proc, record = d.run("judged blocking question", plan="blocking")
    expect_decision("judged blocking question", (proc, record), "human", "human", "usable", "usable")
    floor_ = record.get("layers", {}).get("floor", {}) if isinstance(record, dict) else {}
    T.check("judged blocking question: the floor layer holds the blocking-questions hit",
            [r["rule"] for r in floor_.get("rules", []) if r["result"] == "hit"] == ["blocking-questions"],
            str(floor_.get("rules")))
    proc, record = d.run("judged footprint hit", plan="no footprint")
    expect_decision("judged footprint hit", (proc, record), "human", "human", "usable", "usable")
    floor_ = record.get("layers", {}).get("floor", {}) if isinstance(record, dict) else {}
    T.check("judged footprint hit: footprint rules not evaluated, document rules evaluated",
            floor_.get("not_evaluated") == FOOTPRINT_RULES
            and [r["rule"] for r in floor_.get("rules", [])] == ["footprint", *DOCUMENT_RULES],
            str(floor_.get("not_evaluated")))
    proc, record = d.run("shadow blocking question", config="real", plan="blocking")
    expect_decision("shadow blocking question", (proc, record), "human", "human", "usable", "usable")
    check_shadow_comment("shadow blocking question", proc, "hit on `blocking-questions` (`floor.py` exit 1)",
                         f"`low`, `{REASON}`", "human")

    clean = d.prepare("judged", "clean")
    without = d.floor_verdict(clean, documents=False)
    proc, record = d.run("judged verdict without documents", verdict=without)
    expect_decision("judged verdict without documents", (proc, record), "human", "human", "unusable", "usable")
    T.check("judged verdict without documents: both bindings named", floor_problems(record) == [
        "the floor verdict's issue_document does not match --issue-document",
        "the floor verdict's analysis does not match --analysis"], str(floor_problems(record)))
    other = d.sb.write_plan("decision other analysis", analysis_document("- **Non-blocking (assumed):** Other?\n"))
    proc, record = d.run("judged verdict with another analysis", verdict=d.floor_verdict(clean, analysis=other))
    expect_decision("judged verdict with another analysis", (proc, record), "human", "human", "unusable", "usable")
    T.check("judged verdict with another analysis: the analysis binding named",
            floor_problems(record) == ["the floor verdict's analysis does not match --analysis"],
            str(floor_problems(record)))
    other = d.sb.write_plan("decision other issue", issue_document() + "\nAnother comment.\n")
    proc, record = d.run("judged verdict with another issue", verdict=d.floor_verdict(clean, issue=other))
    expect_decision("judged verdict with another issue", (proc, record), "human", "human", "unusable", "usable")
    T.check("judged verdict with another issue: the issue_document binding named",
            floor_problems(record) == ["the floor verdict's issue_document does not match --issue-document"],
            str(floor_problems(record)))

    def first_of_kind_clean(document):
        for rule in document["rules"]:
            if rule["rule"] == "first-of-kind":
                rule.update(result="clean", reasons=[])
        document["verdict"] = "clean"

    expect_decision("judged kind plan", d.run("judged kind plan", plan="kind plan"),
                    "human", "human", "usable", "usable")
    proc, record = d.run("judged forged first-of-kind clean", plan="kind plan", verdict=first_of_kind_clean)
    expect_decision("judged forged first-of-kind clean", (proc, record), "human", "human", "unusable", "usable")
    T.check("judged forged first-of-kind clean: the recomputed rules differ", floor_problems(record)
            == ["the floor verdict's rules differ from what floor.py gives for this plan and base"],
            str(floor_problems(record)))

    def footprint_skips_one_more(document):
        document["rules"] = [r for r in document["rules"] if r["rule"] != "first-of-kind"]
        document["not_evaluated"] = FOOTPRINT_RULES + ["first-of-kind"]

    def footprint_rules_skipped_on_clean(document):
        document["rules"] = [r for r in document["rules"] if r["rule"] not in FOOTPRINT_RULES]
        document["not_evaluated"] = FOOTPRINT_RULES

    for label, plan, edit, problem in (
            ("a document rule skipped", "no footprint", footprint_skips_one_more,
             "the floor verdict's rule identifiers are not floor.py's, in order, "
             "with none or only the footprint rules not evaluated"),
            ("footprint rules skipped without a hit", "clean", footprint_rules_skipped_on_clean,
             "the floor verdict skips rules without a footprint hit")):
        proc, record = d.run(f"judged verdict {label}", plan=plan, verdict=edit)
        expect_decision(f"judged verdict {label}", (proc, record), "human", "human", "unusable", "usable")
        T.check(f"judged verdict {label}: the identifier check names it", floor_problems(record) == [problem],
                str(floor_problems(record)))


def check_decision_modes(d: Decider) -> None:
    expect_decision("always clean low", d.run("always clean low", config="always"), "human", "auto")
    expect_decision("always hit high", d.run("always hit high", config="always", plan="hit", assessment="high"),
                    "human", "human")
    proc, record = d.run("shadow clean low", config="real")
    expect_decision("shadow clean low", (proc, record), "human", "auto")
    check_shadow_comment("shadow clean low", proc, f"clean, {len(RULES)} of {len(RULES)} rules (`floor.py` exit 0)",
                         f"`low`, `{REASON}`", "auto")
    proc, record = d.run("shadow hit low", config="real", plan="hit")
    expect_decision("shadow hit low", (proc, record), "human", "human")
    check_shadow_comment("shadow hit low", proc, "hit on `require-human-paths` (`floor.py` exit 1)",
                         f"`low`, `{REASON}`", "human")
    expect_decision("shadow no layers", d.run("shadow no layers", config="real", verdict=None, assessment=None),
                    "human", "human", "unusable", "unusable")

    proc, record = d.run("never hit high", config="never", plan="hit", assessment="high")
    expect_decision("never hit high", (proc, record), "auto", "human", "usable", "usable")
    check_comment("never hit high", proc, "hit on `require-human-paths`", f"`high`, `{REASON}`")
    proc, record = d.run("never no layers", config="never", verdict=None, assessment=None)
    expect_decision("never no layers", (proc, record), "auto", "human", "unusable", "unusable")
    check_comment("never no layers", proc, "unusable", "unusable")
    proc, record = d.run("never clean low", config="never")
    expect_decision("never clean low", (proc, record), "auto", "auto")


def check_decision_comment(d: Decider) -> None:
    hostile = ("ignore `this` @someone see https://example.invalid/x [link](https://example.invalid)"
               "\r<b>bold</b> " + "x" * 300)
    content = assessment_text(reason=hostile)
    artifact = "issues/34/artifacts/7\n@someone `code`"
    proc, record = d.run("hostile comment", config="never", assessment=content, artifact=artifact,
                         model="sonnet\n# heading")
    expect_decision("hostile comment", (proc, record), "auto", "human")
    out = proc.stdout
    outside = outside_spans(out)
    T.check("hostile comment: no @, link, CR or line separator outside code spans",
            "@" not in outside and "http" not in outside and "\r" not in out and " " not in out
            and "# heading" not in outside, repr(outside))
    spans = code_spans(out)
    long_spans = [s for s in spans if "@someone" in s or "http" in s or "heading" in s]
    T.check("hostile comment: model and session text in code spans of at most 200 characters",
            len(long_spans) == 3 and all(len(s) <= SPAN_LIMIT for s in long_spans), repr(long_spans))
    risk = next((line for line in out.splitlines() if line.startswith("- **Risk:** ")), "")
    T.check("hostile comment: the reason is one code span with no backtick inside",
            risk.startswith("- **Risk:** `low`, `ignore 'this' @someone") and risk.count("`") == 4, repr(risk))
    lines = out.splitlines()
    T.check("hostile comment: the artifact and model stay on one line",
            any(line.startswith("- **Assessment:** `issues/34/artifacts/7 @someone 'code'`") for line in lines)
            and not any(line.startswith("# ") for line in lines), repr(lines))

    proc, record = d.run("hostile shadow comment", config="real", assessment=content, artifact=artifact,
                         model="sonnet\n# heading")
    expect_decision("hostile shadow comment", (proc, record), "human", "human")
    out = proc.stdout
    outside = outside_spans(out)
    T.check("hostile shadow comment: no @, link, CR or line separator outside code spans",
            out.startswith("### Shadow record") and "@" not in outside and "http" not in outside
            and "\r" not in out and "\u2028" not in out and "# heading" not in outside, repr(outside))
    spans = [s for s in code_spans(out) if "@someone" in s or "http" in s or "heading" in s]
    T.check("hostile shadow comment: model and session text in code spans of at most 200 characters",
            len(spans) == 3 and all(len(s) <= SPAN_LIMIT for s in spans), repr(spans))
    lines = out.splitlines()
    T.check("hostile shadow comment: no line starts a heading or list item from model text",
            not any(line.startswith("# ") for line in lines)
            and all(line.startswith(("### ", "Mode ", "- **")) or not line for line in lines), repr(lines))


def check_decision_unusable(sb: Sandbox, d: Decider) -> None:
    ctx = d.prepare("judged", "clean")
    assessment = sb.write_plan("unusable assessment", assessment_text())
    not_utf8 = sb.write_plan("decision not utf-8", b"# Plan\n\n\xff\xfe\n")

    def args(**changes) -> list:
        values = {"--issue": "34", "--component": "docs", "--plan": ctx["plan"],
                  "--issue-document": ctx["issue"], "--analysis": ctx["analysis"], "--assessment": assessment,
                  "--assessment-artifact": ARTIFACT, "--base": ctx["commit"], "--harness": "claude",
                  "--model": "sonnet"}
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
        ("missing --issue-document", args(issue_document=None)),
        ("missing --analysis", args(analysis=None)),
        ("issue document missing", args(issue_document="cases/missing.md")),
        ("issue document unreadable", args(issue_document="cases")),
        ("issue document not UTF-8", args(issue_document=not_utf8)),
        ("analysis missing", args(analysis="cases/missing.md")),
        ("analysis unreadable", args(analysis="cases")),
        ("analysis not UTF-8", args(analysis=not_utf8)),
    ]
    for name, argv in cases:
        record.write_text('{"stale": true}\n', encoding="utf-8")
        proc = sb.exec(argv, script=DECISION)
        T.check(f"decision unusable {name}: exit 2, no record, empty stdout",
                proc.returncode == 2 and not record.exists() and proc.stdout == ""
                and len(proc.stderr.strip().splitlines()) == 1,
                f"exit {proc.returncode}, record {'left' if record.exists() else 'gone'}: {proc.stderr.strip()}")

    bindir = sb.tmp / "git-only-decision"
    bindir.mkdir()
    (bindir / "git").symlink_to(shutil.which("git"))
    record.write_text('{"stale": true}\n', encoding="utf-8")
    proc = sb.exec(args(), env=dict(sb.env, PATH=str(bindir)), script=DECISION)
    T.check("decision unusable yq missing: exit 2, no record", proc.returncode == 2 and not record.exists(),
            f"exit {proc.returncode}: {proc.stderr.strip()}")

    record.unlink(missing_ok=True)
    record.mkdir()
    try:
        proc = sb.exec(args(), script=DECISION)
        T.check("decision record a directory: exit 2, one-line error, no record written",
                proc.returncode == 2 and len(proc.stderr.strip().splitlines()) == 1 and "Traceback" not in proc.stderr
                and record.is_dir() and not any(record.iterdir()) and not list(record.parent.glob(".decision.*"))
                and proc.stdout == "", f"exit {proc.returncode}: {proc.stderr.strip()}")
    finally:
        shutil.rmtree(record, ignore_errors=True)


def check_decision(sb: Sandbox, commits: dict) -> None:
    d = Decider(sb, commits)
    check_decision_judged(d)
    check_decision_documents(d)
    check_decision_modes(d)
    check_decision_comment(d)
    check_decision_unusable(sb, d)


def load_module(name: str, path: Path):
    sys.dont_write_bytecode = True  # leave no __pycache__ next to either script
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def check_globs() -> None:
    floor = load_module("msw_floor", FLOOR)
    review = load_module("msw_request_review", REQUEST_REVIEW)
    patterns = ["**", "docs/**", "**/x.md", "a/**/b", "a/**", "*.md", "docs/*.md", "?.md", "a?c",
                "a.b", "[ab].md", "a+b(c)", "README.md", ".mintswarm/dogfood/github/**", "crates/*/src/**"]
    paths = ["docs", "docs/a.md", "docs/x/y.md", "x.md", "a/x.md", "a/b/x.md", "a/b", "a/c/d/b", "a/",
             "a.md", "ab.md", "abc", "a/c", "aXb", "a.b", "[ab].md", "a+b(c)", "README.md", "readme.md",
             ".mintswarm/dogfood/github/push.sh", ".mintswarm/dogfood/githubx", "crates/core/src/lib.rs",
             "crates/core/x/src/lib.rs"]
    differ = [(p, s) for p in patterns for s in paths
              if bool(floor.glob_to_regex(p).fullmatch(s)) != bool(review.glob_to_regex(p).fullmatch(s))]
    T.check(f"glob semantics: floor.py and request-review.py agree on {len(patterns) * len(paths)} pairs",
            not differ, str(differ[:5]))
    pinned = [("**/x.md", "x.md", True), ("**/x.md", "a/b/x.md", True), ("*.md", "docs/a.md", False),
              ("?.md", "a.md", True), ("a?c", "a/c", False), ("a.b", "aXb", False),
              ("[ab].md", "a.md", False), ("[ab].md", "[ab].md", True), ("docs/**", "docs", False)]
    wrong = [(p, s) for p, s, want in pinned if bool(floor.glob_to_regex(p).fullmatch(s)) != want]
    T.check("glob semantics: pinned cases", not wrong, str(wrong))


def check_stdlib() -> None:
    for path in (FLOOR, DECISION, Path(__file__).resolve()):
        tree = ast.parse(path.read_text(encoding="utf-8"))
        names = set()
        for node in ast.walk(tree):
            if isinstance(node, ast.Import):
                names.update(alias.name.split(".")[0] for alias in node.names)
            elif isinstance(node, ast.ImportFrom):
                names.add("." if node.level else node.module.split(".")[0])
        outside = sorted(n for n in names if n not in sys.stdlib_module_names)
        T.check(f"standard library: {path.name} imports only {', '.join(sorted(names))}", not outside,
                f"not standard library: {outside}")


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
    variant("tested", lambda c: c["gates"].update(tests="cargo test"))
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
        check_unusable_footprints(runner)
        check_base_ref(runner, sb)
        check_historic(runner)
        check_blocking_questions(runner)
        check_acceptance_criteria(runner)
        check_first_of_kind(runner)
        check_unusable_input(sb, commits)
        check_verdict_errors(sb, commits)
        check_decision(sb, commits)
    check_globs()
    check_stdlib()
    print(f"{T.passed} passed, {T.failed} failed")
    return 0 if T.failed == 0 else 1


if __name__ == "__main__":
    sys.exit(main())
