#!/usr/bin/env python3
"""Offline self-test of floor.py and decision.py, the two scripts of plan approval.

    selftest.py

Needs `git` and `yq` on PATH; nothing else, and no network. It works in a
temporary directory with its own HOME, no system or global git config and a
fixed identity, and runs `git init` on a throwaway repository with no
remote. Base configs are commits there: `real`, the checkout's
.mintswarm/config.yaml byte for byte, and variants of it, read with
`yq -o=json`, changed here and committed as JSON text at the same path.
floor.py and decision.py run as `sys.executable SCRIPT ...` with that
repository as its working directory and plans and assessments written under
`cases/`.

Checks:

- config: the checkout's config has no pipeline.approve_plan and has the
  expected pipeline.plan_approval;
- verdict shape, on every case that exits 0 or 1: the verdict's keys in
  order, the 13 rule identifiers in order across `rules` and
  `not_evaluated`, results and reasons, `verdict` against the rules and the
  exit code, `base.commit`, `mode` and the printed lines;
- clean base: a footprint modelled on #20 is clean on every rule, and is
  accepted with `mode: always`, which the verdict records;
- single-rule hits: each rule hit by a case derived from the clean base,
  and each limit clean at its value and a hit one over;
- footprint unusable: missing, misplaced, malformed and badly shaped
  footprints, and each bad path form, are `footprint` hits with the other
  rules not evaluated;
- base ref: with a worktree config and a HEAD commit that drop a
  require_human path, `--base` at the earlier commit still gives the hit;
- historic fixtures: fixtures/plan-{9,12,14,19,20}.md against the real
  config give the expected 13 results and reasons;
- unusable input: bad arguments, plans, refs and configs, and a missing
  `yq`, exit 2 and leave no verdict file, even a stale one;
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
  binding, a forged verdict that is consistent in itself but differs from
  what floor.py gives for the plan, and a missing, non-UTF-8 or malformed
  assessment are `human`, with the failing layer unusable; a trailing space
  and CRLF still give `auto`; a harness or model other than the config's
  sets `as_configured` false and gives `human`;
- decision under `always`, `shadow` and `never`: `human`, `human` with
  `judged_decision` recorded, and `auto` whatever the layers say;
- the comments: the auto-approval comment's heading, floor line, risk and
  reason and veto paragraph; the shadow-record comment's heading and rows;
  and in both, model and session text only inside code spans of at most
  200 characters;
- decision unusable input: bad arguments, plans, refs and configs, a
  missing `yq` and a record path that is a directory exit 2, print nothing
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
]
VERDICT_KEYS = [
    "tool", "version", "plan", "component", "base", "config", "mode",
    "verdict", "rules", "not_evaluated",
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
}


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


def plan_text(block: str = None, footprint: dict = None, approach: str = "", between: str = "",
              section: str = "## Footprint\n\n", opener: str = "```json", closer: str = "```\n",
              after: str = "") -> str:
    if block is None:
        block = footprint_block(footprint if footprint is not None else CLEAN_BASE)
    return (
        "# Plan: selftest case\n\nGenerated by selftest.py.\n\n"
        f"## Approach\n\nA generated case.\n{approach}\n"
        "## Steps\n\n1. **Change** — files: as declared.\n\n"
        f"{between}{section}{opener}\n{block}\n{closer}{after}"
    )


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
    if [r.get("rule") for r in rules] + list(skipped) != RULES:
        problems.append("rule identifiers not the 13 in order")
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
    if verdict.get("tool") != "mintswarm-plan-floor" or verdict.get("version") != 1:
        problems.append("tool or version")
    plan_bytes = (args["cwd"] / args["plan"]).read_bytes()
    if verdict.get("plan") != {"path": args["plan"], "sha256": hashlib.sha256(plan_bytes).hexdigest()}:
        problems.append(f"plan {verdict.get('plan')}")
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
            ref: str = None, commit: str = None, mode: str = None):
        mode = mode or EXPECTED_APPROVAL["mode"]
        plan = self.sb.write_plan(name, content)
        ref = ref or self.commits[config]
        commit = commit or self.commits[config]
        proc, verdict = self.sb.floor(["--plan", plan, "--component", component, "--base", ref])
        check_shape(name, proc, verdict, {"cwd": self.sb.repo, "plan": plan, "component": component, "base": ref},
                    commit, mode)
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
               reason: str = None, **kwargs):
    proc, verdict = r.run(name, plan_text(footprint=footprint), **kwargs)
    results = results_of(verdict)
    ok = proc.returncode == 1 and results.get(rule) == "hit"
    detail = f"exit {proc.returncode}, {rule} {results.get(rule)!r}"
    if ok and only:
        others = [k for k, v in results.items() if v == "hit" and k != rule]
        ok = not others
        detail = f"other hits {others}"
    if ok and reason is not None:
        ok = any(reason in x for x in reasons_of(verdict, rule))
        detail = f"no reason naming {reason!r}: {reasons_of(verdict, rule)}"
    T.check(f"{name}: {rule} hit", ok, detail)
    return verdict


def expect_clean(r: Runner, name: str, footprint: dict, rules: list, **kwargs):
    proc, verdict = r.run(name, plan_text(footprint=footprint), **kwargs)
    results = results_of(verdict)
    bad = [rule for rule in rules if results.get(rule) != "clean"]
    T.check(f"{name}: {', '.join(rules)} clean", proc.returncode in (0, 1) and not bad, f"not clean: {bad}")
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
    for name, text in unusable_footprints():
        proc, verdict = r.run(name, text)
        ok = (proc.returncode == 1 and results_of(verdict) == {"footprint": "hit"}
              and verdict.get("not_evaluated") == RULES[1:])
        T.check(f"{name}: footprint hit, nothing else evaluated", ok,
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
    for fixture, (component, hits) in HISTORIC.items():
        content = (FIXTURES / fixture).read_bytes()
        proc, verdict = r.run(f"fixture {fixture}", content, component=component)
        expected = {rule: ("hit" if rule in hits else "clean") for rule in RULES}
        got = results_of(verdict)
        diff = {k: got.get(k) for k in RULES if got.get(k) != expected[k]}
        T.check(f"fixture {fixture}: the 13 expected results", got == expected and
                proc.returncode == (1 if hits else 0), f"exit {proc.returncode}, differs on {diff}")
        fixture_reasons = {
            "plan-9.md": ("require-human-components", "the issue's component agents"),
            "plan-12.md": ("require-human-components", "the issue's component agents"),
            "plan-14.md": ("require-human-paths", ".mintswarm/dogfood/github/**"),
            "plan-19.md": ("external-steps", "Code Owner review"),
        }
        if fixture in fixture_reasons:
            rule, text = fixture_reasons[fixture]
            T.check(f"fixture {fixture}: {rule} names {text!r}",
                    any(text in x for x in reasons_of(verdict, rule)), str(reasons_of(verdict, rule)))


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
    args = ["--component", "docs", "--base", commits["real"]]
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


class Decider:
    """Runs decision.py in the sandbox after a real floor.py run at the same base commit."""

    def __init__(self, sb: Sandbox, commits: dict):
        self.sb = sb
        self.commits = commits
        self.prepared = {}

    def prepare(self, config: str, plan: str) -> dict:
        key = (config, plan)
        if key not in self.prepared:
            footprint = CLEAN_BASE if plan == "clean" else base(files=["CLAUDE.md"])
            rel = self.sb.write_plan(f"decision {config} {plan}", plan_text(footprint=footprint))
            commit = self.commits[config]
            proc = self.sb.exec(["--plan", rel, "--component", "docs", "--base", commit])
            if proc.returncode != (0 if plan == "clean" else 1):
                raise RuntimeError(f"selftest bug: floor.py exit {proc.returncode} for {key}: {proc.stderr.strip()}")
            data = (self.sb.repo / VERDICT).read_bytes()
            self.prepared[key] = {"plan": rel, "commit": commit, "component": "docs", "verdict": data}
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
        args = ["--issue", "34", "--component", ctx["component"], "--plan", ctx["plan"], "--assessment", rel,
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
        if len(fl["rules"]) != len(RULES) or not all(list(r) == ["rule", "result", "reasons"] for r in fl["rules"]):
            problems.append("floor layer rules are not the 13 rule objects")
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
        ("version 2", setter("version", value=2)),
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
        values = {"--issue": "34", "--component": "docs", "--plan": ctx["plan"], "--assessment": assessment,
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
        check_unusable_input(sb, commits)
        check_verdict_errors(sb, commits)
        check_decision(sb, commits)
    check_globs()
    check_stdlib()
    print(f"{T.passed} passed, {T.failed} failed")
    return 0 if T.failed == 0 else 1


if __name__ == "__main__":
    sys.exit(main())
