#!/usr/bin/env python3
"""Offline self-test of floor.py, the landing floor.

    floor-selftest.py

Needs `git` and `yq` on PATH and the checkout's full local history; nothing
else, and no network. It works in a temporary directory with its own HOME,
no system or global git config and a fixed identity, and runs `git init` on
a throwaway repository with no remote. Commits there are built with git
plumbing (`hash-object`, `update-index --index-info`, `write-tree`,
`commit-tree`), so case variants and binary blobs do not depend on the file
system. Base configs are `real`, the checkout's .mintswarm/config.yaml byte
for byte, and variants of it, read with `yq -o=json`, changed here and
committed as JSON text at the same path. floor.py runs as
`sys.executable -I floor.py ...` with that repository as its working
directory, plans and facts files written under `cases/` and TMPDIR an
empty directory of the sandbox, where the floor makes its object stores. Every run gets
a facts file for its own base and head: clean (no checks, a passed gate
round, a review `pass`, both judging the head, no steers, no pending
review request) unless the case
changes it.

Checks:

- config: the checkout's pipeline.landing has the expected values, and
  plan-approval/floor.py accepts the real config (exit 0 or 1, a verdict);
- verdict shape, on every case that exits 0 or 1: the verdict's keys in
  order, the 12 rule identifiers in order across `rules` and
  `not_evaluated`, results and reasons, `verdict` against the rules and the
  exit code, the plan's sha256, `facts` (path, the sha256 of the facts
  file and the checks it lists), the full `base`, `head` and `merge_base`
  commits, `mode` and the printed lines;
- clean cases, a dependency manifest declared only through
  new_dependencies, a path with a space and a non-ASCII character, and
  `mode: shadow`;
- single-rule hits: each diff rule, `size` at and one over its limit, and
  the require_human rules with case variants;
- protected: each fixed path, also under a config without require_human;
- footprint: an unusable footprint skips the three diff rules and still
  evaluates the other eight;
- facts rules, each clean and hit: `gates` (failed then passed is clean; no
  round, a last round failed, passed with a failed test, an unknown
  verdict, failed tests without a string name, a passed last round from
  the base commit), `checks` (zero checks and
  successes are clean and listed; a failure, a run not completed, a
  pending status, a neutral run, a name holding a newline, U+2028 and
  U+0085 that stays escaped on its printed line; on a base
  with .github/workflows, in either case, zero checks hits and a success
  is clean), `review` (changes required then pass is clean; no round, pass
  then changes required, a passing last round from another commit),
  stale earlier rounds followed by current passing ones are clean, `steers`
  (none is clean; one and two steers,
  named by author and URL), `review-requests` (none is clean; one and two
  re-requested, one reason per login), and all four of gates, checks, review and
  steers at once, also with an unusable footprint;
- submodules: a gitlink hidden by a worktree `.gitmodules` `ignore = all` is
  still a changed and added path, and every gitlink is unmeasurable for
  `size`; with hostile system, global, environment and repository config,
  the verdict does not change;
- history tampering: a replace ref on the head or the base, info/grafts,
  GIT_GRAFT_FILE, `git replace --graft`, GIT_REPLACE_REF_BASE and a
  commit-graph patched in its CDAT chunk to cut the link between the
  head's parent and the base (so a plain `git merge-base` finds none) leave
  the hits in place, and a head
  the base already contains is unusable;
- binary content: a blob with a NUL that an untracked .gitattributes,
  info/attributes, the XDG attributes file or core.attributesFile forces to
  text is still unmeasurable, on either side;
- protected single entries: a symlink or gitlink at .github/workflows or at
  a directory above a protected path is a `protected` hit without
  require_human;
- base ref: with a head commit and a worktree config that raise
  lines_factor, the base's limit still gives the `size` hit;
- unusable input: bad arguments (a missing or repeated --facts included),
  refs, history, configs and plans, a facts file that is missing, a
  directory, a symlink, not UTF-8, not JSON, has a duplicate key or NaN, is
  not an object, has the wrong tool or version (1 included), a short head,
  the head or base of another commit, mistyped or missing fields, a round
  `commit` that is not a hash, a `current` that is a string, a
  `review_requests` that is missing, not a list or holds a non-string, `current` true
  on a stale round or false on a current one, or rounds out of order, a
  missing `yq` and a missing plan-approval/floor.py exit 2 with a
  one-line message naming the cause and leave no verdict file, even a
  stale one; every case but those without it passes --facts for its own
  base and head, so it fails for its own reason;
- facts contract: collect-facts.py's TOOL and VERSION (3) are the ones
  floor.py reads, and facts written by the real `collect-facts.py
  assemble`, from saved responses and version 2 kept rounds made here, in a
  runner-state directory outside the repository, are read by
  the floor: clean ones, with a first review request pending, are clean,
  and a CTRF test without a name, a failed check run, a human comment
  after the approval and a request re-made after a change request give
  `gates` (an unnamed gate), `checks`, `steers` and `review-requests` hits;
- the plan floor's own verdict is left alone, and a copy of both scripts
  elsewhere gives the same verdict;
- historic replays: the squash commits bb7f812 and 4e7ba0a, rebuilt from the
  checkout's objects, against plan-approval/fixtures/plan-{20,19}.md give
  their recorded verdicts;
- glob semantics: require-human-paths reasons agree with
  plan-approval/floor.py's matching;
- object store, with every run's TMPDIR an empty sandbox directory: GIT_DIR
  and GIT_OBJECT_DIRECTORY at a decoy repository, where a plain git cannot
  see the base, leave the hits in place; a loose blob only the head adds,
  overwritten with another blob that a plain `git cat-file` then shows,
  makes the floor exit 2 with one line naming the fetch and no verdict, even
  a stale one; a .git/shallow forged in the job repository at the head's
  parent, cutting off the commit between it and the base, makes the floor
  exit 2 with one
  line naming the fetch and no verdict; and no `msw-floor-objects-*` store
  is left after clean, hit and exit 2 runs;
- standard library: every import of floor.py and floor-selftest.py is in the
  standard library.

Prints one `ok` or `FAIL` line per check, then a count, and exits 0 only
when all passed.
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
import unicodedata
import zlib
from pathlib import Path

HERE = Path(__file__).resolve().parent
FLOOR = HERE / "floor.py"
COLLECT_FACTS = HERE / "collect-facts.py"
PLAN_FLOOR = HERE.parent / "plan-approval" / "floor.py"
FIXTURES = HERE.parent / "plan-approval" / "fixtures"
ROOT = HERE.parents[2]
CONFIG = ROOT / ".mintswarm" / "config.yaml"
CONFIG_PATH = ".mintswarm/config.yaml"
VERDICT = Path(".mintswarm/job/outputs/landing-floor.json")
PLAN_VERDICT = Path(".mintswarm/job/outputs/floor.json")
HASH = re.compile(r"[0-9a-f]{40}")
GITLINK = "160000"
SYMLINK = "120000"
SUBMODULE = (GITLINK, "1234567890abcdef1234567890abcdef12345678")
GITMODULES_HIDDEN = b'[submodule "x"]\n\tpath = docs/sub\n\turl = ./nowhere\n\tignore = all\n'

RULES = [
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
    "review-requests",
]
DIFF_RULES = ["undeclared-files", "undeclared-new-files", "size"]
FACTS_RULES = ["gates", "checks", "review", "steers", "review-requests"]
VERDICT_KEYS = [
    "tool",
    "version",
    "plan",
    "facts",
    "component",
    "base",
    "head",
    "merge_base",
    "config",
    "mode",
    "verdict",
    "rules",
    "not_evaluated",
]
EXPECTED_LANDING = {
    "mode": "always",
    "require_human": {
        "paths": [".mintswarm/**", ".github/**", "AGENTS.md", "CLAUDE.md", ".claude/agents/**", ".claude/skills/**"]
    },
    "footprint": {"lines_factor": 1.5},
}
HISTORIC = {
    "bb7f812": ("plan-20.md", "docs", {}),
    "4e7ba0a": (
        "plan-19.md",
        "tooling",
        {
            "require-human-paths": [".github/CODEOWNERS matches require_human.paths .github/**"],
            "protected": [".github/CODEOWNERS is a CODEOWNERS location"],
        },
    ),
}
GLOB_PATTERNS = [
    "**",
    "docs/**",
    "**/x.md",
    "a/**/b",
    "a/**",
    "*.md",
    "docs/*.md",
    "?.md",
    "a?c",
    "a.b",
    "[ab].md",
    "a+b(c)",
    "README.md",
    ".mintswarm/dogfood/github/**",
    "crates/*/src/**",
]
GLOB_PATHS = [
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


def text(n: int, tag: str = "line") -> bytes:
    return "".join(f"{tag} {i}\n" for i in range(n)).encode("utf-8")


BASE_TREE = {
    "CONTRIBUTING.md": text(3, "contributing"),
    "README.md": text(2, "readme"),
    "docs/guide.md": text(4, "guide"),
    "docs/Guide.md": text(2, "other guide"),
    "docs/requirements.txt": text(1, "requirement"),
}


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
                "GIT_AUTHOR_NAME": "landing floor selftest",
                "GIT_AUTHOR_EMAIL": "selftest@example.invalid",
                "GIT_COMMITTER_NAME": "landing floor selftest",
                "GIT_COMMITTER_EMAIL": "selftest@example.invalid",
                "GIT_AUTHOR_DATE": "2026-01-01T00:00:00Z",
                "GIT_COMMITTER_DATE": "2026-01-01T00:00:00Z",
            }
        )
        self.store_tmp = tmp / "tmpdir"  # TMPDIR of every run: the floor's object stores go here
        self.store_tmp.mkdir()
        env["TMPDIR"] = str(self.store_tmp)
        self.env = (
            env  # floor.py runs with this; it must neutralise replace objects, grafts and the commit-graph itself
        )
        # the self-test's own git calls always see the true objects and history
        self.git_env = dict(
            env,
            GIT_NO_REPLACE_OBJECTS="1",
            GIT_GRAFT_FILE=os.devnull,
            GIT_CONFIG_COUNT="1",
            GIT_CONFIG_KEY_0="core.commitGraph",
            GIT_CONFIG_VALUE_0="false",
        )
        self.repo.mkdir()
        self.cases.mkdir()
        self.git("init", "-q", "-b", "master")
        self.counter = 0
        self.trees = {}

    def raw(self, *args: str, data: bytes = None, env: dict = None, cwd: Path = None) -> bytes:
        proc = subprocess.run(
            ["git", *args], cwd=cwd or self.repo, env=env or self.git_env, input=data, capture_output=True
        )
        if proc.returncode != 0:
            raise RuntimeError(f"git {' '.join(args)} failed: {proc.stderr.decode(errors='replace').strip()}")
        return proc.stdout

    def git(self, *args: str, data: bytes = None) -> str:
        return self.raw(*args, data=data).decode("utf-8").strip()

    def commit(self, files: dict, parents: tuple = (), message: str = "case") -> str:
        """Commit `files` ({path: bytes or (mode, bytes)}) as the whole tree.

        A gitlink is (GITLINK, "<commit hash>"); git does not need that commit.
        """
        self.counter += 1
        records = b""
        for path, content in files.items():
            mode, data = content if isinstance(content, tuple) else ("100644", content)
            sha = data if mode == GITLINK else self.git("hash-object", "-w", "--stdin", data=data)
            records += f"{mode} {sha}\t".encode("utf-8") + path.encode("utf-8") + b"\0"
        env = dict(self.git_env, GIT_INDEX_FILE=str(self.tmp / f"index-{self.counter}"))
        self.raw("update-index", "-z", "--index-info", data=records, env=env)
        tree = self.raw("write-tree", env=env).decode("ascii").strip()
        os.unlink(self.tmp / f"index-{self.counter}")
        argv = ["commit-tree", tree]
        for parent in parents:
            argv += ["-p", parent]
        commit = self.git(*argv, "-m", f"{message} ({self.counter})")
        self.trees[commit] = dict(files)
        return commit

    def child(self, parent: str, changes: dict, message: str = "head") -> str:
        files = dict(self.trees[parent])
        for path, content in changes.items():
            if content is None:
                files.pop(path)
            else:
                files[path] = content
        return self.commit(files, (parent,), message)

    def write_plan(self, name: str, content) -> str:
        self.counter += 1
        slug = re.sub(r"[^A-Za-z0-9.-]+", "-", name)[:40]
        rel = f"cases/{self.counter:03d}-{slug}.md"
        data = content.encode("utf-8") if isinstance(content, str) else content
        (self.repo / rel).write_bytes(data)
        return rel

    def write_facts(self, name: str, content) -> str:
        """Write a facts file (a dict as the collector would, or raw bytes) under cases/."""
        self.counter += 1
        slug = re.sub(r"[^A-Za-z0-9.-]+", "-", name)[:40]
        rel = f"cases/{self.counter:03d}-{slug}.facts.json"
        data = content if isinstance(content, bytes) else facts_bytes(content)
        (self.repo / rel).write_bytes(data)
        return rel

    def commit_of(self, ref: str):
        """The full commit `ref` resolves to, or None."""
        proc = subprocess.run(
            ["git", "rev-parse", "--verify", "--quiet", "--end-of-options", f"{ref}^{{commit}}"],
            cwd=self.repo,
            env=self.git_env,
            capture_output=True,
            text=True,
        )
        commit = proc.stdout.strip()
        return commit if proc.returncode == 0 and HASH.fullmatch(commit) else None

    def exec(self, args: list, env: dict = None, script: Path = FLOOR):
        return subprocess.run(
            [sys.executable, "-I", str(script), *args],
            cwd=self.repo,
            env=env or self.env,
            capture_output=True,
            text=True,
        )

    def floor(self, args: list, env: dict = None, script: Path = FLOOR):
        stale = self.repo / VERDICT
        stale.parent.mkdir(parents=True, exist_ok=True)
        stale.write_text('{"stale": true}\n', encoding="utf-8")
        proc = self.exec(args, env, script)
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


def load_module(name: str, path: Path):
    sys.dont_write_bytecode = True  # leave no __pycache__ next to either script
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


# --- plans ----------------------------------------------------------------


def footprint(files: list, new_files: list = (), estimated_lines: int = 50, new_dependencies: list = ()) -> dict:
    return {
        "files": list(files),
        "new_files": list(new_files),
        "estimated_lines": estimated_lines,
        "new_dependencies": list(new_dependencies),
        "external_steps": [],
        "gate_changes": [],
        "overridden_assumptions": [],
    }


def plan_text(fp: dict = None) -> str:
    section = ""
    if fp is not None:
        section = "## Footprint\n\n```json\n" + json.dumps({"footprint": fp}, indent=2, ensure_ascii=False) + "\n```\n"
    return (
        "# Plan: landing selftest case\n\nGenerated by floor-selftest.py.\n\n"
        "## Approach\n\nA generated case.\n\n"
        "## Steps\n\n1. **Change** — files: as declared.\n\n" + section
    )


def appended(path: str, n: int, tag: str = "more") -> dict:
    return {path: BASE_TREE[path] + text(n, tag)}


# --- facts ----------------------------------------------------------------


def gate_round(n: int, verdict: str = "passed", failed: list = (), reason: str = None, commit: str = None) -> dict:
    """A gate round; a missing commit and current are filled in for the facts' head (fill_rounds)."""
    return {
        "round": n,
        "commit": commit,
        "current": None,
        "verdict": verdict,
        "failed_tests": list(failed),
        "reason": reason,
        "files": [],
    }


