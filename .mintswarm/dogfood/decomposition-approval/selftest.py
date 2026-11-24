#!/usr/bin/env python3
"""Offline self-test of floor.py, the structure floor of decomposition approval.

    selftest.py

Needs `git` and `yq` on PATH; nothing else, and no network. It works in a
temporary directory with its own HOME, no system or global git config and a
fixed identity, and runs `git init` on a throwaway repository with no
remote. Its commits: `noconfig`, an empty commit; `base`, a config whose
components are agents, docs and web; and HEAD, a config that adds `extra`
and drops `web`, with an uncommitted worktree config that does the same.
`badcomponents` is a branch whose config has `components` as a list. All
fixtures are inline: decompositions and parent issues are built here and
written under `cases/`. floor.py runs as `sys.executable floor.py ...` with
the repository as its working directory and a TMPDIR inside the sandbox.

Checks:

- verdict shape, on every run that exits 0 or 1: the keys in order,
  version 1, the seven rules in order across `rules` and `not_evaluated`,
  which is empty or the five child rules after a `children-block` hit, the
  sha256 of both documents, `base.commit`, the base config's components,
  `verdict` against the rules and the exit code, and the printed lines;
- a clean base modelled on the decompose output format, and a clean case
  with trailing whitespace on both fence lines of the Children block;
- per rule, clean and hit cases: children-block (missing section or fence,
  `~~~` and bare `yaml` openers, unclosed fence, yaml that does not parse,
  multi-document yaml, `children` empty or not a list, a duplicate or
  missing `key`, a missing `depends_on`), child-kind (`design`, missing),
  child-component (missing, empty, a list), known-components (`web` known
  at `base`, `extra` a hit), depends-on-siblings, no-cycles (two children,
  a self-dependency) and coverage (a missing row, an empty `Covered by`
  cell, bullet-only criteria, no criteria section, no Coverage table; a
  reworded row, `N)` criteria and a decoy section under `## Comments` are
  clean);
- unusable input exits 2 and leaves no verdict, even a stale one: bad
  arguments, a missing, unreadable or non-UTF-8 document, a base ref that
  does not resolve, no config at the base, `components` not a mapping and
  no `yq` on PATH;
- no `__pycache__` is left next to either floor.
"""

import hashlib
import json
import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
FLOOR = HERE / "floor.py"
PLAN_FLOOR_DIR = HERE.parent / "plan-approval"
CONFIG_PATH = ".mintswarm/config.yaml"
VERDICT = Path(".mintswarm/job/outputs/floor.json")
RULES = [
    "children-block",
    "child-kind",
    "child-component",
    "known-components",
    "depends-on-siblings",
    "no-cycles",
    "coverage",
]
CHILD_RULES = RULES[1:6]
VERDICT_KEYS = [
    "tool",
    "version",
    "decomposition",
    "issue_document",
    "base",
    "config",
    "components",
    "verdict",
    "rules",
    "not_evaluated",
]
BASE_COMPONENTS = ["agents", "docs", "web"]


def config_yaml(components: list, comment: str = "") -> bytes:
    lines = [f"# {comment}"] if comment else []
    lines += ["version: 1", "components:"]
    for name in components:
        lines += [f"  {name}:", "    paths:", f'      - "{name}/**"']
    lines += ["gates:", "  run: []"]
    return ("\n".join(lines) + "\n").encode("utf-8")


CHILDREN = """children:
  - key: floor
    kind: implementation
    title: "Decomposition floor"
    component: agents
    depends_on: []
    problem: |
      A floor is needed.
    acceptance_criteria:
      - "the floor writes floor.json"
  - key: decision
    kind: implementation
    title: "Decision"
    component: web
    depends_on: [floor]
    problem: |
      A decision is needed.
  - key: rollout
    kind: plan
    title: "Rollout"
    depends_on:
      - floor
      - decision
"""
COVERAGE = [
    "| 1. The floor writes a verdict | `floor` |",
    "| 2. The decision reads it | `decision` |",
    "| 3. It rolls out | `rollout` |",
]


