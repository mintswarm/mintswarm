#!/usr/bin/env python3
"""Offline self-test of decision.py, the landing decision.

    decision-selftest.py

Needs `git` and `yq` on PATH; nothing else, and no network. It works in a
temporary directory with its own HOME and TMPDIR, no system or global git
config and a fixed identity, and runs `git init` on a throwaway repository
with no remote. Its base commits hold the checkout's .mintswarm/config.yaml,
read with `yq -o=json` and committed as JSON text at the same path, with
`pipeline.landing.mode` set to `always`, `shadow`, `judged` and `never`, one
`judged` variant with `min_confidence: 90` and one with an
`agents.stages.review` override whose `effort` is not a string; each base
has a head commit on top. Landing floor verdicts, facts files and landing
documents are written here as fixtures, bound to those commits, and
decision.py runs as `sys.executable -I decision.py ...` with the repository
as its working directory.

Checks:

- the matrix: each of the four modes against a floor that is clean, hit
  or missing (floor exit 2) and a landing document at the threshold, below
  it, `blocked` with a hard trigger, unusable (a `Risk:` line) and missing:
  `would_be` is judgement.py's judged_decision for the same inputs,
  `effective` follows the mode, the exit code is 0 exactly for an
  `auto` effective decision, `acts` is false, and a comment is printed in
  every mode but `always`; under `shadow`, the record's mode reason and the
  comment state that the decision changes nothing;
- reasons: every hit rule with its reasons, the band, confidence and
  decisive reason, each hard trigger, a confidence below min_confidence
  and an unusable layer's problems;
- threshold edges: with `min_confidence: 90`, 90 is `auto` and 89 `human`,
  under `judged`;
- bindings: a verdict whose plan, facts, base, head or merge base,
  component or mode is not the run's, whose keys are out of order, whose
  verdict disagrees with its rules, whose not_evaluated does not follow its
  footprint, or that is not strict JSON; facts of another head, base or PR,
  or with another tool or version; and a landing document that is not
  UTF-8: each makes its layer unusable with a fixed problem and gives
  `human`, also under `judged` and `never`;
- a footprint hit with the diff rules not evaluated is a usable hit, listed
  in the comment;
- approval: a human approver with no kickoff record and one with a kickoff
  decision, floor and assessment give the same `would_be`; the approver,
  the approval time and the kickoff records are in `inputs.approval`;
- the record: its keys and its layers' keys in order, the subject's PR,
  issue, commits, plan sha256 and config commit, the sha256 of the verdict,
  the facts and the landing document, and the harness, model and effort of
  implement, review and landing, from the real config and from the
  override (null for a non-string effort); the landing stage's model
  differs from implement's and review's;
- the comment: its heading, the PR and commits, the would-be and effective
  decisions, one row per floor hit, the Confidence row with the band's
  name, score and decisive reason, the approval as inputs and the agents,
  from the real config;
  hostile text with backticks, newlines, an at-sign and the steer marker
  in the reason, a doubt, a floor reason and the approver stays in code
  spans, and no line is the steer marker;
- unusable input: bad arguments, an unknown component, an unreadable plan,
  an unresolvable head, a base without a config and an invalid config exit
  2 with no record, even a stale one; every normal run replaces a stale
  record;
- shape: the record's top-level keys are plan-approval/decision.py's, with
  `judged_decision` and `decision` named `would_be` and `effective`, and
  `acts`, `inputs` and `agents` added;
- the checkout: `pipeline.landing.mode` is `shadow`, and gates.run runs this
  self-test as `landing-decision-selftest`, after
  `landing-judgement-selftest`;
- hygiene: no object store is left in TMPDIR, no __pycache__ next to the
  scripts, and decision.py and this file import only the standard library.

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
from pathlib import Path

sys.dont_write_bytecode = True  # leave no __pycache__ in the source tree

HERE = Path(__file__).resolve().parent
DECISION = HERE / "decision.py"
JUDGEMENT = HERE / "judgement.py"
LANDING_FLOOR = HERE / "floor.py"
PLAN_DECISION = HERE.parent / "plan-approval" / "decision.py"
ROOT = HERE.parents[2]
CONFIG = ROOT / ".mintswarm" / "config.yaml"
CONFIG_PATH = ".mintswarm/config.yaml"
RECORD = Path(".mintswarm/job/outputs/landing.json")
VERDICT = Path(".mintswarm/job/outputs/landing-floor.json")
STEER = "<!-- mintswarm: steer -->"
COMPONENT = "agents"
PR = 7
ISSUE = 50
MODES = ("always", "shadow", "judged", "never")
RECORD_KEYS = [
    "tool",
    "version",
    "approval",
    "subject",
    "mode",
    "min_confidence",
    "layers",
    "would_be",
    "effective",
    "reasons",
    "acts",
    "inputs",
    "agents",
]
SUBJECT_KEYS = ["pr", "issue", "component", "plan", "base", "head", "config"]
FLOOR_LAYER_KEYS = ["status", "problems", "path", "sha256", "verdict", "rules", "not_evaluated"]
JUDGEMENT_LAYER_KEYS = [
    "status",
    "problems",
    "stage",
    "path",
    "sha256",
    "confidence",
    "band",
    "hard_triggers",
    "doubts",
    "reasons",
]
DOUBT = {"statement": "a helper may be unused", "consequence": "dead code lands", "resolution": "grep for callers"}


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


def load_module(name: str, path: Path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


J = load_module("msw_landing_judgement_for_decision_selftest", JUDGEMENT)
F = load_module("msw_landing_floor_for_decision_selftest", LANDING_FLOOR)


def sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def text(n: int, tag: str) -> bytes:
    return "".join(f"{tag} {i}\n" for i in range(n)).encode("utf-8")


BASE_TREE = {"README.md": text(2, "readme"), "docs/guide.md": text(4, "guide")}


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
                "GIT_AUTHOR_NAME": "landing decision selftest",
                "GIT_AUTHOR_EMAIL": "selftest@example.invalid",
                "GIT_COMMITTER_NAME": "landing decision selftest",
                "GIT_COMMITTER_EMAIL": "selftest@example.invalid",
                "GIT_AUTHOR_DATE": "2026-01-01T00:00:00Z",
                "GIT_COMMITTER_DATE": "2026-01-01T00:00:00Z",
            }
        )
        self.store_tmp = tmp / "tmpdir"  # TMPDIR of every run: the object stores go here
        self.store_tmp.mkdir()
        env["TMPDIR"] = str(self.store_tmp)
        self.env = env
        self.repo.mkdir()
        self.cases.mkdir()
        self.git("init", "-q", "-b", "master")
        self.counter = 0
        self.trees = {}

    def git(self, *args: str, data: bytes = None, env: dict = None) -> str:
        proc = subprocess.run(["git", *args], cwd=self.repo, env=env or self.env, input=data, capture_output=True)
        if proc.returncode != 0:
            raise RuntimeError(f"git {' '.join(args)} failed: {proc.stderr.decode(errors='replace').strip()}")
        return proc.stdout.decode("utf-8").strip()

    def commit(self, files: dict, parents: tuple = (), message: str = "case") -> str:
        self.counter += 1
        records = b""
        for path, data in files.items():
            sha = self.git("hash-object", "-w", "--stdin", data=data)
            records += f"100644 {sha}\t".encode("utf-8") + path.encode("utf-8") + b"\0"
        env = dict(self.env, GIT_INDEX_FILE=str(self.tmp / f"index-{self.counter}"))
        self.git("update-index", "-z", "--index-info", data=records, env=env)
        tree = self.git("write-tree", env=env)
        os.unlink(self.tmp / f"index-{self.counter}")
        argv = ["commit-tree", tree]
        for parent in parents:
            argv += ["-p", parent]
        commit = self.git(*argv, "-m", f"{message} ({self.counter})")
        self.trees[commit] = dict(files)
        return commit

    def write(self, name: str, data: bytes) -> str:
        self.counter += 1
        slug = re.sub(r"[^A-Za-z0-9.-]+", "-", name)[:40]
        rel = f"cases/{self.counter:03d}-{slug}"
        (self.repo / rel).write_bytes(data)
        return rel

    def run(self, args: list):
        """Run decision.py over a stale record; (process, record or None)."""
        stale = self.repo / RECORD
        stale.parent.mkdir(parents=True, exist_ok=True)
        stale.write_text('{"stale": true}\n', encoding="utf-8")
        proc = subprocess.run(
            [sys.executable, "-I", str(DECISION), *args],
            cwd=self.repo,
            env=self.env,
            capture_output=True,
            text=True,
        )
        record = None
        if stale.exists():
            try:
                record = json.loads(stale.read_text(encoding="utf-8"))
            except ValueError:
                record = "unparsable"
        return proc, record


def yq_json(path: Path) -> dict:
    proc = subprocess.run(["yq", "-o=json", ".", str(path)], capture_output=True, text=True)
    if proc.returncode != 0:
        raise RuntimeError(f"yq failed on {path}: {proc.stderr.strip()}")
    return json.loads(proc.stdout)


def as_json(config: dict) -> bytes:
    return (json.dumps(config, indent=2) + "\n").encode("utf-8")


# --- fixtures -------------------------------------------------------------


def envelope(band: str, confidence: int, triggers=None, doubts=None, reason="the change does what the plan says"):
    if triggers is None:
        triggers = ["criterion-not-met"] if band == "blocked" else []
    if doubts is None:
        doubts = [DOUBT] if band == "likely-safe" else []
    return {"confidence": confidence, "band": band, "hard_triggers": triggers, "reason": reason, "doubts": doubts}


def document(env: dict, extra: str = "", summary: str = None) -> bytes:
    if summary is None:
        summary = f"{env['confidence']} confidence: {' '.join(str(env['reason']).split())}"
    fenced = json.dumps(env, indent=2, ensure_ascii=False)
    return (
        f"# Landing: a change\n\n{summary}\n\n## Envelope\n\n```json\n{fenced}\n```\n\n"
        f"## Acceptance criteria\n## Conformance\n## Notes\nNone.\n{extra}"
    ).encode("utf-8")


JUDGEMENTS = {
    "at": document(envelope("safe", 95)),
    "below": document(envelope("likely-safe", 94)),
    "blocked": document(envelope("blocked", 0, triggers=["criterion-not-met", "credentials"])),
    "unusable": document(envelope("safe", 97), extra="Risk: low\n"),
    "missing": None,
}


def clean_facts(base: str, head: str, **changes) -> dict:
    facts = {
        "tool": "mintswarm-landing-facts",
        "version": 3,
        "pr": PR,
        "issue": ISSUE,
        "head": head,
        "base": base,
        "fetched_at": "2026-01-01T00:00:00Z",
        "checks": [],
        "gate_rounds": [],
        "review_rounds": [],
        "steers": [],
        "review_requests": [],
        "approval": {"approver": "maintainer", "approved_at": "2026-01-01T00:00:00Z", "approval_ref": None},
    }
    facts.update(changes)
    return facts


def facts_bytes(facts: dict) -> bytes:
    return (json.dumps(facts, indent=2, ensure_ascii=False) + "\n").encode("utf-8")


def rules_for(hits: dict, skip_diff: bool = False) -> tuple:
    rules = []
    skipped = list(F.DIFF_RULES) if skip_diff else []
    for name in F.RULES:
        if name in skipped:
            continue
        reasons = hits.get(name, [])
        rules.append({"rule": name, "result": "hit" if reasons else "clean", "reasons": reasons})
    return rules, skipped


def verdict_for(ctx: dict, hits: dict = None, skip_diff: bool = False, **changes) -> dict:
    rules, skipped = rules_for(hits or {}, skip_diff)
    verdict = {
        "tool": "mintswarm-landing-floor",
        "version": 1,
        "plan": {"path": ctx["plan"], "sha256": ctx["plan_sha"]},
        "facts": {"path": ctx["facts"], "sha256": ctx["facts_sha"], "checks": []},
        "component": COMPONENT,
        "base": {"ref": ctx["base"], "commit": ctx["base"]},
        "head": {"ref": ctx["head"], "commit": ctx["head"]},
        "merge_base": ctx["base"],
        "config": CONFIG_PATH,
        "mode": ctx["mode"],
        "verdict": "hit" if any(r["result"] == "hit" for r in rules) else "clean",
        "rules": rules,
        "not_evaluated": skipped,
    }
    verdict.update(changes)
    return verdict


HIT = {"steers": ["pr-comment by someone at 2026-01-02T00:00:00Z is a steer: https://example.invalid/1"]}


class Runner:
    def __init__(self, sb: Sandbox, bases: dict):
        self.sb = sb
        self.bases = bases  # name -> (base, head, mode, min_confidence)
        self.plan = sb.write("plan.md", b"# Plan: a case\n\nGenerated by decision-selftest.py.\n")
        self.plan_sha = sha256((sb.repo / self.plan).read_bytes())

    def context(self, name: str, facts=None) -> dict:
        base, head, mode, min_confidence = self.bases[name]
        if facts is None:
            facts = facts_bytes(clean_facts(base, head))
        elif isinstance(facts, dict):
            facts = facts_bytes(facts)
        path = self.sb.write("facts.json", facts)
        return {
            "name": name,
            "base": base,
            "head": head,
            "mode": mode,
            "min_confidence": min_confidence,
            "plan": self.plan,
            "plan_sha": self.plan_sha,
            "facts": path,
            "facts_sha": sha256(facts),
        }

    def run(self, ctx: dict, verdict, landing, argv: list = None):
        """verdict: a dict, raw bytes or None (missing); landing: bytes or None (missing)."""
        target = self.sb.repo / VERDICT
        target.parent.mkdir(parents=True, exist_ok=True)
        target.unlink(missing_ok=True)
        verdict_bytes = None
        if verdict is not None:
            verdict_bytes = verdict if isinstance(verdict, bytes) else facts_bytes(verdict)
            target.write_bytes(verdict_bytes)
        landing_path = self.sb.write("landing.md", landing) if landing is not None else "cases/no-landing.md"
        args = argv or [
            "--pr",
            str(PR),
            "--component",
            COMPONENT,
            "--plan",
            ctx["plan"],
            "--base",
            ctx["base"],
            "--head",
            ctx["head"],
            "--facts",
            ctx["facts"],
            "--landing",
            landing_path,
        ]
        proc, record = self.sb.run(args)
        return proc, record, verdict_bytes, landing_path


def judged_layer(landing) -> dict:
    if landing is None:
        return {"status": "unusable", "hard_triggers": [], "confidence": None}
    return J.judgement(landing)


def expected_effective(mode: str, would_be: str, floor_verdict) -> str:
    if mode == "judged":
        return would_be
    if mode == "never":
        return "auto" if floor_verdict == "clean" else "human"
    return "human"


def usable_record(record) -> bool:
    return isinstance(record, dict) and list(record) == RECORD_KEYS


# --- checks ---------------------------------------------------------------


def expected_agents(real: dict) -> dict:
    """The harness, model and effort of implement, review and landing: agents.stages over agents.default."""
    default = real["agents"]["default"]
    stages = real["agents"].get("stages", {})
    return {
        stage: {k: stages.get(stage, {}).get(k, default[k]) for k in ("harness", "model", "effort")}
        for stage in ("implement", "review", "landing")
    }


def check_matrix(r: Runner) -> None:
    for mode in MODES:
        for floor_name in ("clean", "hit", "missing"):
            for judgement_name, landing in JUDGEMENTS.items():
                name = f"matrix {mode}, floor {floor_name}, judgement {judgement_name}"
                ctx = r.context(mode)
                verdict = None if floor_name == "missing" else verdict_for(ctx, HIT if floor_name == "hit" else {})
                proc, record, _, _ = r.run(ctx, verdict, landing)
                if not T.check(f"{name}: a record", usable_record(record), proc.stderr.strip()):
                    continue
                floor_verdict = None if verdict is None else verdict["verdict"]
                would_be = J.judged_decision(floor_verdict, judged_layer(landing), ctx["min_confidence"])
                want_auto = floor_name == "clean" and judgement_name == "at"
                effective = expected_effective(mode, would_be, floor_verdict)
                problems = []
                if (would_be == "auto") != want_auto:
                    problems.append(f"judged_decision gives {would_be}")
                if record["would_be"] != would_be:
                    problems.append(f"would_be {record['would_be']}, not {would_be}")
                if record["effective"] != effective:
                    problems.append(f"effective {record['effective']}, not {effective}")
                if proc.returncode != (0 if effective == "auto" else 1):
                    problems.append(f"exit {proc.returncode}")
                if record["acts"] is not False:
                    problems.append("acts is not false")
                if record["layers"]["floor"]["status"] != ("unusable" if floor_name == "missing" else "usable"):
                    problems.append(f"floor layer {record['layers']['floor']['status']}")
                if bool(proc.stdout) != (mode != "always"):
                    problems.append("a comment printed in always, or none in another mode")
                if mode == "shadow":
                    if "changes nothing" not in record["reasons"][0] or "changes nothing" not in proc.stdout:
                        problems.append("shadow does not state that it changes nothing")
                    if not proc.stdout.startswith("### Shadow record: landing\n"):
                        problems.append("shadow heading")
                elif mode != "always" and not proc.stdout.startswith("### Landing decision\n"):
                    problems.append("heading")
                T.check(name, not problems, "; ".join(problems))


def check_reasons(r: Runner) -> None:
    ctx = r.context("judged")
    hits = {"steers": HIT["steers"], "review": ["there is no review round", "another reason"]}
    proc, record, _, _ = r.run(ctx, verdict_for(ctx, hits), JUDGEMENTS["blocked"])
    reasons = record["reasons"] if usable_record(record) else []
    want = [
        "floor: hit on review, steers",
        "floor hit review: there is no review round; another reason",
        "floor hit steers: " + HIT["steers"][0],
        "judgement: blocked 0: the change does what the plan says",
        "judgement: hard trigger criterion-not-met",
        "judgement: hard trigger credentials",
        "would_be: human",
        "effective: human",
    ]
    T.check("reasons: hit rules and hard triggers", all(w in reasons for w in want), json.dumps(reasons))
    proc, record, _, _ = r.run(ctx, verdict_for(ctx), JUDGEMENTS["below"])
    reasons = record["reasons"] if usable_record(record) else []
    T.check(
        "reasons: clean floor and a confidence below min_confidence",
        f"floor: clean, {len(F.RULES)} of {len(F.RULES)} rules" in reasons
        and "judgement: confidence 94 below min_confidence 95" in reasons
        and reasons[0].startswith("mode judged:"),
        json.dumps(reasons),
    )
    proc, record, _, _ = r.run(ctx, None, JUDGEMENTS["unusable"])
    reasons = record["reasons"] if usable_record(record) else []
    T.check(
        "reasons: unusable layers give their problems",
        "floor: the floor verdict is missing" in reasons
        and "judgement: the landing document has a line starting with 'Risk:'" in reasons,
        json.dumps(reasons),
    )


def check_threshold(r: Runner) -> None:
    for confidence, want in ((90, "auto"), (89, "human")):
        band = "likely-safe" if confidence >= 80 else "needs-review"
        ctx = r.context("judged-90")
        proc, record, _, _ = r.run(ctx, verdict_for(ctx), document(envelope(band, confidence, doubts=[DOUBT])))
        ok = usable_record(record) and record["min_confidence"] == 90 and record["effective"] == want
        T.check(f"threshold: {confidence} against min_confidence 90 is {want}", ok, proc.stderr.strip())
        if confidence == 90:
            T.check("threshold: auto exits 0", proc.returncode == 0, str(proc.returncode))


def check_bindings(r: Runner) -> None:
    good = JUDGEMENTS["at"]
    other = r.bases["shadow"][0]

    def edit(change):
        return lambda ctx: change(verdict_for(ctx), ctx)

    def setting(key, value):
        def change(v, ctx):
            v[key] = value(ctx) if callable(value) else value
            return v

        return change

    cases = [
        (
            "verdict plan",
            edit(setting("plan", lambda c: {"path": c["plan"], "sha256": "0" * 64})),
            "the floor verdict's plan.sha256 does not match the plan",
        ),
        (
            "verdict facts",
            edit(setting("facts", lambda c: {"path": c["facts"], "sha256": "0" * 64, "checks": []})),
            "the floor verdict's facts.sha256 does not match the facts file",
        ),
        (
            "verdict base",
            edit(setting("base", {"ref": "x", "commit": other})),
            "the floor verdict's base.commit does not match the base",
        ),
        (
            "verdict head",
            edit(setting("head", {"ref": "x", "commit": other})),
            "the floor verdict's head.commit does not match the head",
        ),
        (
            "verdict merge base",
            edit(setting("merge_base", other)),
            "the floor verdict's merge_base does not match the merge base",
        ),
        ("verdict mode", edit(setting("mode", "always")), "the floor verdict's mode does not match the base config"),
        (
            "verdict component",
            edit(setting("component", "docs")),
            "the floor verdict's component does not match --component",
        ),
        (
            "verdict key order",
            lambda ctx: dict(reversed(list(verdict_for(ctx).items()))),
            "the floor verdict's keys are not landing/floor.py's, in order",
        ),
        (
            "verdict disagrees with its rules",
            lambda ctx: verdict_for(ctx, HIT, verdict="clean"),
            "the floor verdict's verdict does not agree with its rules",
        ),
        (
            "verdict skips the diff rules without a footprint hit",
            lambda ctx: verdict_for(ctx, skip_diff=True),
            "the floor verdict's not_evaluated does not follow its footprint result",
        ),
        (
            "verdict tool",
            edit(setting("tool", "mintswarm-plan-floor")),
            "the floor verdict's tool or version is not landing/floor.py's",
        ),
        (
            "verdict not strict JSON",
            lambda ctx: facts_bytes(verdict_for(ctx)).replace(b'"tool"', b'"tool": "x", "tool"', 1),
            "the floor verdict is not strict JSON",
        ),
    ]
    for name, make, problem in cases:
        for mode in ("judged", "never"):
            ctx = r.context(mode)
            proc, record, _, _ = r.run(ctx, make(ctx), good)
            ok = (
                usable_record(record)
                and record["layers"]["floor"]["status"] == "unusable"
                and problem in record["layers"]["floor"]["problems"]
                and record["would_be"] == "human"
                and record["effective"] == "human"
                and proc.returncode == 1
            )
            T.check(f"bindings: {name} under {mode}", ok, json.dumps(record)[:400] if record else proc.stderr)

    base, head = r.bases["judged"][0], r.bases["judged"][1]
    facts_cases = [
        ("facts head", clean_facts(base, other), "the facts file's head does not match --head"),
        ("facts base", clean_facts(other, head), "the facts file's base does not match --base"),
        ("facts pr", clean_facts(base, head, pr=PR + 1), "the facts file's pr does not match --pr"),
        (
            "facts tool",
            clean_facts(base, head, tool="mintswarm-landing-floor"),
            "the facts file's tool or version is not collect-facts.py's",
        ),
        (
            "facts version",
            clean_facts(base, head, version=2),
            "the facts file's tool or version is not collect-facts.py's",
        ),
        ("facts not JSON", b"{not json", "the facts file is not strict JSON"),
    ]
    for name, facts, problem in facts_cases:
        for mode in ("judged", "never"):
            ctx = r.context(mode, facts)
            proc, record, _, _ = r.run(ctx, verdict_for(ctx), good)
            ok = (
                usable_record(record)
                and record["layers"]["floor"]["status"] == "unusable"
                and problem in record["layers"]["floor"]["problems"]
                and record["effective"] == "human"
                and record["subject"]["issue"] is None
                and record["inputs"]["approval"]["approver"] is None
            )
            T.check(f"bindings: {name} under {mode}", ok, json.dumps(record)[:400] if record else proc.stderr)

    ctx = r.context("judged")
    proc, record, _, _ = r.run(ctx, verdict_for(ctx), b"# Landing\n\xff\xfe\n")
    ok = (
        usable_record(record)
        and record["layers"]["judgement"]["status"] == "unusable"
        and record["layers"]["judgement"]["problems"] == ["the landing document is not UTF-8"]
        and record["effective"] == "human"
    )
    T.check("bindings: a landing document that is not UTF-8", ok, proc.stderr.strip())

    ctx = r.context("never")
    proc, record, _, _ = r.run(ctx, verdict_for(ctx, {"footprint": ["the footprint is missing"]}, skip_diff=True), good)
    ok = (
        usable_record(record)
        and record["layers"]["floor"]["status"] == "usable"
        and record["layers"]["floor"]["not_evaluated"] == list(F.DIFF_RULES)
        and record["effective"] == "human"
        and "- **Not evaluated:** `undeclared-files`, `undeclared-new-files`, `size`" in proc.stdout.splitlines()
    )
    T.check("bindings: a footprint hit with the diff rules not evaluated is a usable hit", ok, proc.stdout)


def check_approval(r: Runner) -> None:
    base, head = r.bases["judged"][0], r.bases["judged"][1]
    human = {"approver": "zmij", "approved_at": "2026-11-20T10:00:00Z", "approval_ref": "https://example.invalid/c/1"}
    kickoff = dict(
        human,
        decision={"path": "decision.json", "sha256": "1" * 64, "record": {"tool": "mintswarm-approval-decision"}},
        kickoff_floor={"path": "floor.json", "sha256": "2" * 64, "record": {"tool": "mintswarm-plan-floor"}},
        assessment={"path": "assessment.md", "sha256": "3" * 64, "confidence": 40, "band": "uncertain"},
    )
    results = {}
    for name, approval in (("human, no kickoff record", human), ("with kickoff records", kickoff)):
        ctx = r.context("judged", clean_facts(base, head, approval=approval))
        proc, record, _, _ = r.run(ctx, verdict_for(ctx), JUDGEMENTS["at"])
        results[name] = record
        inputs = record["inputs"]["approval"] if usable_record(record) else {}
        ok = (
            inputs.get("approver") == "zmij"
            and inputs.get("approved_at") == "2026-11-20T10:00:00Z"
            and inputs.get("approval_ref") == "https://example.invalid/c/1"
        )
        T.check(f"approval: {name}: approver and time are inputs", ok, json.dumps(inputs))
        if mode_line := [line for line in proc.stdout.splitlines() if line.startswith("- **Approval")]:
            T.check(
                f"approval: {name}: in the comment as inputs, not gates",
                mode_line[0]
                == "- **Approval (inputs, not gates):** approver `zmij`, approved at `2026-11-20T10:00:00Z`",
                mode_line[0],
            )
    a, b = results["human, no kickoff record"], results["with kickoff records"]
    T.check(
        "approval: a human approval with no kickoff record leaves would_be as it is",
        usable_record(a) and usable_record(b) and a["would_be"] == b["would_be"] == "auto",
        f"{a and a.get('would_be')} {b and b.get('would_be')}",
    )
    T.check(
        "approval: kickoff records are listed, none without them",
        usable_record(a)
        and usable_record(b)
        and a["inputs"]["approval"]["kickoff_records"] == []
        and b["inputs"]["approval"]["kickoff_records"]
        == [
            {"kind": "decision", "path": "decision.json", "sha256": "1" * 64},
            {"kind": "kickoff_floor", "path": "floor.json", "sha256": "2" * 64},
            {"kind": "assessment", "path": "assessment.md", "sha256": "3" * 64},
        ],
        json.dumps(b["inputs"]["approval"] if usable_record(b) else None),
    )


def check_record(r: Runner, real: dict) -> None:
    ctx = r.context("shadow")
    proc, record, verdict_bytes, landing_path = r.run(ctx, verdict_for(ctx, HIT), JUDGEMENTS["below"])
    if not T.check("record: written with its keys in order", usable_record(record), proc.stderr.strip()):
        return
    subject = record["subject"]
    T.check("record: subject keys in order", list(subject) == SUBJECT_KEYS, json.dumps(list(subject)))
    T.check(
        "record: tool, version and approval",
        (record["tool"], record["version"], record["approval"]) == ("mintswarm-landing-decision", 1, "landing"),
    )
    T.check(
        "record: subject",
        subject
        == {
            "pr": PR,
            "issue": ISSUE,
            "component": COMPONENT,
            "plan": {"path": ctx["plan"], "sha256": ctx["plan_sha"]},
            "base": {"ref": ctx["base"], "commit": ctx["base"]},
            "head": {"ref": ctx["head"], "commit": ctx["head"]},
            "config": {"path": CONFIG_PATH, "commit": ctx["base"]},
        },
        json.dumps(subject),
    )
    floor_ = record["layers"]["floor"]
    judged = record["layers"]["judgement"]
    T.check("record: floor layer keys in order", list(floor_) == FLOOR_LAYER_KEYS, json.dumps(list(floor_)))
    T.check("record: judgement layer keys in order", list(judged) == JUDGEMENT_LAYER_KEYS, json.dumps(list(judged)))
    T.check(
        "record: the sha256 of the verdict, the landing document and the facts",
        floor_["sha256"] == sha256(verdict_bytes)
        and floor_["path"] == str(VERDICT)
        and judged["sha256"] == sha256(JUDGEMENTS["below"])
        and judged["path"] == landing_path
        and judged["stage"] == "landing"
        and record["inputs"]["facts"] == {"path": ctx["facts"], "sha256": ctx["facts_sha"]},
        json.dumps([floor_["sha256"], judged["sha256"], record["inputs"]["facts"]]),
    )
    T.check(
        "record: the judgement layer's envelope",
        judged["confidence"] == 94
        and judged["band"] == "likely-safe"
        and judged["doubts"] == [DOUBT]
        and judged["reasons"] == ["the change does what the plan says"],
        json.dumps(judged),
    )
    default = real["agents"]["default"]
    want = expected_agents(real)
    T.check("record: agents from the real config", record["agents"] == want, json.dumps(record["agents"]))
    agents = record["agents"]
    T.check(
        "record: the landing stage's model differs from implement's and review's",
        isinstance(agents["landing"]["model"], str)
        and agents["landing"]["model"] not in (agents["implement"]["model"], agents["review"]["model"]),
        json.dumps(agents),
    )
    T.check("record: mode and min_confidence", (record["mode"], record["min_confidence"]) == ("shadow", 95))

    ctx = r.context("override")
    proc, record, _, _ = r.run(ctx, verdict_for(ctx), JUDGEMENTS["at"])
    want = dict(want, review={"harness": default["harness"], "model": "sonnet", "effort": None})
    T.check(
        "record: agents.stages.review over agents.default, null for a non-string effort",
        usable_record(record) and record["agents"] == want,
        json.dumps(record["agents"] if usable_record(record) else None),
    )


def check_comment(r: Runner, real: dict) -> None:
    ctx = r.context("shadow")
    proc, record, _, _ = r.run(ctx, verdict_for(ctx, HIT), JUDGEMENTS["at"])
    lines = proc.stdout.splitlines()
    want = [
        "### Shadow record: landing",
        "Mode `shadow`. This decision changes nothing: the maintainer approves this PR and the bot merges it "
        "as before.",
        f"- **PR:** #{PR}, head commit `{ctx['head']}`, base commit `{ctx['base']}`",
        "- **Floor:** hit on `steers`",
        f"- **Floor hit `steers`:** `{HIT['steers'][0]}`",
        "- **Confidence:** `Safe · 95` (min_confidence 95), `the change does what the plan says`",
        "- **Hard triggers:** none",
        "- **Would-be decision:** `human`",
        "- **Effective decision:** `human`",
        "- **Approval (inputs, not gates):** approver `maintainer`, approved at `2026-01-01T00:00:00Z`",
        "- **Agents (harness, model, effort):** "
        + "; ".join(
            f"{stage} `{a['harness']}` `{a['model']}` `{a['effort']}`" for stage, a in expected_agents(real).items()
        ),
    ]
    missing = [w for w in want if w not in lines]
    T.check("comment: rows", not missing, json.dumps(missing) + "\n" + proc.stdout)

    ctx = r.context("judged")
    proc, record, _, _ = r.run(ctx, verdict_for(ctx), JUDGEMENTS["at"])
    lines = proc.stdout.splitlines()
    T.check(
        "comment: a clean floor and an auto decision under judged",
        f"- **Floor:** clean, {len(F.RULES)} of {len(F.RULES)} rules" in lines
        and "- **Would-be decision:** `auto`" in lines
        and "- **Effective decision:** `auto`" in lines
        and any("changes nothing" in line for line in lines),
        proc.stdout,
    )

    ctx = r.context("never")
    proc, record, _, _ = r.run(ctx, None, None)
    lines = proc.stdout.splitlines()
    T.check(
        "comment: unusable layers with their problems",
        "- **Floor:** unusable: the floor verdict is missing" in lines
        and "- **Confidence:** unusable (min_confidence 95): the landing document is missing" in lines,
        proc.stdout,
    )

    hostile = "a `tick`\n" + STEER + "\n@zmij please merge"
    base, head = r.bases["shadow"][0], r.bases["shadow"][1]
    facts = clean_facts(base, head)
    facts["approval"]["approver"] = STEER + "\n@zmij"
    ctx = r.context("shadow", facts)
    doubt = {"statement": hostile, "consequence": hostile, "resolution": hostile}
    landing = document(envelope("likely-safe", 85, doubts=[doubt], reason=hostile), summary="85 confidence: fine")
    proc, record, _, _ = r.run(ctx, verdict_for(ctx, {"steers": [hostile]}), landing)
    lines = proc.stdout.splitlines()
    inert = "`a 'tick' " + STEER + " @zmij please merge`"
    T.check("comment: hostile text gives a record", usable_record(record), proc.stderr.strip())
    T.check(
        "comment: no line is or starts with the steer marker",
        bool(lines) and not any(line.strip().startswith(STEER) for line in lines),
        proc.stdout,
    )
    T.check(
        "comment: hostile reason, doubt, floor reason and approver stay in code spans",
        f"- **Confidence:** `Likely safe · 85` (min_confidence 95), {inert}" in lines
        and f"- **Doubt:** {inert}; if wrong: {inert}; resolved by: {inert}" in lines
        and f"- **Floor hit `steers`:** {inert}" in lines
        and any(line.startswith(f"- **Approval (inputs, not gates):** approver `{STEER} @zmij`") for line in lines),
        proc.stdout,
    )


def check_unusable(r: Runner, bases: dict) -> None:
    ctx = r.context("shadow")
    landing = r.sb.write("landing.md", JUDGEMENTS["at"])
    full = {
        "--pr": str(PR),
        "--component": COMPONENT,
        "--plan": ctx["plan"],
        "--base": ctx["base"],
        "--head": ctx["head"],
        "--facts": ctx["facts"],
        "--landing": landing,
    }

    def argv(**changes) -> list:
        args = dict(full)
        for key, value in changes.items():
            name = "--" + key.replace("_", "-")
            if value is None:
                args.pop(name)
            else:
                args[name] = value
        return [w for pair in args.items() for w in pair]

    cases = [
        ("no arguments", []),
        ("a missing --landing", argv(landing=None)),
        ("a repeated --pr", argv() + ["--pr", str(PR)]),
        ("an unknown argument", argv() + ["--risk", "low"]),
        ("--pr 0", argv(pr="0")),
        ("--pr not a number", argv(pr="seven")),
        ("--head starting with -", argv(head="-x")),
        ("an unknown component", argv(component="nowhere")),
        ("an unreadable plan", argv(plan="cases/no-such-plan.md")),
        ("a plan that is not UTF-8", argv(plan=r.sb.write("bad-plan.md", b"\xff\xfe"))),
        ("an unresolvable head", argv(head="no-such-ref")),
        ("a base without a config", argv(base=bases["noconfig"][0])),
        ("an invalid config", argv(base=bases["invalid"][0])),
    ]
    for name, args in cases:
        proc, record = r.sb.run(args)
        stderr = proc.stderr.strip().splitlines()
        T.check(
            f"unusable: {name} exits 2 with no record",
            proc.returncode == 2 and record is None and proc.stdout == "" and len(stderr) == 1,
            f"exit {proc.returncode}, record {record!r}, stderr {stderr}",
        )


def check_stale(r: Runner) -> None:
    ctx = r.context("judged")
    proc, record, _, _ = r.run(ctx, verdict_for(ctx), JUDGEMENTS["at"])
    T.check("stale: a normal run replaces a stale record", usable_record(record), json.dumps(record)[:200])
    leftovers = [p.name for p in (r.sb.repo / RECORD).parent.iterdir() if p.name.startswith(".landing.")]
    T.check("stale: no temporary record is left", not leftovers, str(leftovers))


def plan_record_keys() -> list:
    """The top-level keys of plan-approval/decision.py's record, from its source."""
    tree = ast.parse(PLAN_DECISION.read_text(encoding="utf-8"))
    for node in ast.walk(tree):
        if (
            isinstance(node, ast.Assign)
            and any(isinstance(t, ast.Name) and t.id == "record" for t in node.targets)
            and isinstance(node.value, ast.Dict)
        ):
            return [k.value for k in node.value.keys if isinstance(k, ast.Constant)]
    return []