def review_round(n: int, verdict: str = "pass", commit: str = None) -> dict:
    return {
        "round": n,
        "commit": commit,
        "current": None,
        "verdict": verdict,
        "findings": {"blocker": 0, "major": 0, "minor": 0, "nit": 0},
        "files": [],
    }


def fill_rounds(content: dict) -> dict:
    """Give each round without a commit the head, and without current whether its commit is the head."""
    for key in ("gate_rounds", "review_rounds"):
        rounds = content.get(key)
        for entry in rounds if isinstance(rounds, list) else []:
            if isinstance(entry, dict):
                if entry.get("commit") is None:
                    entry["commit"] = content["head"]
                if entry.get("current") is None:
                    entry["current"] = entry["commit"] == content["head"]
    return content


def check_entry(name: str, conclusion, source: str = "check-run", app="github-actions") -> dict:
    return {"name": name, "conclusion": conclusion, "source": source, "app": app}


def steer(author: str, url: str, kind: str = "pr-comment", time: str = "2026-01-02T00:00:00Z") -> dict:
    return {"kind": kind, "author": author, "time": time, "edited_at": None, "url": url, "via": "account"}


def clean_facts(base: str, head: str) -> dict:
    """A clean facts file in collect-facts.py's key order: no checks, a passed gate round, a review pass."""
    return fill_rounds(
        {
            "tool": "mintswarm-landing-facts",
            "version": 3,
            "pr": 1,
            "issue": 1,
            "head": head,
            "base": base,
            "fetched_at": "2026-01-01T00:00:00Z",
            "checks": [],
            "gate_rounds": [gate_round(1)],
            "review_rounds": [review_round(1)],
            "steers": [],
            "review_requests": [],
            "approval": {"approver": "maintainer", "approved_at": "2026-01-01T00:00:00Z", "approval_ref": None},
        }
    )


def facts_bytes(facts) -> bytes:
    return (json.dumps(facts, indent=2, ensure_ascii=False) + "\n").encode("utf-8")


def facts_for(base: str, head: str, facts=None):
    """Clean facts with `facts` applied: None, top-level overrides, a callable over the clean facts, or bytes."""
    if isinstance(facts, bytes):
        return facts
    content = clean_facts(base, head)
    if callable(facts):
        content = facts(content)
    elif facts is not None:
        content.update(facts)
    return fill_rounds(content)


# --- verdict checks -------------------------------------------------------


def printable(text: str) -> str:
    """A reason as floor.py prints it: control characters and line or paragraph separators as JSON escapes."""
    return "".join(json.dumps(c)[1:-1] if unicodedata.category(c) in ("Cc", "Zl", "Zp") else c for c in text)


def in_order(names: list, order: list) -> bool:
    return [n for n in order if n in names] == list(names)


def check_shape(name: str, proc, verdict, sb: Sandbox, args: dict, mode: str) -> bool:
    problems = []
    if not isinstance(verdict, dict):
        return T.check(
            f"{name}: verdict shape", False, f"no verdict file (exit {proc.returncode}): {proc.stderr.strip()}"
        )
    if list(verdict) != VERDICT_KEYS:
        problems.append(f"keys {list(verdict)}")
    rules = verdict.get("rules", [])
    skipped = list(verdict.get("not_evaluated", []))
    evaluated = [r.get("rule") for r in rules]
    if not in_order(evaluated, RULES) or not in_order(skipped, RULES) or sorted(evaluated + skipped) != sorted(RULES):
        problems.append(f"rule identifiers {evaluated} + {skipped} are not the {len(RULES)} in order")
    if skipped not in ([], DIFF_RULES):
        problems.append(f"not_evaluated {skipped}")
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
    if verdict.get("tool") != "mintswarm-landing-floor" or verdict.get("version") != 1:
        problems.append("tool or version")
    plan_bytes = (sb.repo / args["plan"]).read_bytes()
    if verdict.get("plan") != {"path": args["plan"], "sha256": hashlib.sha256(plan_bytes).hexdigest()}:
        problems.append(f"plan {verdict.get('plan')}")
    facts_data = (sb.repo / args["facts"]).read_bytes()
    try:
        listed = json.loads(facts_data)["checks"]
        expected_facts = {
            "path": args["facts"],
            "sha256": hashlib.sha256(facts_data).hexdigest(),
            "checks": [{k: c[k] for k in ("name", "conclusion", "source", "app")} for c in listed],
        }
    except (ValueError, KeyError, TypeError) as exc:
        expected_facts = f"a facts file the floor should have refused: {exc}"
    if verdict.get("facts") != expected_facts:
        problems.append(f"facts {verdict.get('facts')}, expected {expected_facts}")
    if verdict.get("component") != args["component"]:
        problems.append("component")
    base = sb.git("rev-parse", f"{args['base']}^{{commit}}")
    head = sb.git("rev-parse", f"{args['head']}^{{commit}}")
    merge = sb.git("merge-base", base, head)
    for key, ref, commit in (("base", args["base"], base), ("head", args["head"], head)):
        if verdict.get(key) != {"ref": ref, "commit": commit} or not HASH.fullmatch(commit):
            problems.append(f"{key} {verdict.get(key)}, expected commit {commit}")
    if verdict.get("merge_base") != merge or not HASH.fullmatch(str(verdict.get("merge_base"))):
        problems.append(f"merge_base {verdict.get('merge_base')}, expected {merge}")
    if verdict.get("config") != CONFIG_PATH:
        problems.append("config")
    if verdict.get("mode") != mode:
        problems.append(f"mode {verdict.get('mode')!r}, expected {mode!r}")
    lines = proc.stdout.splitlines()
    expected = []
    for r in rules:
        line = f"{r.get('result')!s:7} {r.get('rule')}"
        if r.get("reasons"):
            line += f": {printable(r['reasons'][0])}"
        expected.append(line)
    expected.append(f"floor: {verdict.get('verdict')}")
    if lines != expected:
        problems.append(f"stdout {lines!r}")
    return T.check(f"{name}: verdict shape", not problems, "; ".join(problems))


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


def hits_of(verdict) -> list:
    return [rule for rule, result in results_of(verdict).items() if result == "hit"]


class Runner:
    def __init__(self, sb: Sandbox, commits: dict, modes: dict):
        self.sb = sb
        self.commits = commits
        self.modes = modes

    def run(
        self,
        name: str,
        content,
        changes: dict = None,
        component: str = "docs",
        config: str = "real",
        base: str = None,
        head: str = None,
        mode: str = None,
        env: dict = None,
        facts=None,
    ):
        plan = self.sb.write_plan(name, content)
        base = base or self.commits[config]
        head = head or self.sb.child(self.sb.git("rev-parse", f"{base}^{{commit}}"), changes or {})
        facts_path = self.sb.write_facts(name, facts_for(self.sb.commit_of(base), self.sb.commit_of(head), facts))
        args = {"plan": plan, "component": component, "base": base, "head": head, "facts": facts_path}
        proc, verdict = self.sb.floor(
            ["--plan", plan, "--component", component, "--base", base, "--head", head, "--facts", facts_path], env=env
        )
        check_shape(name, proc, verdict, self.sb, args, mode or self.modes[config])
        return proc, verdict

    def expect(self, name: str, fp, changes: dict, hits: dict, **kwargs):
        """Run a case; `hits` maps each rule that must hit, and no other, to a reason it must give."""
        content = fp if isinstance(fp, str) else plan_text(fp)
        proc, verdict = self.run(name, content, changes, **kwargs)
        got = hits_of(verdict)
        problems = []
        if proc.returncode != (1 if hits else 0):
            problems.append(f"exit {proc.returncode}: {proc.stderr.strip()}")
        if sorted(got) != sorted(hits):
            problems.append(f"hits {got}, expected {sorted(hits)}")
        for rule, reason in hits.items():
            if reason is not None and reason not in reasons_of(verdict, rule):
                problems.append(f"{rule} reasons {reasons_of(verdict, rule)} lack {reason!r}")
        label = ", ".join(f"{rule} hit" for rule in hits) or "clean"
        T.check(f"{name}: {label}", not problems, "; ".join(problems))
        return proc, verdict