def child_yaml(children: list) -> str:
    """A children block from dicts; values in flow form, which is yaml."""
    out = ["children:"]
    for child in children:
        first = True
        for key, value in child.items():
            out.append(f"{'  - ' if first else '    '}{key}: {json.dumps(value)}")
            first = False
    return "\n".join(out) + "\n"


def child(key: str, kind="implementation", component="agents", depends_on=None, **extra) -> dict:
    c = {"key": key, "kind": kind, "component": component, "depends_on": depends_on or []}
    for name, value in list(c.items()):
        if value is DROP:
            del c[name]
    c.update(extra)
    return c


DROP = object()


def decomposition(children: str = CHILDREN, coverage: list = None, block: str = None, tail: str = "") -> str:
    rows = COVERAGE if coverage is None else coverage
    if block is None:
        block = f"## Children\n\n```yaml\n{children}```\n"
    return (
        "# Decomposition: An example\n\nThree children.\n\n## Approach\n\nSplit it.\n\n"
        + block
        + "\n## Coverage\n\n| Acceptance criterion of the issue | Covered by |\n| --- | --- |\n"
        + "".join(row + "\n" for row in rows)
        + "\n## Order\n\nfloor, then decision, then rollout.\n\n## Questions\n"
        + tail
    )


CRITERIA = (
    "1. The floor writes a verdict.\n2. The decision reads it,\n   wrapped onto a second line.\n3. It rolls out.\n"
)


def issue_document(criteria: str = CRITERIA, heading: str = "## Acceptance criteria") -> str:
    section = f"{heading}\n\n{criteria}\n" if heading else ""
    return (
        "# An example\n\n- **Issue:** #9\n- **Kind:** plan\n\n## Problem\n\nIt is needed.\n\n"
        + section
        + "## Comments\n\n## Acceptance criteria\n\n4. A decoy criterion in a comment.\n"
    )


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
                "PYTHONDONTWRITEBYTECODE": "1",
            }
        )
        store_tmp = tmp / "tmpdir"
        store_tmp.mkdir()
        env["TMPDIR"] = str(store_tmp)
        self.env = env
        self.repo.mkdir()
        self.cases.mkdir()
        self.git("init", "-q", "-b", "master")
        self.counter = 0
        self.commits = {}

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
        self.git("commit", "-q", "-m", message)
        return self.git("rev-parse", "HEAD")

    def build(self) -> None:
        self.git("commit", "-q", "--allow-empty", "-m", "noconfig")
        self.commits["noconfig"] = self.git("rev-parse", "HEAD")
        self.commits["base"] = self.commit_config(config_yaml(BASE_COMPONENTS), "base")
        self.git("tag", "base")
        self.git("checkout", "-q", "-b", "badcomponents")
        bad = b"version: 1\ncomponents:\n  - agents\n  - docs\n"
        self.commits["badcomponents"] = self.commit_config(bad, "components as a list")
        self.git("checkout", "-q", "master")
        self.commits["head"] = self.commit_config(config_yaml(["agents", "docs", "extra"]), "head")
        (self.repo / CONFIG_PATH).write_bytes(config_yaml(["agents", "docs", "extra"], "uncommitted"))

    def write(self, name: str, content) -> str:
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
            [sys.executable, str(FLOOR), *args], cwd=self.repo, env=env or self.env, capture_output=True, text=True
        )
        verdict = None
        if stale.exists():
            try:
                verdict = json.loads(stale.read_text(encoding="utf-8"))
            except ValueError:
                verdict = "unparsable"
        return proc, verdict


def sha256(sb: Sandbox, rel: str) -> str:
    return hashlib.sha256((sb.repo / rel).read_bytes()).hexdigest()