def check_shape() -> None:
    plan = plan_record_keys()
    renamed = [{"judged_decision": "would_be", "decision": "effective"}.get(k, k) for k in plan]
    T.check(
        "shape: the record's keys are plan decision's, renamed, then acts, inputs and agents",
        bool(plan) and RECORD_KEYS == renamed + ["acts", "inputs", "agents"],
        json.dumps(plan),
    )


def check_checkout(real: dict) -> None:
    T.check(
        "checkout: pipeline.landing.mode is shadow",
        real.get("pipeline", {}).get("landing", {}).get("mode") == "shadow",
        json.dumps(real.get("pipeline", {}).get("landing")),
    )
    gates = [g.get("name") for g in real.get("gates", {}).get("run", [])]
    commands = {g.get("name"): g.get("command") for g in real.get("gates", {}).get("run", [])}
    ok = (
        "landing-decision-selftest" in gates
        and "landing-judgement-selftest" in gates
        and gates.index("landing-decision-selftest") == gates.index("landing-judgement-selftest") + 1
        and commands["landing-decision-selftest"] == "python3 .mintswarm/dogfood/landing/decision-selftest.py"
    )
    T.check("checkout: gates.run runs this self-test after landing-judgement-selftest", ok, json.dumps(gates))


def check_hygiene(sb: Sandbox) -> None:
    stores = [p.name for p in sb.store_tmp.iterdir()]
    T.check("hygiene: no object store is left in TMPDIR", not stores, str(stores))
    caches = [str(p) for p in (HERE, PLAN_DECISION.parent) if (p / "__pycache__").exists()]
    T.check("hygiene: no __pycache__ next to the scripts", not caches, str(caches))
    for path in (DECISION, Path(__file__).resolve()):
        tree = ast.parse(path.read_text(encoding="utf-8"))
        names = set()
        for node in ast.walk(tree):
            if isinstance(node, ast.Import):
                names.update(alias.name.split(".")[0] for alias in node.names)
            elif isinstance(node, ast.ImportFrom):
                names.add("." if node.level else node.module.split(".")[0])
        outside = sorted(n for n in names if n not in sys.stdlib_module_names)
        T.check(f"hygiene: {path.name} imports only the standard library", not outside, str(outside))