# --- check groups ---------------------------------------------------------


def check_config(sb: Sandbox, commits: dict) -> None:
    config = yq_json(CONFIG)
    landing = (config.get("pipeline") or {}).get("landing")
    T.check("config: pipeline.landing values", landing == EXPECTED_LANDING, json.dumps(landing))
    plan = sb.write_plan("plan floor on the real config", (FIXTURES / "plan-20.md").read_bytes())
    target = sb.repo / PLAN_VERDICT
    target.parent.mkdir(parents=True, exist_ok=True)
    target.unlink(missing_ok=True)
    proc = sb.exec(["--plan", plan, "--component", "docs", "--base", commits["real"]], script=PLAN_FLOOR)
    T.check(
        "config: plan-approval/floor.py accepts the real config",
        proc.returncode in (0, 1) and target.is_file(),
        f"exit {proc.returncode}: {proc.stderr.strip()}",
    )


def check_clean(r: Runner) -> None:
    r.expect("clean change", footprint(["docs/guide.md"], estimated_lines=4), appended("docs/guide.md", 6), {})
    r.expect(
        "manifest through new_dependencies",
        footprint(
            ["docs/guide.md"],
            estimated_lines=4,
            new_dependencies=[{"name": "dep", "manifest": "docs/requirements.txt"}],
        ),
        {**appended("docs/guide.md", 2), **appended("docs/requirements.txt", 1)},
        {},
    )
    odd = "docs/sp ace é.md"
    r.expect("path with a space and non-ASCII", footprint([odd], [odd], estimated_lines=2), {odd: text(2)}, {})
    r.expect("deleted declared file", footprint(["README.md"], estimated_lines=2), {"README.md": None}, {})
    moved = {"docs/guide.md": None, "docs/moved.md": BASE_TREE["docs/guide.md"]}
    r.expect(
        "rename declared as both paths",
        footprint(["docs/guide.md", "docs/moved.md"], ["docs/moved.md"], estimated_lines=8),
        moved,
        {},
    )
    r.expect(
        "shadow mode",
        footprint(["docs/guide.md"], estimated_lines=4),
        appended("docs/guide.md", 2),
        {},
        config="shadow",
    )
    for config in ("min-confidence-90", "min-confidence-100"):
        r.expect(
            f"pipeline.landing.min_confidence {config.rsplit('-', 1)[1]}",
            footprint(["docs/guide.md"], estimated_lines=4),
            appended("docs/guide.md", 2),
            {},
            config=config,
        )


def check_single_rules(r: Runner) -> None:
    fp = footprint(["docs/guide.md"], estimated_lines=10)
    r.expect(
        "undeclared path",
        fp,
        {**appended("docs/guide.md", 1), **appended("CONTRIBUTING.md", 1)},
        {"undeclared-files": "CONTRIBUTING.md is changed but is neither in files nor a new dependency's manifest"},
    )
    r.expect(
        "case-only variant of a declared path",
        fp,
        appended("docs/Guide.md", 1),
        {"undeclared-files": "docs/Guide.md is changed but is neither in files nor a new dependency's manifest"},
    )
    r.expect(
        "undeclared deletion",
        fp,
        {"README.md": None},
        {"undeclared-files": "README.md is changed but is neither in files nor a new dependency's manifest"},
    )
    r.expect(
        "rename without its old path",
        footprint(["docs/moved.md"], ["docs/moved.md"], estimated_lines=8),
        {"docs/guide.md": None, "docs/moved.md": BASE_TREE["docs/guide.md"]},
        {"undeclared-files": "docs/guide.md is changed but is neither in files nor a new dependency's manifest"},
    )
    r.expect(
        "undeclared added file",
        footprint(["docs/guide.md", "docs/new.md"], estimated_lines=10),
        {"docs/new.md": text(1)},
        {"undeclared-new-files": "docs/new.md is added but is not in new_files"},
    )

    # factor 1.5, estimated_lines 7: floor(10.5) = 10
    r.expect(
        "size at floor(7 × 1.5)", footprint(["docs/guide.md"], estimated_lines=7), appended("docs/guide.md", 10), {}
    )
    r.expect(
        "size one over floor(7 × 1.5)",
        footprint(["docs/guide.md"], estimated_lines=7),
        appended("docs/guide.md", 11),
        {"size": "the diff counts 11 lines, over the limit of 10 (estimated_lines 7 × lines_factor 1.5)"},
    )
    # factor 1.15, estimated_lines 100: exactly 115, where binary floats give 114
    r.expect(
        "size at 100 × 1.15",
        footprint(["docs/guide.md"], estimated_lines=100),
        appended("docs/guide.md", 115),
        {},
        config="factor-1.15",
    )
    r.expect(
        "size one over 100 × 1.15",
        footprint(["docs/guide.md"], estimated_lines=100),
        appended("docs/guide.md", 116),
        {"size": "the diff counts 116 lines, over the limit of 115 (estimated_lines 100 × lines_factor 1.15)"},
        config="factor-1.15",
    )
    r.expect(
        "binary file",
        footprint(["docs/logo.png"], ["docs/logo.png"], estimated_lines=100),
        {"docs/logo.png": b"\x89PNG\r\n\x1a\n\x00\x00\x00\x00binary\x00"},
        {"size": "docs/logo.png is binary or otherwise unmeasurable"},
    )

    r.expect(
        "CLAUDE.md",
        footprint(["CLAUDE.md"], ["CLAUDE.md"], estimated_lines=2),
        {"CLAUDE.md": text(1)},
        {"require-human-paths": "CLAUDE.md matches require_human.paths CLAUDE.md"},
    )
    r.expect(
        "agents.md case variant",
        footprint(["agents.md"], ["agents.md"], estimated_lines=2),
        {"agents.md": text(1)},
        {"require-human-paths": "agents.md matches require_human.paths AGENTS.md"},
    )
    r.expect(
        ".GitHub/dependabot.yml case variant",
        footprint([".GitHub/dependabot.yml"], [".GitHub/dependabot.yml"], estimated_lines=2),
        {".GitHub/dependabot.yml": text(1)},
        {"require-human-paths": ".GitHub/dependabot.yml matches require_human.paths .github/**"},
    )

    r.expect(
        "path in a listed component",
        footprint(["web/index.html"], ["web/index.html"], estimated_lines=2),
        {"web/index.html": text(1)},
        {"require-human-components": "web/index.html is in component web (web/**), which requires a human"},
        config="rh-web",
    )
    r.expect(
        "case variant in a listed component",
        footprint(["Web/Index.html"], ["Web/Index.html"], estimated_lines=2),
        {"Web/Index.html": text(1)},
        {"require-human-components": "Web/Index.html is in component web (web/**), which requires a human"},
        config="rh-web",
    )
    r.expect(
        "the issue's own component",
        footprint(["docs/guide.md"], estimated_lines=4),
        appended("docs/guide.md", 1),
        {"require-human-components": "the issue's component web is in require_human.components"},
        component="web",
        config="rh-web",
    )


def check_protected(r: Runner, commits: dict) -> None:
    cases = [
        (CONFIG_PATH, "holds the gates and the landing policy", False),
        (".mintswarm/dogfood/render-agents.py", "is named by gate agents-drift", True),
        ("CODEOWNERS", "is a CODEOWNERS location", True),
        (".github/CODEOWNERS", "is a CODEOWNERS location", True),
        ("docs/CODEOWNERS", "is a CODEOWNERS location", True),
        (".github/workflows/ci.yml", "is under .github/workflows/ (.github/workflows/**)", True),
        (".GitHub/Workflows/CI.yml", "is under .github/workflows/ (.github/workflows/**)", True),
        ("docs/CodeOwners", "is a CODEOWNERS location", True),
        (".MintSwarm/Config.yaml", "holds the gates and the landing policy", True),
    ]
    for config in ("real", "no-rh"):
        for path, reason, new in cases:
            if new:
                fp = footprint([path], [path], estimated_lines=5)
                changes = {path: text(1)}
            else:
                fp = footprint([path], estimated_lines=5)
                changes = {path: r.sb.trees[commits[config]][path] + b"\n"}
            hits = {"protected": f"{path} {reason}"}
            if config == "real" and path.lower().startswith((".mintswarm/", ".github/")):
                hits["require-human-paths"] = None
            r.expect(f"protected {path} under {config}", fp, changes, hits, config=config)


def check_footprint_hit(r: Runner) -> None:
    proc, verdict = r.run("no footprint", plan_text(None), {**appended("CONTRIBUTING.md", 1), "CLAUDE.md": text(1)})
    ok = (
        proc.returncode == 1
        and isinstance(verdict, dict)
        and [x["rule"] for x in verdict["rules"]]
        == ["footprint", "require-human-paths", "require-human-components", "protected", *FACTS_RULES]
        and verdict["not_evaluated"] == DIFF_RULES
        and results_of(verdict)
        == {
            "footprint": "hit",
            "require-human-paths": "hit",
            "require-human-components": "clean",
            "protected": "clean",
            **{rule: "clean" for rule in FACTS_RULES},
        }
        and "no '## Footprint' section" in reasons_of(verdict, "footprint")
    )
    T.check(
        "no footprint: diff rules not evaluated, the other eight are",
        ok,
        f"exit {proc.returncode}: {proc.stdout.strip()} {proc.stderr.strip()}",
    )


def check_base_ref(r: Runner, sb: Sandbox, configs: dict) -> None:
    loose = copy.deepcopy(configs["factor-1"])
    loose["pipeline"]["landing"]["footprint"]["lines_factor"] = 100
    worktree = sb.repo / CONFIG_PATH
    worktree.parent.mkdir(parents=True, exist_ok=True)
    worktree.write_bytes(as_json(loose))
    fp = footprint([CONFIG_PATH, "docs/guide.md"], estimated_lines=10)
    base = r.commits["factor-1"]
    sb.git("tag", "landing-base", base)
    first = sb.child(base, {CONFIG_PATH: as_json(loose), **appended("docs/guide.md", 12)}, "raise lines_factor")
    second = sb.child(first, {"docs/guide.md": sb.trees[first]["docs/guide.md"] + text(12, "again")}, "more")
    reason = "the diff counts 14 lines, over the limit of 10 (estimated_lines 10 × lines_factor 1)"
    r.expect(
        "base ref: head and worktree raise lines_factor",
        fp,
        None,
        {"size": reason, "protected": None, "require-human-paths": None},
        base="landing-base",
        head=first,
        config="factor-1",
    )
    r.expect(
        "base ref: a later head still under the base's limit",
        fp,
        None,
        {
            "size": "the diff counts 26 lines, over the limit of 10 (estimated_lines 10 × lines_factor 1)",
            "protected": None,
            "require-human-paths": None,
        },
        base="landing-base",
        head=second,
        config="factor-1",
    )
    r.expect(
        "base ref: --base at the raised commit",
        footprint(["docs/guide.md"], estimated_lines=10),
        None,
        {},
        base=first,
        head=second,
        config="factor-1",
    )