def check_shape(sb: Sandbox, name: str, proc, verdict, dec: str, issue: str, ref: str) -> bool:
    problems = []
    if not isinstance(verdict, dict):
        return T.check(f"{name}: verdict shape", False, f"no verdict (exit {proc.returncode}): {proc.stderr.strip()}")
    if list(verdict) != VERDICT_KEYS:
        problems.append(f"keys {list(verdict)}")
    if verdict.get("tool") != "mintswarm-decomposition-floor" or verdict.get("version") != 1:
        problems.append("tool or version")
    if verdict.get("decomposition") != {"path": dec, "sha256": sha256(sb, dec)}:
        problems.append("decomposition binding")
    if verdict.get("issue_document") != {"path": issue, "sha256": sha256(sb, issue)}:
        problems.append("issue document binding")
    if verdict.get("base") != {"ref": ref, "commit": sb.git("rev-parse", f"{ref}^{{commit}}")}:
        problems.append(f"base {verdict.get('base')}")
    if verdict.get("config") != CONFIG_PATH or verdict.get("components") != BASE_COMPONENTS:
        problems.append(f"config or components {verdict.get('components')}")
    rules = verdict.get("rules") or []
    skipped = verdict.get("not_evaluated")
    evaluated = [r.get("rule") for r in rules]
    if (
        evaluated != [rule for rule in RULES if rule not in (skipped or [])]
        or sorted(evaluated + (skipped or []), key=lambda rule: RULES.index(rule) if rule in RULES else -1) != RULES
    ):
        problems.append(f"rule order {[r.get('rule') for r in rules]} + {skipped}")
    if skipped not in ([], CHILD_RULES):
        problems.append(f"not_evaluated {skipped}")
    for r in rules:
        if list(r) != ["rule", "result", "reasons"] or r["result"] not in ("clean", "hit"):
            problems.append(f"rule entry {r}")
        elif (r["result"] == "hit") != bool(r["reasons"]):
            problems.append(f"{r['rule']}: result against reasons")
    hit = any(r.get("result") == "hit" for r in rules)
    if verdict.get("verdict") != ("hit" if hit else "clean") or proc.returncode != (1 if hit else 0):
        problems.append(f"verdict {verdict.get('verdict')} with exit {proc.returncode}")
    expected = []
    for r in rules:
        line = f"{r.get('result', ''):7} {r.get('rule')}"
        if r.get("reasons"):
            line += f": {r['reasons'][0]}"
        expected.append(line)
    expected.append(f"floor: {verdict.get('verdict')}")
    if proc.stdout.splitlines() != expected:
        problems.append(f"printed {proc.stdout.splitlines()}")
    return T.check(f"{name}: verdict shape", not problems, "; ".join(problems))


def case(sb: Sandbox, name: str, dec_text: str, hits=(), skipped=False, issue_text: str = None, contains=None):
    """Run the floor and check that exactly the rules in hits are hits."""
    dec = sb.write(name, dec_text)
    issue = sb.write(f"{name} issue", issue_document() if issue_text is None else issue_text)
    proc, verdict = sb.floor(["--decomposition", dec, "--issue-document", issue, "--base", "base"])
    if not check_shape(sb, name, proc, verdict, dec, issue, "base"):
        return
    results = {r["rule"]: r["result"] for r in verdict["rules"]}
    wanted = {rule: "hit" if rule in hits else "clean" for rule in RULES if not (skipped and rule in CHILD_RULES)}
    T.check(f"{name}: results", results == wanted, f"got {results}, wanted {wanted}")
    T.check(f"{name}: child rules skipped", (verdict["not_evaluated"] == CHILD_RULES) == skipped)
    if contains:
        reasons = [reason for r in verdict["rules"] for reason in r["reasons"]]
        T.check(f"{name}: reason names {contains!r}", any(contains in r for r in reasons), f"reasons {reasons}")


def check_clean(sb: Sandbox) -> None:
    case(sb, "clean base", decomposition())
    case(sb, "clean flow-form children", decomposition(child_yaml([child("a"), child("b", depends_on=["a"])])))


