#!/usr/bin/env python3
"""Offline self-test of floor.py, the deterministic floor of plan approval.

    selftest.py

Needs `git` and `yq` on PATH; nothing else, and no network. It works in a
temporary directory with its own HOME, no system or global git config and a
fixed identity, and runs `git init` on a throwaway repository with no
remote. Base configs are commits there: `real`, the checkout's
.mintswarm/config.yaml byte for byte, and variants of it, read with
`yq -o=json`, changed here and committed as JSON text at the same path.
floor.py runs as `sys.executable floor.py ...` with that repository as its
working directory and plans written under `cases/`.

Checks:

- config: the checkout's config has no pipeline.approve_plan and has the
  expected pipeline.plan_approval;
- verdict shape, on every case that exits 0 or 1: the verdict's keys in
  order, the 13 rule identifiers in order across `rules` and
  `not_evaluated`, results and reasons, `verdict` against the rules and the
  exit code, `base.commit`, `mode` and the printed lines;
- clean base: a footprint modelled on #20 is clean on every rule, and is
  accepted with `mode: shadow`, which the verdict records;
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
- glob semantics: floor.py's glob_to_regex agrees with the one in
  .mintswarm/dogfood/github/request-review.py;
- standard library: every import of floor.py and selftest.py is in the
  standard library.

Prints one `ok` or `FAIL` line per check and exits 0 only when all passed.
"""

import ast
import copy
import hashlib
import importlib.util
import json
import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
FLOOR = HERE / "floor.py"
FIXTURES = HERE / "fixtures"
ROOT = HERE.parents[2]
CONFIG = ROOT / ".mintswarm" / "config.yaml"
REQUEST_REVIEW = ROOT / ".mintswarm" / "dogfood" / "github" / "request-review.py"
CONFIG_PATH = ".mintswarm/config.yaml"
VERDICT = Path(".mintswarm/job/outputs/floor.json")

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
    "mode": "always",
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

    def floor(self, args: list, env: dict = None):
        stale = self.repo / VERDICT
        stale.parent.mkdir(parents=True, exist_ok=True)
        stale.write_text('{"stale": true}\n', encoding="utf-8")
        proc = subprocess.run(
            [sys.executable, str(FLOOR), *args],
            cwd=self.repo, env=env or self.env, capture_output=True, text=True,
        )
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
            ref: str = None, commit: str = None, mode: str = "always"):
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
    proc, verdict = r.run("shadow mode", plan_text(), config="shadow", mode="shadow")
    T.check("shadow mode: accepted, rules evaluated, mode recorded",
            proc.returncode == 0 and len(results_of(verdict)) == len(RULES)
            and isinstance(verdict, dict) and verdict.get("mode") == "shadow",
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
    for path in (FLOOR, Path(__file__).resolve()):
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
    variant("shadow", lambda c: approval(c).update(mode="shadow"))
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
    check_globs()
    check_stdlib()
    print(f"{T.passed} passed, {T.failed} failed")
    return 0 if T.failed == 0 else 1


if __name__ == "__main__":
    sys.exit(main())