def check_submodules(r: Runner, sb: Sandbox) -> None:
    base = r.commits["real"]
    gitmodules = sb.repo / ".gitmodules"
    gitmodules.write_bytes(GITMODULES_HIDDEN)  # git reads it from the worktree, which the branch controls
    hidden = sb.child(
        base,
        {".gitmodules": GITMODULES_HIDDEN, "docs/sub": SUBMODULE, **appended("docs/guide.md", 1)},
        "hidden submodule",
    )
    plain = sb.git("diff", "--numstat", "--no-renames", f"{base}...{hidden}")
    T.check("submodule: a plain git diff hides the gitlink under ignore = all", "docs/sub" not in plain, plain)
    gitlink_reason = "docs/sub is a submodule (gitlink), whose change is not measured in lines"
    r.expect(
        "undeclared submodule hidden by .gitmodules",
        footprint(["docs/guide.md", ".gitmodules"], [".gitmodules"], estimated_lines=10),
        None,
        {
            "undeclared-files": "docs/sub is changed but is neither in files nor a new dependency's manifest",
            "undeclared-new-files": "docs/sub is added but is not in new_files",
            "size": gitlink_reason,
        },
        head=hidden,
    )
    declared = footprint(["docs/guide.md", ".gitmodules", "docs/sub"], [".gitmodules", "docs/sub"], estimated_lines=10)
    r.expect("declared submodule: unmeasurable", declared, None, {"size": gitlink_reason}, head=hidden)
    r.expect(
        "file replaced by a submodule",
        footprint(["docs/guide.md"], estimated_lines=10),
        {"docs/guide.md": SUBMODULE},
        {"size": "docs/guide.md is a submodule (gitlink), whose change is not measured in lines"},
    )

    attributes = sb.tmp / "hostile-attributes"
    attributes.write_text("*.md binary\n", encoding="utf-8")
    hostile = sb.tmp / "hostile-gitconfig"
    hostile.write_text(
        "[diff]\n\tignoreSubmodules = all\n\talgorithm = patience\n\texternal = false\n"
        f"[core]\n\tattributesFile = {attributes}\n",
        encoding="utf-8",
    )
    env = {k: v for k, v in sb.env.items() if k != "GIT_CONFIG_NOSYSTEM"}
    env.update(
        {
            "GIT_CONFIG_GLOBAL": str(hostile),
            "GIT_CONFIG_PARAMETERS": "'diff.ignoreSubmodules'='all'",
            "GIT_CONFIG_COUNT": "1",
            "GIT_CONFIG_KEY_0": "core.attributesFile",
            "GIT_CONFIG_VALUE_0": str(attributes),
            "GIT_EXTERNAL_DIFF": "false",
        }
    )
    sb.git("config", "diff.ignoreSubmodules", "all")
    sb.git("config", "submodule.x.ignore", "all")
    try:
        _, verdict = r.expect(
            "hostile config: system, global and environment config ignored",
            declared,
            None,
            {"size": gitlink_reason},
            head=hidden,
            env=env,
        )
        T.check(
            "hostile config: only the gitlink is unmeasurable, no *.md is binary",
            reasons_of(verdict, "size") == [gitlink_reason],
            str(reasons_of(verdict, "size")),
        )
    finally:
        sb.git("config", "--unset", "diff.ignoreSubmodules")
        sb.git("config", "--unset", "submodule.x.ignore")
        gitmodules.unlink()


def check_history_tampering(r: Runner, sb: Sandbox) -> None:
    """Replace objects and grafts in the repository or the environment must not change the verdict."""
    base = r.commits["real"]
    decoy = sb.child(base, appended("docs/guide.md", 1), "decoy")
    head = sb.child(base, {**appended("docs/guide.md", 1), ".github/workflows/x.yml": text(1)}, "tampered head")
    fp = footprint(["docs/guide.md"], estimated_lines=10)
    hits = {
        "undeclared-files": ".github/workflows/x.yml is changed but is neither in files nor a new dependency's manifest",
        "undeclared-new-files": ".github/workflows/x.yml is added but is not in new_files",
        "require-human-paths": ".github/workflows/x.yml matches require_human.paths .github/**",
        "protected": ".github/workflows/x.yml is under .github/workflows/ (.github/workflows/**)",
    }
    grafts = sb.repo / ".git" / "info" / "grafts"
    graft_file = sb.tmp / "grafts"
    graft_file.write_text(f"{base} {head}\n", encoding="ascii")

    def plain(*args: str, env: dict = None) -> str:
        return sb.raw(*args, env=env or sb.env).decode("utf-8").strip()

    def tamper_replace():
        sb.git("update-ref", f"refs/replace/{head}", decoy)
        return lambda: sb.git("update-ref", "-d", f"refs/replace/{head}")

    def tamper_graft_file():
        grafts.parent.mkdir(parents=True, exist_ok=True)
        grafts.write_text(f"{base} {head}\n", encoding="ascii")
        return grafts.unlink

    def tamper_replace_graft():
        sb.git("replace", "--graft", base, head)
        return lambda: sb.git("update-ref", "-d", f"refs/replace/{base}")

    def tamper_ref_base():
        sb.git("update-ref", f"refs/elsewhere/{head}", decoy)
        return lambda: sb.git("update-ref", "-d", f"refs/elsewhere/{head}")

    cases = [
        ("git replace on the head", tamper_replace, {}, "diff"),
        ("info/grafts onto the head", tamper_graft_file, {}, "merge-base"),
        ("GIT_GRAFT_FILE onto the head", lambda: (lambda: None), {"GIT_GRAFT_FILE": str(graft_file)}, "merge-base"),
        ("git replace --graft onto the head", tamper_replace_graft, {}, "merge-base"),
        ("GIT_REPLACE_REF_BASE on the head", tamper_ref_base, {"GIT_REPLACE_REF_BASE": "refs/elsewhere/"}, "diff"),
    ]
    for name, tamper, extra, effect in cases:
        undo = tamper()
        env = dict(sb.env, **extra)
        try:
            if effect == "diff":
                seen = plain("diff", "--name-only", "--no-renames", f"{base}...{head}", env=env)
                T.check(f"{name}: a plain git diff sees the decoy", seen == "docs/guide.md", seen)
            else:
                seen = plain("merge-base", base, head, env=env)
                T.check(f"{name}: a plain git merge-base is the head", seen == head, seen)
            r.expect(f"{name}: still hit", fp, None, hits, head=head, env=env)
        finally:
            undo()

    # git parses the commits it is given from their objects, and their ancestors from the commit-graph, so the
    # graph cuts the link between the head's parent and the base
    middle = sb.child(base, appended("docs/guide.md", 1), "middle")
    far_head = sb.child(middle, {".github/workflows/x.yml": text(1)}, "far head")
    sb.git("update-ref", "refs/selftest/graph", far_head)
    sb.raw("commit-graph", "write", "--reachable", "--no-progress", env=sb.env)
    cut_parents_in_graph(sb.repo / ".git", middle)
    try:
        proc = subprocess.run(["git", "merge-base", base, far_head], cwd=sb.repo, env=sb.env, capture_output=True)
        T.check(
            "a commit-graph cutting the head's history: a plain git merge-base finds none",
            proc.returncode != 0,
            proc.stdout.decode(),
        )
        r.expect("a commit-graph cutting the head's history: still hit", fp, None, hits, head=far_head)
    finally:
        sb.git("update-ref", "-d", "refs/selftest/graph")
        (sb.repo / ".git" / "objects" / "info" / "commit-graph").unlink()

    # a replaced base would read a looser config at the base
    loose_base = sb.commit({**BASE_TREE, CONFIG_PATH: CONFIG.read_bytes()}, (), "base to be replaced")
    loose_head = sb.child(loose_base, {"CLAUDE.md": text(1)})
    sb.git("update-ref", f"refs/replace/{loose_base}", r.commits["no-rh"])
    try:
        r.expect(
            "git replace on the base: the real config still applies",
            footprint(["CLAUDE.md"], ["CLAUDE.md"], estimated_lines=5),
            None,
            {"require-human-paths": "CLAUDE.md matches require_human.paths CLAUDE.md"},
            base=loose_base,
            head=loose_head,
        )
    finally:
        sb.git("update-ref", "-d", f"refs/replace/{loose_base}")


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


def stores_left(sb: Sandbox) -> list:
    return sorted(p.name for p in sb.store_tmp.iterdir() if p.name.startswith(STORE_PREFIX))


def check_object_store(r: Runner, sb: Sandbox) -> None:
    """The landing floor reads a fresh, re-hashed object store, never the job repository's objects."""
    base = r.commits["real"]
    fp = footprint(["docs/guide.md", "docs/added.md"], ["docs/added.md"], estimated_lines=10)
    r.expect("object store: clean run", fp, {**appended("docs/guide.md", 1), "docs/added.md": text(2, "added")}, {})
    T.check("object store: no store is left after a clean run", not stores_left(sb), str(stores_left(sb)))

    decoy = sb.tmp / "decoy"
    sb.raw("init", "-q", str(decoy), cwd=sb.tmp)
    decoy_env = dict(sb.env, GIT_DIR=str(decoy / ".git"), GIT_OBJECT_DIRECTORY=str(decoy / ".git" / "objects"))
    seen = subprocess.run(
        ["git", "rev-parse", "--verify", "--quiet", f"{base}^{{commit}}"],
        cwd=sb.repo,
        env=decoy_env,
        capture_output=True,
    )
    T.check("object store, GIT_DIR at a decoy: plain git does not see the base", seen.returncode != 0)
    r.expect(
        "object store, GIT_DIR and GIT_OBJECT_DIRECTORY at a decoy",
        footprint(["docs/guide.md"], estimated_lines=10),
        {**appended("docs/guide.md", 1), ".github/workflows/x.yml": text(1)},
        {
            "undeclared-files": None,
            "undeclared-new-files": None,
            "require-human-paths": None,
            "protected": None,
        },
        env=decoy_env,
    )
    T.check("object store: no store is left after a hit run", not stores_left(sb), str(stores_left(sb)))

    # a blob only the head adds, overwritten with another blob
    content = text(3, "only in the head")
    head = sb.child(base, {"docs/forged.md": content})
    blob = sb.git("rev-parse", f"{head}:docs/forged.md")
    other = text(1, "forged")
    path = sb.repo / ".git" / "objects" / blob[:2] / blob[2:]
    original = path.read_bytes()
    plan = sb.write_plan("forged object", plan_text(footprint(["docs/forged.md"], ["docs/forged.md"], 10)))
    facts = sb.write_facts("forged object", facts_for(base, head))
    argv = ["--plan", plan, "--component", "docs", "--base", base, "--head", head, "--facts", facts]
    path.chmod(0o644)
    path.write_bytes(zlib.compress(b"blob %d\0" % len(other) + other))
    try:
        T.check("object store, forged object: plain git shows the forgery", sb.raw("cat-file", "blob", blob) == other)
        proc, verdict = sb.floor(argv)
        lines = proc.stderr.strip().splitlines()
        T.check(
            "object store, forged object: exit 2 naming the fetch, no verdict",
            proc.returncode == 2 and verdict is None and len(lines) == 1 and "fetch" in lines[0],
            f"exit {proc.returncode}, verdict {verdict!r}: {proc.stderr.strip()}",
        )
    finally:
        path.write_bytes(original)
        path.chmod(0o444)
    T.check("object store, forged object: restored", sb.raw("cat-file", "blob", blob) == content)
    T.check("object store: no store is left after an exit 2 run", not stores_left(sb), str(stores_left(sb)))

    # a .git/shallow at the head's parent, cutting off the commit between it and the base: upload-pack sends
    # the cut history, and the store is not marked shallow
    between = sb.child(base, appended("docs/guide.md", 1), "shallow between")
    middle = sb.child(between, appended("docs/guide.md", 2), "shallow middle")
    far_head = sb.child(middle, {"docs/added.md": text(2, "added")}, "shallow head")
    plan = sb.write_plan("forged shallow", plan_text(fp))
    facts = sb.write_facts("forged shallow", facts_for(base, far_head))
    argv = ["--plan", plan, "--component", "docs", "--base", base, "--head", far_head, "--facts", facts]
    shallow = sb.repo / ".git" / "shallow"
    shallow.write_text(f"{middle}\n", encoding="ascii")
    try:
        T.check(
            "object store, forged .git/shallow: plain git sees a shallow repository",
            sb.raw("rev-parse", "--is-shallow-repository").decode().strip() == "true",
        )
        proc, verdict = sb.floor(argv)
        lines = proc.stderr.strip().splitlines()
        T.check(
            "object store, forged .git/shallow: exit 2 naming the fetch, no verdict",
            proc.returncode == 2 and verdict is None and len(lines) == 1 and "the fetch" in lines[0],
            f"exit {proc.returncode}, verdict {verdict!r}: {proc.stderr.strip()}",
        )
    finally:
        shallow.unlink()
    T.check("object store: no store is left after a shallow run", not stores_left(sb), str(stores_left(sb)))