def check_children_block(sb: Sandbox) -> None:
    hit = {"hits": ["children-block"], "skipped": True}
    case(sb, "no children section", decomposition(block="## Kids\n\n```yaml\n" + CHILDREN + "```\n"), **hit)
    case(sb, "no fence", decomposition(block="## Children\n\n" + CHILDREN), **hit)
    case(sb, "tilde fence", decomposition(block="## Children\n\n~~~yaml\n" + CHILDREN + "~~~\n"), **hit)
    case(sb, "bare yaml opener", decomposition(block="## Children\n\nyaml\n" + CHILDREN), **hit)
    # the open fence runs to the end of the document, so it swallows '## Coverage' too
    unclosed = decomposition(block="## Children\n\n```yaml\n" + CHILDREN)
    case(sb, "unclosed fence", unclosed, hits=["children-block", "coverage"], skipped=True, contains="not closed")
    spaced = decomposition(block="## Children\n\n```yaml  \n" + CHILDREN + "```\t\n")
    case(sb, "fence lines with trailing whitespace", spaced)
    case(sb, "unparsable yaml", decomposition("children: [\n  - key: a\n"), **hit)
    case(sb, "multi-document yaml", decomposition(CHILDREN + "---\n" + CHILDREN), **hit)
    case(sb, "children empty", decomposition("children: []\n"), **hit)
    case(sb, "children not a list", decomposition("children:\n  key: a\n"), **hit)
    case(sb, "no children key", decomposition("kids: []\n"), **hit)
    case(sb, "child not a mapping", decomposition("children:\n  - floor\n"), **hit)
    dup = child_yaml([child("a"), child("a")])
    case(sb, "duplicate key", decomposition(dup), contains="'a'", **hit)
    case(sb, "missing key", decomposition(child_yaml([child("a"), child(DROP)])), **hit)
    case(sb, "missing depends_on", decomposition(child_yaml([child("a", depends_on=DROP)])), **hit)
    case(sb, "depends_on not strings", decomposition(child_yaml([child("a"), child("b", depends_on=[1])])), **hit)


def check_child_rules(sb: Sandbox) -> None:
    case(sb, "plan child", decomposition(child_yaml([child("a"), child("b", kind="plan", component=DROP)])))
    case(sb, "kind design", decomposition(child_yaml([child("a", kind="design")])), ["child-kind"], contains="design")
    case(sb, "kind missing", decomposition(child_yaml([child("a", kind=DROP)])), ["child-kind"])
    case(sb, "component missing", decomposition(child_yaml([child("a", component=DROP)])), ["child-component"])
    case(
        sb,
        "component empty",
        decomposition(child_yaml([child("a", component="")])),
        ["child-component", "known-components"],
    )
    case(
        sb, "component list", decomposition(child_yaml([child("a", component=["agents", "docs"])])), ["child-component"]
    )
    case(
        sb,
        "component list of one",
        decomposition(child_yaml([child("a", component=["agents"])])),
        ["child-component"],
        contains="component is a list of 1",
    )
    case(sb, "component web known at base", decomposition(child_yaml([child("a", component="web")])))
    case(
        sb,
        "component extra unknown at base",
        decomposition(child_yaml([child("a", component="extra")])),
        ["known-components"],
        contains="extra",
    )
    case(
        sb,
        "plan child with unknown component",
        decomposition(child_yaml([child("a", kind="plan", component="nope")])),
        ["known-components"],
    )


def check_dependencies(sb: Sandbox) -> None:
    deps = [child("a"), child("b", depends_on=["a"]), child("c", depends_on=["a", "b"])]
    case(sb, "diamond dependencies", decomposition(child_yaml(deps)))
    case(
        sb,
        "non-sibling depends_on",
        decomposition(child_yaml([child("a"), child("b", depends_on=["zzz"])])),
        ["depends-on-siblings"],
        contains="zzz",
    )
    case(
        sb,
        "two-child cycle",
        decomposition(child_yaml([child("a", depends_on=["b"]), child("b", depends_on=["a"])])),
        ["no-cycles"],
        contains="a, b, a",
    )
    case(
        sb,
        "self-dependency",
        decomposition(child_yaml([child("a"), child("b", depends_on=["b"])])),
        ["no-cycles"],
        contains="b, b",
    )