def build_bases(sb: Sandbox, real: dict) -> dict:
    bases = {}

    def variant(name: str, change=None, config=True) -> None:
        tree = dict(BASE_TREE)
        mode, threshold = None, None
        if config:
            cfg = copy.deepcopy(real)
            if change is not None:
                change(cfg)
            tree[CONFIG_PATH] = as_json(cfg)
            landing = cfg["pipeline"]["landing"]
            mode, threshold = landing.get("mode"), landing.get("min_confidence", 95)
        base = sb.commit(tree, (), f"base {name}")
        head = sb.commit({**tree, "docs/guide.md": text(5, "guide")}, (base,), f"head {name}")
        bases[name] = (base, head, mode, threshold)

    def landing(**values):
        return lambda c: c["pipeline"]["landing"].update(values)

    for mode in MODES:
        variant(mode, landing(mode=mode))
    variant("judged-90", landing(mode="judged", min_confidence=90))
    variant(
        "override",
        lambda c: (
            c["pipeline"]["landing"].update(mode="shadow"),
            c["agents"]["stages"].update(review={"model": "sonnet", "effort": 3}),
        ),
    )
    variant("invalid", landing(mode="sometimes"))
    variant("noconfig", config=False)
    return bases


def main() -> int:
    for tool in ("git", "yq"):
        if shutil.which(tool) is None:
            print(f"decision-selftest.py: {tool} is not on PATH", file=sys.stderr)
            return 2
    real = yq_json(CONFIG)
    with tempfile.TemporaryDirectory(prefix="msw-landing-decision-") as tmp:
        sb = Sandbox(Path(tmp))
        bases = build_bases(sb, real)
        runner = Runner(sb, bases)
        check_matrix(runner)
        check_reasons(runner)
        check_threshold(runner)
        check_bindings(runner)
        check_approval(runner)
        check_record(runner, real)
        check_comment(runner, real)
        check_unusable(runner, bases)
        check_stale(runner)
        check_hygiene(sb)
    check_shape()
    check_checkout(real)
    print(f"{T.passed} passed, {T.failed} failed")
    return 0 if T.failed == 0 else 1


if __name__ == "__main__":
    sys.exit(main())