def check_binary_content(r: Runner, sb: Sandbox) -> None:
    """Attributes from any source must not make a binary blob count as text."""
    blob = b"x" * 7990 + b"\0" * 20000 + b"\n"  # the first NUL is just inside git's 8000-byte probe
    path = "docs/x.bin"
    line = f"{path} diff\n"
    worktree = sb.repo / ".gitattributes"
    info = sb.repo / ".git" / "info" / "attributes"
    xdg = sb.tmp / "xdg"
    attributes_file = sb.tmp / "repo-attributes"
    env_xdg = dict(sb.env, XDG_CONFIG_HOME=str(xdg))

    def write(target: Path) -> callable:
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(line, encoding="utf-8")
        return target.unlink

    def core_attributes_file() -> callable:
        attributes_file.write_text(line, encoding="utf-8")
        sb.git("config", "core.attributesFile", str(attributes_file))
        return lambda: (sb.git("config", "--unset", "core.attributesFile"), attributes_file.unlink())

    sources = [
        ("an untracked worktree .gitattributes", lambda: write(worktree), sb.env),
        ("info/attributes", lambda: write(info), sb.env),
        ("$XDG_CONFIG_HOME/git/attributes", lambda: write(xdg / "git" / "attributes"), env_xdg),
        ("core.attributesFile", core_attributes_file, sb.env),
    ]
    added = sb.child(r.commits["real"], {path: blob}, "binary added")
    was_binary = sb.child(r.commits["real"], {path: blob}, "binary at the base")
    to_text = sb.child(was_binary, {path: text(2)}, "binary becomes text")
    reason = f"{path} is binary or otherwise unmeasurable"
    for name, source, env in sources:
        undo = source()
        try:
            counted = sb.raw("diff", "--numstat", "--no-renames", f"{r.commits['real']}...{added}", env=env).decode(
                "utf-8"
            )
            T.check(
                f"binary forced to text by {name}: a plain numstat counts it",
                not counted.startswith("-"),
                counted.strip(),
            )
            r.expect(
                f"binary forced to text by {name}: still unmeasurable",
                footprint([path], [path], estimated_lines=10),
                None,
                {"size": reason},
                head=added,
                env=env,
            )
            r.expect(
                f"binary at the base forced to text by {name}: still unmeasurable",
                footprint([path], estimated_lines=10),
                None,
                {"size": reason},
                base=was_binary,
                head=to_text,
                env=env,
            )
        finally:
            undo()


def check_protected_entries(r: Runner) -> None:
    """A single entry (symlink or gitlink) standing in for a protected directory, under no require_human."""
    base = r.commits["no-rh"]
    docs = [p for p in r.sb.trees[base] if p.startswith("docs/")]
    cases = [
        (
            ".github/workflows",
            (SYMLINK, b"../elsewhere"),
            {},
            ".github/workflows is the protected directory itself, changed as a single entry",
        ),
        (
            ".GitHub/Workflows",
            (SYMLINK, b"../elsewhere"),
            {},
            ".GitHub/Workflows is the protected directory itself, changed as a single entry",
        ),
        (
            ".github",
            SUBMODULE,
            {"size": ".github is a submodule (gitlink), whose change is not measured in lines"},
            ".github is a single entry standing in for a directory above the protected .github/codeowners",
        ),
        (
            ".mintswarm/dogfood",
            (SYMLINK, b"../elsewhere"),
            {},
            ".mintswarm/dogfood is a single entry standing in for a directory above the protected "
            ".mintswarm/dogfood/render-agents.py",
        ),
        (
            "docs",
            (SYMLINK, b"elsewhere"),
            {},
            "docs is a single entry standing in for a directory above the protected docs/codeowners",
        ),
    ]
    for path, content, extra, reason in cases:
        changes = {path: content}
        files = [path]
        if path == "docs":
            changes.update({p: None for p in docs})
            files += docs
        r.expect(
            f"protected single entry {path} under no-rh",
            footprint(files, [path], estimated_lines=50),
            changes,
            {"protected": reason, **extra},
            config="no-rh",
        )


def check_unusable(sb: Sandbox, commits: dict) -> None:
    good = sb.write_plan("good", plan_text(footprint(["docs/guide.md"], estimated_lines=10)))
    not_utf8 = sb.write_plan("not utf-8", b"# Plan\n\n\xff\xfe\n")
    real = commits["real"]
    head = sb.child(real, appended("docs/guide.md", 1))
    orphan = sb.commit({**BASE_TREE, CONFIG_PATH: CONFIG.read_bytes(), "orphan.md": text(1)}, (), "orphan")
    good_facts = sb.write_facts("good", clean_facts(real, head))
    matching = {}

    def facts_matching(base: str, head_: str) -> str:
        """A clean facts file for the case's base and head where both resolve, else the good one."""
        b, h = sb.commit_of(base), sb.commit_of(head_)
        if b is None or h is None:
            return good_facts
        if (b, h) not in matching:
            matching[(b, h)] = sb.write_facts("matching", clean_facts(b, h))
        return matching[(b, h)]

    def args(plan=good, component="docs", base=real, head_=head, facts=None) -> list:
        facts = facts if facts is not None else facts_matching(base, head_)
        return ["--plan", plan, "--component", component, "--base", base, "--head", head_, "--facts", facts]

    def without(argv: list, name: str) -> list:
        i = argv.index(name)
        return argv[:i] + argv[i + 2 :]

    def edited(change) -> str:
        content = clean_facts(real, head)
        change(content)
        return sb.write_facts("edited", fill_rounds(content))

    link = sb.repo / "cases" / "link.facts.json"
    link.symlink_to(Path(good_facts).name)
    nan = facts_bytes(clean_facts(real, head)).replace(b'"pr": 1', b'"pr": NaN')
    other = commits["shadow"]
    cases = [
        ("no arguments", [], "missing --plan"),
        ("unknown argument", args() + ["--extra", "x"], "unknown argument '--extra'"),
        ("argument twice", args() + ["--plan", good], "--plan given twice"),
        ("missing value", args()[:-1], "--facts needs a value"),
        ("missing --head", without(args(), "--head"), "missing --head"),
        ("missing --facts", without(args(), "--facts"), "missing --facts"),
        ("--facts given twice", args() + ["--facts", good_facts], "--facts given twice"),
        ("base starting with -", args(base="-HEAD"), "--base must not start with '-'"),
        ("head starting with -", args(head_="-HEAD"), "--head must not start with '-'"),
        ("missing plan", args(plan="cases/missing.md"), "cases/missing.md"),
        ("plan not UTF-8", args(plan=not_utf8), not_utf8),
        ("unresolvable base", args(base="no-such-ref"), "base ref 'no-such-ref' does not resolve"),
        ("unresolvable head", args(head_="no-such-ref"), "head ref 'no-such-ref' does not resolve"),
        ("head is a blob", args(head_=sb.git("hash-object", "-w", "--stdin", data=b"blob\n")), "does not resolve"),
        ("no merge base", args(head_=orphan), "have no merge base"),
        ("head equal to base", args(head_=real), "there is no change to judge"),
        ("head already in the base's history", args(base=head, head_=real), "there is no change to judge"),
        (
            "base without config",
            args(base=commits["noconfig"], head_=sb.child(commits["noconfig"], {"x.md": text(1)})),
            f"no {CONFIG_PATH}",
        ),
        ("unknown component", args(component="nope"), "nope"),
        ("facts missing", args(facts="cases/missing.facts.json"), "cases/missing.facts.json is missing"),
        ("facts a directory", args(facts="cases"), "the facts file cases is not a regular file"),
        ("facts a symlink", args(facts=str(link.relative_to(sb.repo))), "is a symbolic link"),
        ("facts not UTF-8", args(facts=sb.write_facts("not utf-8", b'{"tool": "\xff"}\n')), "is not UTF-8"),
        ("facts not JSON", args(facts=sb.write_facts("not json", b"{\n")), "is not JSON"),
        (
            "facts duplicate key",
            args(facts=sb.write_facts("duplicate", b'{"tool": 1, "tool": 2}\n')),
            "duplicate key 'tool'",
        ),
        ("facts NaN", args(facts=sb.write_facts("nan", nan)), "NaN is not JSON"),
        ("facts top-level list", args(facts=sb.write_facts("list", b"[]\n")), "is not a JSON object"),
        (
            "facts wrong tool",
            args(facts=edited(lambda c: c.update(tool="mintswarm-plan-floor"))),
            "tool must be 'mintswarm-landing-facts'",
        ),
        ("facts wrong version", args(facts=edited(lambda c: c.update(version=4))), "version must be 3, not 4"),
        ("facts of version 2", args(facts=edited(lambda c: c.update(version=2))), "version must be 3, not 2"),
        ("facts of version 1", args(facts=edited(lambda c: c.update(version=1))), "version must be 3, not 1"),
        (
            "facts current true on a stale round",
            args(facts=edited(lambda c: c["gate_rounds"][0].update(commit=other, current=True))),
            f"gate_rounds[0].current is true, but its commit {other} is not the head {head}",
        ),
        (
            "facts current false on a current round",
            args(facts=edited(lambda c: c["review_rounds"][0].update(current=False))),
            f"review_rounds[0].current is false, but its commit {head} is the head {head}",
        ),
        (
            "facts round commit not a hash",
            args(facts=edited(lambda c: c["gate_rounds"][0].update(commit="abc123"))),
            'gate_rounds[0].commit must be a full commit hash, not "abc123"',
        ),
        (
            "facts current a string",
            args(facts=edited(lambda c: c["review_rounds"][0].update(current="true"))),
            'review_rounds[0].current must be a boolean, not "true"',
        ),
        ("facts short head", args(facts=edited(lambda c: c.update(head=head[:12]))), "head must be a full commit hash"),
        (
            "facts head of another commit",
            args(facts=edited(lambda c: c.update(head=real))),
            f"is for head {real}, not the floor's head {head}",
        ),
        (
            "facts base of another commit",
            args(facts=edited(lambda c: c.update(base=other))),
            f"is for base {other}, not the floor's base {real}",
        ),
        ("facts checks not a list", args(facts=edited(lambda c: c.update(checks={}))), "checks must be a list"),
        (
            "facts check without name",
            args(
                facts=edited(lambda c: c.update(checks=[{"conclusion": "success", "source": "check-run", "app": None}]))
            ),
            "checks[0] has no name",
        ),
        (
            "facts boolean round",
            args(facts=edited(lambda c: c["gate_rounds"][0].update(round=True))),
            "gate_rounds[0].round must be an integer, not true",
        ),
        (
            "facts failed_tests not a list",
            args(facts=edited(lambda c: c["gate_rounds"][0].update(failed_tests="whitespace"))),
            'gate_rounds[0].failed_tests must be a list, not "whitespace"',
        ),
        (
            "facts review round without verdict",
            args(facts=edited(lambda c: c["review_rounds"][0].pop("verdict"))),
            "review_rounds[0] has no verdict",
        ),
        (
            "facts steer without url",
            args(
                facts=edited(
                    lambda c: c.update(
                        steers=[{k: v for k, v in steer("alice", "https://example.invalid/1").items() if k != "url"}]
                    )
                )
            ),
            "steers[0] has no url",
        ),
        (
            "facts review_requests missing",
            args(facts=edited(lambda c: c.pop("review_requests"))),
            "has no review_requests",
        ),
        (
            "facts review_requests not a list",
            args(facts=edited(lambda c: c.update(review_requests="zmij"))),
            'review_requests must be a list, not "zmij"',
        ),
        (
            "facts review_requests with a non-string entry",
            args(facts=edited(lambda c: c.update(review_requests=[{"login": "zmij"}]))),
            "review_requests[0] must be a string",
        ),
        (
            "facts rounds out of order",
            args(facts=edited(lambda c: c.update(gate_rounds=[gate_round(2), gate_round(1)]))),
            "gate_rounds round numbers must be positive and strictly increasing, not [2, 1]",
        ),
    ]
    for variant in (
        "no-landing",
        "landing-null",
        "landing-unknown-key",
        "require-human-unknown-key",
        "footprint-unknown-key",
        "mode-sometimes",
        "mode-missing",
        "factor-0.9",
        "factor-true",
        "factor-string",
        "factor-1e400",
        "factor-inf",
        "factor-missing",
        "footprint-missing",
        "undefined-component",
        "plan-approval-invalid",
        "min-confidence-89",
        "min-confidence-101",
        "min-confidence-true",
        "min-confidence-string",
        "min-confidence-float",
    ):
        base = commits[variant]
        # a value yq cannot convert (.inf) fails in yq, before the config is checked
        cases.append(
            (
                f"config {variant}",
                args(base=base, head_=sb.child(base, appended("docs/guide.md", 1))),
                (
                    ("pipeline.landing.min_confidence must be an integer from 90 to 100, not",)
                    if variant.startswith("min-confidence-")
                    else ("config", "yq -p=yaml failed")
                ),
            )
        )
    for name, argv, fragment in cases:
        if name not in ("no arguments", "missing --facts") and "--facts" not in argv:
            T.check(f"unusable {name}: --facts given", False, "selftest bug: the case lacks --facts")
            continue
        proc, verdict = sb.floor(argv)
        T.check(
            f"unusable {name}: exit 2, no verdict",
            proc.returncode == 2
            and verdict is None
            and len(proc.stderr.strip().splitlines()) == 1
            and "Traceback" not in proc.stderr
            and any(f in proc.stderr for f in ((fragment,) if isinstance(fragment, str) else fragment)),
            f"exit {proc.returncode}, verdict {'left' if verdict is not None else 'gone'}, "
            f"expected {fragment!r}: {proc.stderr.strip()}",
        )

    bindir = sb.tmp / "git-only"
    bindir.mkdir()
    (bindir / "git").symlink_to(shutil.which("git"))
    proc, verdict = sb.floor(args(), env=dict(sb.env, PATH=str(bindir)))
    T.check(
        "unusable yq missing: exit 2, no verdict",
        proc.returncode == 2 and verdict is None,
        f"exit {proc.returncode}: {proc.stderr.strip()}",
    )

    lonely = sb.tmp / "lonely" / "landing"
    lonely.mkdir(parents=True)
    shutil.copy2(FLOOR, lonely / "floor.py")
    proc, verdict = sb.floor(args(), script=lonely / "floor.py")
    T.check(
        "unusable plan-approval/floor.py missing: exit 2, no verdict",
        proc.returncode == 2 and verdict is None and "plan floor" in proc.stderr,
        f"exit {proc.returncode}: {proc.stderr.strip()}",
    )