def check_coverage(sb: Sandbox) -> None:
    hit = ["coverage"]
    case(sb, "missing coverage row", decomposition(coverage=COVERAGE[:2]), hit, contains="criterion 3")
    rows = COVERAGE[:2] + ["| 3. It rolls out |  |"]
    case(sb, "empty covered-by cell", decomposition(coverage=rows), hit, contains="criterion 3")
    reworded = ["| 1. A verdict is written | `floor` |", "| 2) Read by the decision | `decision` |", COVERAGE[2]]
    case(sb, "reworded rows", decomposition(coverage=reworded))
    paren = CRITERIA.replace("1.", "1)").replace("2.", "2)").replace("3.", "3)")
    case(sb, "criteria in N) form", decomposition(), issue_text=issue_document(paren))
    case(sb, "bullet criteria", decomposition(), hit, issue_text=issue_document("- one\n- two\n"))
    case(sb, "no criteria section", decomposition(), hit, issue_text=issue_document(heading=""))
    case(sb, "row 10 does not cover 1", decomposition(coverage=["| 10. x | a |"] + COVERAGE[1:]), hit)
    no_table = decomposition().replace("| Acceptance criterion of the issue | Covered by |\n| --- | --- |\n", "")
    case(sb, "no coverage table header", no_table, hit)
    no_section = decomposition().replace("## Coverage\n", "## Mapping\n")
    case(sb, "no coverage section", no_section, hit)


def check_base_ref(sb: Sandbox) -> None:
    worktree = (sb.repo / CONFIG_PATH).read_text(encoding="utf-8")
    T.check("base ref: worktree and HEAD configs have extra, not web", "extra" in worktree and "web" not in worktree)
    T.check("base ref: the worktree config is uncommitted", sb.git("status", "--porcelain", CONFIG_PATH) != "")


def check_unusable(sb: Sandbox) -> None:
    dec = sb.write("unusable decomposition", decomposition())
    issue = sb.write("unusable issue", issue_document())
    not_utf8 = sb.write("not utf-8", b"# Decomposition\n\n\xff\xfe\n")
    good = ["--decomposition", dec, "--issue-document", issue]
    cases = [
        ("no arguments", []),
        ("unknown argument", good + ["--base", "base", "--extra", "x"]),
        ("argument twice", good + ["--base", "base", "--base", "base"]),
        ("missing argument", good),
        ("missing value", good + ["--base"]),
        ("empty value", ["--decomposition", "", "--issue-document", issue, "--base", "base"]),
        ("value an argument name", ["--decomposition", "--base", "--issue-document", issue, "--base", "base"]),
        ("base starting with -", good + ["--base", "-HEAD"]),
        ("missing decomposition", ["--decomposition", "cases/none.md", "--issue-document", issue, "--base", "base"]),
        ("unreadable issue", ["--decomposition", dec, "--issue-document", "cases", "--base", "base"]),
        ("decomposition not UTF-8", ["--decomposition", not_utf8, "--issue-document", issue, "--base", "base"]),
        ("issue not UTF-8", ["--decomposition", dec, "--issue-document", not_utf8, "--base", "base"]),
        ("unresolvable base", good + ["--base", "no-such-ref"]),
        ("base without config", good + ["--base", sb.commits["noconfig"]]),
        ("components not a mapping", good + ["--base", sb.commits["badcomponents"]]),
    ]
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
    proc, verdict = sb.floor(good + ["--base", "base"], env=dict(sb.env, PATH=str(bindir)))
    T.check(
        "unusable yq missing: exit 2, no verdict",
        proc.returncode == 2 and verdict is None,
        f"exit {proc.returncode}: {proc.stderr.strip()}",
    )


def check_no_bytecode() -> None:
    for directory in (HERE, PLAN_FLOOR_DIR):
        T.check(f"no __pycache__ in {directory.name}", not (directory / "__pycache__").exists())


def main() -> int:
    for tool in ("git", "yq"):
        if shutil.which(tool) is None:
            print(f"selftest.py: {tool} is not on PATH", file=sys.stderr)
            return 2
    sys.dont_write_bytecode = True
    with tempfile.TemporaryDirectory(prefix="msw-decomposition-floor-") as tmp:
        sb = Sandbox(Path(tmp))
        sb.build()
        check_clean(sb)
        check_children_block(sb)
        check_child_rules(sb)
        check_dependencies(sb)
        check_coverage(sb)
        check_base_ref(sb)
        check_unusable(sb)
    check_no_bytecode()
    print(f"{T.passed} passed, {T.failed} failed")
    return 0 if T.failed == 0 else 1


if __name__ == "__main__":
    sys.exit(main())