def check_facts_rules(r: Runner) -> None:
    fp = footprint(["docs/guide.md"], estimated_lines=4)
    change = appended("docs/guide.md", 1)

    def expect(name: str, facts, hits: dict, **kwargs):
        return r.expect(f"facts {name}", fp, change, hits, facts=facts, **kwargs)

    # gates
    expect(
        "gates: failed then passed",
        {"gate_rounds": [gate_round(1, "failed", ["whitespace"], "a test failed"), gate_round(2)]},
        {},
    )
    expect("gates: no gate round", {"gate_rounds": []}, {"gates": "there is no gate round"})
    _, verdict = expect(
        "gates: last round failed",
        {"gate_rounds": [gate_round(1), gate_round(2, "failed", ["whitespace"], "a test failed")]},
        {"gates": "gate whitespace failed in the last gate round, 2"},
    )
    T.check(
        "facts gates: last round failed names the round and its reason",
        "the last gate round, 2, has verdict 'failed', not 'passed': a test failed" in reasons_of(verdict, "gates"),
        str(reasons_of(verdict, "gates")),
    )
    expect(
        "gates: passed with a failed test",
        {"gate_rounds": [gate_round(1, "passed", ["agents-drift"])]},
        {"gates": "gate agents-drift failed in the last gate round, 1"},
    )
    expect(
        "gates: unknown verdict",
        {"gate_rounds": [gate_round(1, "skipped")]},
        {"gates": "the last gate round, 1, has verdict 'skipped', not 'passed'"},
    )
    _, verdict = expect(
        "gates: unnamed failed tests",
        {"gate_rounds": [gate_round(1, "failed", [None, 7, "whitespace"], "a test failed")]},
        {"gates": "an unnamed gate (null) failed in the last gate round, 1"},
    )
    T.check(
        "facts gates: unnamed failed tests are each a reason",
        reasons_of(verdict, "gates")[1:]
        == [
            "an unnamed gate (null) failed in the last gate round, 1",
            "an unnamed gate (7) failed in the last gate round, 1",
            "gate whitespace failed in the last gate round, 1",
        ],
        str(reasons_of(verdict, "gates")),
    )

    base = r.commits["real"]
    other = r.commits["shadow"]
    head = r.sb.child(base, change, "stale rounds")
    reason = f"the last gate round, 1, judged commit {base}, not the head {head}"
    _, verdict = expect(
        "gates: a passed last round from the base commit",
        {"gate_rounds": [gate_round(1, commit=base)]},
        {"gates": reason},
        head=head,
    )
    T.check(
        "facts gates: a stale passed round gives only the commit reason",
        reasons_of(verdict, "gates") == [reason],
        str(reasons_of(verdict, "gates")),
    )
    expect(
        "gates: a stale earlier round, then a current pass",
        {"gate_rounds": [gate_round(1, commit=base), gate_round(2)]},
        {},
        head=head,
    )
    _, verdict = expect(
        "gates: a stale failed last round",
        {"gate_rounds": [gate_round(1, "failed", ["whitespace"], "a test failed", commit=base)]},
        {"gates": reason},
        head=head,
    )
    T.check(
        "facts gates: a stale failed round gives the verdict and the commit reasons",
        len(reasons_of(verdict, "gates")) == 3,
        str(reasons_of(verdict, "gates")),
    )

    # checks
    _, verdict = expect("checks: zero checks", {"checks": []}, {})
    T.check(
        "facts checks: zero checks lists none",
        isinstance(verdict, dict) and verdict["facts"]["checks"] == [],
        str(verdict.get("facts") if isinstance(verdict, dict) else verdict),
    )
    green = [check_entry("build", "success"), check_entry("ci/lint", "success", "status", None)]
    _, verdict = expect("checks: two successes", {"checks": green}, {})
    T.check(
        "facts checks: two successes are both listed",
        isinstance(verdict, dict) and verdict["facts"]["checks"] == green,
        str(verdict.get("facts") if isinstance(verdict, dict) else verdict),
    )
    expect(
        "checks: a failed check run",
        {"checks": [check_entry("build", "failure")]},
        {"checks": "check build (check-run, app github-actions) has conclusion 'failure', not 'success'"},
    )
    expect(
        "checks: a check run not completed",
        {"checks": [green[0], check_entry("test", None)]},
        {"checks": "check test (check-run, app github-actions) is not completed"},
    )
    expect(
        "checks: a pending status",
        {"checks": [check_entry("ci/lint", "pending", "status", None)]},
        {"checks": "check ci/lint (status, app none) has conclusion 'pending', not 'success'"},
    )
    expect(
        "checks: a neutral check run",
        {"checks": [check_entry("docs", "neutral", app=None)]},
        {"checks": "check docs (check-run, app none) has conclusion 'neutral', not 'success'"},
    )
    forged = "x\nfloor: clean\u2028floor: clean\x85floor: clean"
    proc, _ = expect(
        "checks: a check name that would forge a line",
        {"checks": [check_entry(forged, "failure", "status", None)]},
        {"checks": f"check {forged} (status, app none) has conclusion 'failure', not 'success'"},
    )
    lines = proc.stdout.splitlines()
    T.check(
        "facts checks: a forged name stays on its own line, escaped",
        lines[-1] == "floor: hit"
        and not any(line.startswith("floor: clean") for line in lines)
        and "hit     checks: check x\\nfloor: clean\\u2028floor: clean\\u0085floor: clean (status" in proc.stdout,
        repr(proc.stdout),
    )
    for config in ("workflows", "workflows-case"):
        expect(
            f"checks: zero checks on a base with workflows ({config})",
            {"checks": []},
            {"checks": "no check is reported on the head, but the base has .github/workflows"},
            config=config,
        )
        expect(f"checks: one success on a base with workflows ({config})", {"checks": green[:1]}, {}, config=config)

    # review
    expect(
        "review: changes required then pass",
        {"review_rounds": [review_round(1, "changes_required"), review_round(2)]},
        {},
    )
    expect("review: no review round", {"review_rounds": []}, {"review": "there is no review round"})
    expect(
        "review: pass then changes required",
        {"review_rounds": [review_round(1), review_round(2, "changes_required")]},
        {"review": "the last review round, 2, has verdict 'changes_required', not 'pass'"},
    )
    reason = f"the last review round, 1, judged commit {other}, not the head {head}"
    _, verdict = expect(
        "review: a passing last round from another commit",
        {"review_rounds": [review_round(1, commit=other)]},
        {"review": reason},
        head=head,
    )
    T.check(
        "facts review: a stale passing round gives only the commit reason",
        reasons_of(verdict, "review") == [reason],
        str(reasons_of(verdict, "review")),
    )
    expect(
        "review: a stale earlier round, then a current pass",
        {"review_rounds": [review_round(1, "changes_required", commit=base), review_round(2)]},
        {},
        head=head,
    )

    # steers
    expect("steers: none", {"steers": []}, {})
    one = steer("alice", "https://github.com/o/r/pull/1#issuecomment-1")
    two = steer("bob", "https://github.com/o/r/issues/2#issuecomment-2", "issue-comment", "2026-01-03T00:00:00Z")
    first = "pr-comment by alice at 2026-01-02T00:00:00Z is a steer: https://github.com/o/r/pull/1#issuecomment-1"
    second = (
        "issue-comment by bob at 2026-01-03T00:00:00Z is a steer: " "https://github.com/o/r/issues/2#issuecomment-2"
    )
    expect("steers: one steer", {"steers": [one]}, {"steers": first})
    _, verdict = expect("steers: two steers", {"steers": [one, two]}, {"steers": second})
    T.check(
        "facts steers: two steers give two reasons",
        reasons_of(verdict, "steers") == [first, second],
        str(reasons_of(verdict, "steers")),
    )

    # review-requests
    expect("review-requests: none", {"review_requests": []}, {})
    pending = "review re-requested from {} is pending; their earlier review is not current"
    expect("review-requests: one pending", {"review_requests": ["zmij"]}, {"review-requests": pending.format("zmij")})
    _, verdict = expect(
        "review-requests: two pending",
        {"review_requests": ["alice", "zmij"]},
        {"review-requests": pending.format("zmij")},
    )
    T.check(
        "facts review-requests: two pending give one reason per login",
        reasons_of(verdict, "review-requests") == [pending.format("alice"), pending.format("zmij")],
        str(reasons_of(verdict, "review-requests")),
    )

    # all four together, also with an unusable footprint
    def all_hit(c: dict) -> dict:
        c.update(gate_rounds=[], checks=[check_entry("build", "failure")], review_rounds=[], steers=[one])
        return c

    hits = {
        "gates": "there is no gate round",
        "checks": "check build (check-run, app github-actions) has conclusion 'failure', not 'success'",
        "review": "there is no review round",
        "steers": first,
    }
    expect("all four rules hit", all_hit, hits)
    _, verdict = r.expect(
        "facts all four rules hit with no footprint",
        plan_text(None),
        change,
        {"footprint": "no '## Footprint' section", **hits},
        facts=all_hit,
    )
    T.check(
        "facts all four rules hit with no footprint: only the diff rules are skipped",
        isinstance(verdict, dict) and verdict["not_evaluated"] == DIFF_RULES,
        str(verdict.get("not_evaluated") if isinstance(verdict, dict) else verdict),
    )


def ctrf_report(tests: list) -> dict:
    passed = sum(1 for t in tests if t.get("status") == "passed")
    return {
        "reportFormat": "CTRF",
        "specVersion": "0.3.0",
        "results": {
            "tool": {"name": "mintswarm-gates"},
            "summary": {
                "tests": len(tests),
                "passed": passed,
                "failed": len(tests) - passed,
                "skipped": 0,
                "pending": 0,
                "other": 0,
                "start": 1,
                "stop": 2,
            },
            "tests": tests,
        },
    }


def write_json(path: Path, value) -> bytes:
    data = (json.dumps(value, indent=2) + "\n").encode("utf-8")
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(data)
    return data


def kept_round(rounds: Path, kind: str, number: int, files: dict, head: str) -> None:
    """Write a kept round as keep-round.py records it: the files and a round.json with the head and sha256s."""
    directory = rounds / f"{kind}-{number}"
    directory.mkdir(parents=True)
    listed = []
    for name in sorted(files):
        (directory / name).write_bytes(files[name])
        listed.append({"name": name, "sha256": hashlib.sha256(files[name]).hexdigest()})
    write_json(
        directory / "round.json",
        {
            "tool": "mintswarm-keep-round",
            "version": 2,
            "kind": kind,
            "round": number,
            "head": head,
            "kept_at": "2026-01-01T00:00:00Z",
            "files": listed,
        },
    )


def collected_facts(
    sb: Sandbox, name: str, base: str, head: str, tests: list, check_runs: list, pr_comments: list, kept_at: str
) -> tuple:
    """Run collect-facts.py assemble on saved responses and rounds kept at `kept_at` in runner state outside
    the repository; return (exit, facts bytes or None). The PR has a pending review request from zmij, who has
    already requested changes only in the `hits` case."""
    responses = sb.tmp / f"collected-{name}" / "responses"
    rounds = sb.tmp / "runner-state" / f"collected-{name}" / "rounds"
    write_json(responses / "request.json", {"pr": 1, "issue": 2, "head": head, "fetched_at": "2026-01-03T00:00:00Z"})
    pull = {"number": 1, "head": {"sha": head}, "base": {"sha": base}, "requested_reviewers": [{"login": "zmij"}]}
    write_json(responses / "pull.1.json", pull)
    zmij = {"user": {"login": "zmij"}, "state": "CHANGES_REQUESTED", "submitted_at": "2026-01-01T00:00:00Z"}
    zmij["html_url"] = "https://github.com/o/r/pull/1#pullrequestreview-1"
    write_json(responses / "check-runs.1.json", {"total_count": len(check_runs), "check_runs": check_runs})
    write_json(responses / "status.1.json", {"statuses": []})
    for listing, items in (
        ("pr-comments", pr_comments),
        ("reviews", [zmij] if name == "hits" else []),
        ("review-comments", []),
        ("issue-comments", []),
    ):
        write_json(responses / f"{listing}.1.json", items)
    gates_input = json.dumps({"gates": [{"name": "whitespace", "command": "true"}]}).encode("utf-8")
    kept_round(
        rounds,
        "gates",
        1,
        {"gates.input.json": gates_input, "gates.ctrf.json": json.dumps(ctrf_report(tests)).encode("utf-8")},
        kept_at,
    )
    review = "# Review round 1: collected\n\n## Verdict\n\npass\n\n```yaml\nfindings: []\n```\n"
    kept_round(rounds, "review", 1, {"review.md": review.encode("utf-8")}, kept_at)
    facts = sb.repo / ".mintswarm" / "job" / "inputs" / "landing" / "facts.json"
    proc = subprocess.run(
        [
            sys.executable,
            "-I",
            str(COLLECT_FACTS),
            "assemble",
            "--responses",
            str(responses),
            "--pr",
            "1",
            "--issue",
            "2",
            "--head",
            head,
            "--base",
            base,
            "--approver",
            "maintainer",
            "--approved-at",
            "2026-01-01T00:00:00Z",
            "--rounds",
            str(rounds),
        ],
        cwd=sb.repo,
        env=sb.env,
        capture_output=True,
        text=True,
    )
    data = facts.read_bytes() if proc.returncode == 0 and facts.is_file() else None
    return proc, data


def check_collected_facts(r: Runner, sb: Sandbox) -> None:
    """Facts written by the real collect-facts.py assemble, read by the floor."""
    base = r.commits["real"]
    fp = footprint(["docs/guide.md"], estimated_lines=4)
    green = [{"id": 1, "name": "build", "status": "completed", "conclusion": "success", "app": {"slug": "ci"}}]
    cases = [
        ("clean", [{"name": "whitespace", "status": "passed"}], green, [], {}),
        (
            "stale",
            [{"name": "whitespace", "status": "passed"}],
            green,
            [],
            {
                "gates": f"the last gate round, 1, judged commit {base}, not the head {{head}}",
                "review": f"the last review round, 1, judged commit {base}, not the head {{head}}",
            },
        ),
        (
            "hits",
            [{"status": "failed"}],
            [{"id": 1, "name": "build", "status": "completed", "conclusion": "failure", "app": {"slug": "ci"}}],
            [
                {
                    "id": 9,
                    "html_url": "https://github.com/o/r/pull/1#issuecomment-9",
                    "user": {"login": "alice", "type": "User"},
                    "created_at": "2026-01-02T00:00:00Z",
                    "updated_at": "2026-01-02T00:00:00Z",
                    "body": "please also do X\n",
                }
            ],
            {
                "gates": "an unnamed gate (null) failed in the last gate round, 1",
                "checks": "check build (check-run, app ci) has conclusion 'failure', not 'success'",
                "steers": "pr-comment by alice at 2026-01-02T00:00:00Z is a steer: "
                "https://github.com/o/r/pull/1#issuecomment-9",
                "review-requests": "review re-requested from zmij is pending; their earlier review is not current",
            },
        ),
    ]
    for name, tests, check_runs, comments, hits in cases:
        head = sb.child(base, appended("docs/guide.md", 1), f"collected {name}")
        proc, data = collected_facts(
            sb, name, base, head, tests, check_runs, comments, base if name == "stale" else head
        )
        hits = {rule: reason.replace("{head}", head) for rule, reason in hits.items()}
        if not T.check(
            f"collected facts {name}: collect-facts.py assemble writes them",
            data is not None,
            f"exit {proc.returncode}: {proc.stderr.strip()}",
        ):
            continue
        r.expect(f"collected facts {name}", fp, None, hits, head=head, facts=data)


def check_facts_contract() -> None:
    collector = load_module("msw_collect_facts_for_selftest", COLLECT_FACTS)
    floor = load_module("msw_landing_floor_for_selftest", FLOOR)
    T.check(
        "facts contract: collect-facts.py writes the tool and version the floor reads",
        (collector.TOOL, collector.VERSION)
        == ("mintswarm-landing-facts", 3)
        == (floor.FACTS_TOOL, floor.FACTS_VERSION),
        f"collector {collector.TOOL!r} {collector.VERSION!r}, floor {floor.FACTS_TOOL!r} {floor.FACTS_VERSION!r}",
    )


def check_min_confidence() -> None:
    """validate_landing, called directly: the default and a given value."""
    floor = load_module("msw_landing_floor_for_selftest", FLOOR)
    floor.P = floor.load_plan_floor()
    config = yq_json(CONFIG)
    components = config["components"]
    got = floor.validate_landing(copy.deepcopy(config), components)["min_confidence"]
    T.check(
        "min_confidence: absent gives the default 95",
        "min_confidence" not in config["pipeline"]["landing"] and got == floor.DEFAULT_MIN_CONFIDENCE == 95,
        f"got {got!r}, DEFAULT_MIN_CONFIDENCE {floor.DEFAULT_MIN_CONFIDENCE!r}",
    )
    T.check("min_confidence: the lowest allowed is 90", floor.MIN_CONFIDENCE == 90, repr(floor.MIN_CONFIDENCE))
    for value in (90, 97, 100):
        given = copy.deepcopy(config)
        given["pipeline"]["landing"]["min_confidence"] = value
        got = floor.validate_landing(given, components)["min_confidence"]
        T.check(f"min_confidence: {value} is returned as given", got == value and type(got) is int, repr(got))
    for value in (89, 101, True, "95", 95.0, None):
        given = copy.deepcopy(config)
        given["pipeline"]["landing"]["min_confidence"] = value
        try:
            floor.validate_landing(given, components)
            message = None
        except floor.InputError as exc:
            message = str(exc)
        T.check(
            f"min_confidence: {value!r} is refused",
            message == f"config: pipeline.landing.min_confidence must be an integer from 90 to 100, not {value!r}",
            repr(message),
        )


def check_isolation(r: Runner, sb: Sandbox) -> None:
    sentinel = b'{"sentinel": "the plan floor\'s verdict"}\n'
    target = sb.repo / PLAN_VERDICT
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_bytes(sentinel)
    fp = footprint(["docs/guide.md"], estimated_lines=4)
    r.expect("isolation clean", fp, appended("docs/guide.md", 1), {})
    T.check("plan verdict untouched after a clean run", target.read_bytes() == sentinel)
    r.expect("isolation hit", fp, appended("CONTRIBUTING.md", 1), {"undeclared-files": None})
    T.check("plan verdict untouched after a hit", target.read_bytes() == sentinel)
    same = sb.write_facts("isolation exit 2", clean_facts(r.commits["real"], r.commits["real"]))
    proc, verdict = sb.floor(
        [
            "--plan",
            "cases/missing.md",
            "--component",
            "docs",
            "--base",
            r.commits["real"],
            "--head",
            r.commits["real"],
            "--facts",
            same,
        ]
    )
    T.check(
        "plan verdict untouched after exit 2",
        proc.returncode == 2 and verdict is None and target.read_bytes() == sentinel,
        proc.stderr.strip(),
    )

    copy_root = sb.tmp / "copy"
    for name, source in (("landing", FLOOR), ("plan-approval", PLAN_FLOOR)):
        (copy_root / name).mkdir(parents=True)
        shutil.copy2(source, copy_root / name / "floor.py")
    plan = sb.write_plan("copied scripts", plan_text(fp))
    head = sb.child(r.commits["real"], {**appended("docs/guide.md", 1), "CLAUDE.md": text(1)})
    facts = sb.write_facts("copied scripts", clean_facts(r.commits["real"], head))
    argv = ["--plan", plan, "--component", "docs", "--base", r.commits["real"], "--head", head, "--facts", facts]
    proc, verdict = sb.floor(argv)
    copied, copied_verdict = sb.floor(argv, script=copy_root / "landing" / "floor.py")
    T.check(
        "copied scripts: the same exit, output and verdict",
        isinstance(verdict, dict)
        and verdict == copied_verdict
        and proc.returncode == copied.returncode == 1
        and proc.stdout == copied.stdout,
        f"exit {proc.returncode} and {copied.returncode}: {copied.stderr.strip()}",
    )
    T.check(
        "copied scripts: no bytecode written",
        not list(sb.tmp.rglob("__pycache__"))
        and not (HERE / "__pycache__").exists()
        and not (PLAN_FLOOR.parent / "__pycache__").exists(),
    )


def check_historic(r: Runner, sb: Sandbox) -> None:
    real = CONFIG.read_bytes()
    for commit, (fixture, component, hits) in HISTORIC.items():
        name = f"historic {commit} with {fixture}"

        def root(*args: str) -> bytes:
            return sb.raw("-C", str(ROOT), *args)

        try:
            root("cat-file", "-e", f"{commit}^{{commit}}")
            root("cat-file", "-e", f"{commit}^^{{commit}}")
            paths = [
                p.decode("utf-8")
                for p in root(
                    "diff-tree",
                    "-r",
                    "--no-renames",
                    "--no-commit-id",
                    "--name-only",
                    "-z",
                    f"{commit}^",
                    commit,
                ).split(b"\0")
                if p
            ]
            versions = {}
            for rev in (f"{commit}^", commit):
                versions[rev] = {}
                for path in paths:
                    entry = root("ls-tree", "-z", rev, "--", path).rstrip(b"\0")
                    if entry:
                        mode, _, sha = entry.split(b"\t", 1)[0].decode("ascii").split(" ")
                        versions[rev][path] = (mode, root("cat-file", "blob", sha))
            expected_numstat = sorted(
                root("show", "--numstat", "--no-renames", "--format=", commit).decode("utf-8").splitlines()
            )
        except RuntimeError as exc:
            T.check(f"{name}: replay", False, f"full local history is needed (not a shallow clone): {exc}")
            continue
        if CONFIG_PATH in paths:
            T.check(f"{name}: replay", False, "selftest bug: the commit changes the config")
            continue
        base = sb.commit({CONFIG_PATH: real, **versions[f"{commit}^"]}, (), f"replay base of {commit}")
        head_files = {CONFIG_PATH: real, **versions[commit]}
        head = sb.commit(head_files, (base,), f"replay of {commit}")
        replayed = sorted(l for l in sb.git("diff", "--numstat", "--no-renames", base, head).splitlines() if l)
        T.check(
            f"{name}: replayed numstat equals the original",
            replayed == [l for l in expected_numstat if l],
            f"{replayed} against {expected_numstat}",
        )
        proc, verdict = r.run(name, (FIXTURES / fixture).read_bytes(), component=component, base=base, head=head)
        expected = [
            {"rule": rule, "result": "hit" if rule in hits else "clean", "reasons": hits.get(rule, [])}
            for rule in RULES
        ]
        got = verdict.get("rules") if isinstance(verdict, dict) else None
        T.check(
            f"{name}: the recorded verdict",
            got == expected and verdict.get("not_evaluated") == [] and proc.returncode == (1 if hits else 0),
            f"exit {proc.returncode}, rules {json.dumps(got)}",
        )


def partition(paths: list) -> list:
    """Split paths into groups a single tree can hold: no path is a directory of another."""
    groups = []
    for path in paths:
        for group in groups:
            if not any(p.startswith(path + "/") or path.startswith(p + "/") for p in group):
                group.append(path)
                break
        else:
            groups.append([path])
    return groups


def check_globs(r: Runner, sb: Sandbox) -> None:
    plan_floor = load_module("msw_plan_floor_for_selftest", PLAN_FLOOR)
    paths = [p for p in GLOB_PATHS if not p.endswith("/")]  # git cannot hold "a/" as a file
    plan = plan_text(footprint(["docs/guide.md"], estimated_lines=10))
    expected, got = [], []
    for index, group in enumerate(partition(paths)):
        _, verdict = r.run(f"glob group {index}", plan, {p: text(1) for p in group}, config="globs")
        expected += [
            f"{p} matches require_human.paths {g}"
            for p in group
            for g in GLOB_PATTERNS
            if plan_floor.matching([g], p, ci=True)
        ]
        got += reasons_of(verdict, "require-human-paths")
    pairs = len(GLOB_PATTERNS) * len(paths)
    T.check(
        f"glob semantics: require-human-paths agrees with plan-approval/floor.py on {pairs} pairs",
        sorted(got) == sorted(expected),
        f"only landing: {sorted(set(got) - set(expected))[:5]}, "
        f"only plan floor: {sorted(set(expected) - set(got))[:5]}",
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
        ("README.md", "readme.md", True),
    ]
    wrong = [(g, p) for g, p, want in pinned if (f"{p} matches require_human.paths {g}" in got) != want]
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
        T.check(
            f"standard library: {path.name} imports only {', '.join(sorted(names))}",
            not outside,
            f"not standard library: {outside}",
        )


def build_commits(sb: Sandbox) -> tuple:
    real_bytes = CONFIG.read_bytes()
    real = yq_json(CONFIG)
    commits = {"noconfig": sb.commit(dict(BASE_TREE), (), "no config")}
    configs = {"real": real}

    def variant(name: str, change, tree: dict = BASE_TREE, raw=None) -> None:
        config = copy.deepcopy(real)
        change(config)
        configs[name] = config
        data = as_json(config)
        if raw is not None:
            data = data.replace(b'"__RAW__"', raw)
        commits[name] = sb.commit({**tree, CONFIG_PATH: data}, (), name)

    def landing(c: dict) -> dict:
        return c["pipeline"]["landing"]

    def factor(value):
        return lambda c: landing(c)["footprint"].update(lines_factor=value)

    commits["real"] = sb.commit({**BASE_TREE, CONFIG_PATH: real_bytes}, (), "real")
    for name, path in (("workflows", ".github/workflows/ci.yml"), ("workflows-case", ".GitHub/Workflows/CI.yml")):
        commits[name] = sb.commit({**BASE_TREE, CONFIG_PATH: real_bytes, path: text(1, "workflow")}, (), name)
        configs[name] = real
    variant("shadow", lambda c: landing(c).update(mode="shadow"))
    variant("factor-1.15", factor(1.15))
    variant("factor-1", factor(1))
    variant("rh-web", lambda c: landing(c).update(require_human={"components": ["web"]}))
    variant("no-rh", lambda c: landing(c).update(require_human={}))
    variant("globs", lambda c: landing(c).update(require_human={"paths": GLOB_PATTERNS}), tree={})
    variant("no-landing", lambda c: c["pipeline"].pop("landing"))
    variant("landing-null", lambda c: c["pipeline"].update(landing=None))
    variant("landing-unknown-key", lambda c: landing(c).update(post_merge_watch=True))
    variant("require-human-unknown-key", lambda c: landing(c)["require_human"].update(limits=[]))
    variant("footprint-unknown-key", lambda c: landing(c)["footprint"].update(files_factor=2))
    variant("mode-sometimes", lambda c: landing(c).update(mode="sometimes"))
    variant("mode-missing", lambda c: landing(c).pop("mode"))
    variant("factor-0.9", factor(0.9))
    variant("factor-true", factor(True))
    variant("factor-string", factor("1.5"))
    variant("factor-1e400", factor("__RAW__"), raw=b"1e400")
    variant("factor-inf", factor("__RAW__"), raw=b".inf")
    variant("factor-missing", lambda c: landing(c)["footprint"].pop("lines_factor"))
    variant("footprint-missing", lambda c: landing(c).pop("footprint"))
    variant("undefined-component", lambda c: landing(c).update(require_human={"components": ["nowhere"]}))
    variant("plan-approval-invalid", lambda c: c["pipeline"]["plan_approval"].update(mode="sometimes"))
    for name, value in (
        ("90", 90),
        ("100", 100),
        ("89", 89),
        ("101", 101),
        ("true", True),
        ("string", "95"),
        ("float", 95.5),  # yq reads 95.0 as 95, so the end-to-end float is not integral; check_min_confidence has 95.0
    ):
        variant(f"min-confidence-{name}", lambda c, value=value: landing(c).update(min_confidence=value))
    modes = {name: (config.get("pipeline", {}).get("landing") or {}).get("mode") for name, config in configs.items()}
    return commits, configs, modes


def main() -> int:
    for tool in ("git", "yq"):
        if shutil.which(tool) is None:
            print(f"floor-selftest.py: {tool} is not on PATH", file=sys.stderr)
            return 2
    with tempfile.TemporaryDirectory(prefix="msw-landing-floor-") as tmp:
        sb = Sandbox(Path(tmp))
        commits, configs, modes = build_commits(sb)
        runner = Runner(sb, commits, modes)
        check_config(sb, commits)
        check_clean(runner)
        check_single_rules(runner)
        check_protected(runner, commits)
        check_footprint_hit(runner)
        check_submodules(runner, sb)
        check_protected_entries(runner)
        check_history_tampering(runner, sb)
        check_binary_content(runner, sb)
        check_base_ref(runner, sb, configs)
        check_facts_rules(runner)
        check_collected_facts(runner, sb)
        check_unusable(sb, commits)
        check_isolation(runner, sb)
        check_historic(runner, sb)
        check_globs(runner, sb)
        check_object_store(runner, sb)
    check_facts_contract()
    check_min_confidence()
    check_stdlib()
    print(f"{T.passed} passed, {T.failed} failed")
    return 0 if T.failed == 0 else 1


if __name__ == "__main__":
    sys.exit(main())
