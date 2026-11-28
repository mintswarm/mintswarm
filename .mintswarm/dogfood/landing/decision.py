#!/usr/bin/env python3
"""Decide whether a finished change needs a human, from the landing floor and the landing stage.

    decision.py --pr N --components NAMES --plan PATH --base REF --head REF
                --facts PATH --landing PATH [--landing-model MODEL]

Dogfooding stand-in for the control plane's landing decision, which combines
the two layers of landing in code: the deterministic landing floor
(landing/floor.py) and the judgement of the landing stage (landing/
judgement.py). Paths are relative to the working directory, which is the
root of a checkout holding both commits. Every argument but --landing-model
is required; each is given at most once, as `--name value`; anything else,
an empty value or a value that is an argument name is unusable input. N is a positive decimal integer, the PR's
number, and neither REF may start with `-`. The first action is to delete
any stale .mintswarm/job/outputs/landing.json; when that fails, the input is
unusable.

Nothing merges on the decision yet: the record's `acts` is always false,
and in every mode the maintainer approves the PR and the bot merges it as
before.

The landing sequence
--------------------

The session runs it on a PR once review passes, and again, with a new
comment, each time review passes on a new head; earlier comments are left
as they are:

1. keep the round with landing/keep-round.py;
2. collect the facts: `landing/collect-facts.py fetch`, then `assemble`,
   which writes .mintswarm/job/inputs/landing/facts.json; right after it,
   `github/ancestor-decisions.py fetch`, then `assemble`, which writes
   .mintswarm/job/inputs/ancestors.md;
3. extract a base-ref copy of .mintswarm/dogfood/ to a temporary directory
   outside the worktree (`git archive` of the base commit), and run
   landing/floor.py from that copy, in the worktree, with the same --plan,
   --components, --base, --head and --facts as below;
4. delete any earlier landing.md, then run the landing stage
   (.mintswarm/stages/landing.md), which writes a fresh landing.md; except
   under `never`, whose judge is skipped;
5. run this script from the same base-ref copy, in the worktree, with
   --landing-model set to the model the landing stage was dispatched with;
6. publish landing.md and .mintswarm/job/outputs/landing.json as issue
   artifacts, and post this script's stdout on the PR as mintswarm-dev-bee[bot]
   through .mintswarm/dogfood/github/bot-gh.sh (`-F body=@<file>`), with
   one appended line holding the artifact URLs.

Under `always`, steps 3 and 4 may be skipped: this script still runs and
records `effective: human`, and prints no comment.

Running from the base-ref copy matters: run from the worktree, both this
script and the floor are the branch's own versions.

Inputs
------

- REF given to --base. The mode and the threshold are
  `pipeline.landing.mode` and `pipeline.landing.min_confidence` (95 when
  absent) of .mintswarm/config.yaml at the commit it resolves to, read and
  validated as landing/floor.py reads them (its load_config, validate_config
  and validate_landing, loaded from the file next to this one with the plan
  floor beside it), so the worktree's config is never opened. NAMES given to
  --components, the issue's components comma-separated in the issue's
  order, must each be non-empty, given once and a defined component. The
  harness, model and effort of implement, review and landing are
  `agents.stages.<stage>` over `agents.default` there; a value that is absent or not a string is null.
- MODEL given to --landing-model, the model the landing stage ran on. When
  it is given, the landing entry's `model` is MODEL; otherwise it is the
  configured model. Implement and review always record the configured one.
- REF given to --head, the head of the change. Both refs are resolved and
  read through the plan floor's job_object_store, from a fresh object store
  in the system temp directory, as the landing floor reads them; the merge
  base is the landing floor's merge_base in that store.
- PATH given to --plan, the approved plan, read as UTF-8 by the plan
  floor's read_plan; an unreadable one is unusable input.
- PATH given to --facts, the landing facts (landing/collect-facts.py
  assemble).
- The landing floor's verdict, .mintswarm/job/outputs/landing-floor.json.
- PATH given to --landing, the landing stage's landing.md.

The facts are usable only if they are strict JSON (no duplicate keys, NaN
or Infinity) holding an object with `tool` `mintswarm-landing-facts`,
`version` 3, `head` and `base` the resolved commits and `pr` N.

The floor layer is usable only if the facts are, and the verdict is strict
JSON and all of these hold: its keys are landing/floor.py's, in order;
`tool` is `mintswarm-landing-floor` and `version` 3; `plan.sha256` is the
sha256 of the plan and `facts.sha256` that of the facts; `base.commit` and
`head.commit` are the resolved commits and `merge_base` the merge base;
`components` is NAMES as a list, in the same order, `mode` the base
config's mode and `config` `.mintswarm/config.yaml`; every rule is {`rule`, `result`, `reasons`} with
`result` `hit` or `clean` and string reasons exactly when it is a hit; the
identifiers follow landing/floor.py's RULES across `rules` and
`not_evaluated`, and `not_evaluated` is empty, or exactly the diff rules
after a `footprint` hit; `evidence` is a list of strings, empty after a
`footprint` hit; and `verdict` agrees with the rules. Evidence (the floor's
line-limit overruns) never makes a hit; it is carried in the layer and
shown in the reasons and the comment. The verdict
is checked for shape and bindings, not recomputed. A missing verdict (the
floor exited 2) makes the layer unusable.

The judgement layer is landing/judgement.py's judgement() of landing.md's
bytes, with `stage` (`landing`), `path` and `sha256`; a missing or
unreadable landing.md is unusable. landing.md carries no commit: it is
bound by its sha256 only, so the session deletes it before each run of the
landing stage (step 4).

Problems in either layer are fixed strings that never quote the files.

The decision
------------

`would_be` is judgement.judged_decision(the floor's verdict when the floor
layer is usable, else none; the judgement layer; min_confidence), in every
mode: `auto` only for a clean floor and a usable judgement with no hard
trigger and a confidence of at least min_confidence; otherwise `human`.
The approval in the facts is an input only: no branch reads it, so a plan
approved by a human with no kickoff record leaves `would_be` as it is. The
landing floor, not a branch here, uses the approval to decide which
require_human paths the plan covered.

`effective` follows the mode:

  always   `human`; neither the floor nor the judge is required
  shadow   `human`; `would_be` is recorded and changes nothing
  judged   `would_be`
  never    the judge is skipped: `auto` exactly when the floor layer is
           usable and clean

The record
----------

.mintswarm/job/outputs/landing.json, written to a temporary file and
renamed onto it. Its keys, in this order, are plan-approval/decision.py's
record's, with its `judged_decision` and `decision` named `would_be` and
`effective`, and `acts`, `inputs` and `agents` added:

  tool             `mintswarm-landing-decision`
  version          4
  approval         `landing`
  subject          {`pr`, `issue` (from the facts, null when they are
                   unusable), `components`, `plan` {`path`, `sha256`},
                   `base` {`ref`, `commit`}, `head` {`ref`, `commit`},
                   `config` {`path`, `commit`, the resolved base}}
  mode, min_confidence
  layers           `floor` {`status`, `problems`, `path`, `sha256`,
                   `verdict`, `rules`, `not_evaluated`, `evidence`} and
                   `judgement`
                   {`status`, `problems`, `stage`, `path`, `sha256`,
                   `confidence`, `band`, `hard_triggers`, `doubts`,
                   `reasons`}
  would_be, effective
  reasons          the mode's rule; the floor's outcome and each hit rule
                   with its reasons, or its problems; each evidence entry;
                   the judgement's band,
                   confidence and decisive reason, then each unusable
                   hard trigger as `hard trigger`, the authority triggers
                   as `requires the maintainer`, or, with no trigger, a
                   confidence below min_confidence; or its problems;
                   `would_be` and `effective`
  acts             false
  inputs           `facts` {`path`, `sha256`} and `approval` {`approver`,
                   `approved_at`, `approval_ref`, `kickoff_records`, each
                   {`kind`, `path`, `sha256`} for the facts' `decision`,
                   `kickoff_floor` and `assessment`}, null or [] when the
                   facts are unusable
  agents           {`implement`, `review`, `landing`}, each {`harness`,
                   `model`, `effort`, `configured_model`, `model_given`,
                   `model_differs`}: `model` is the model that ran,
                   `configured_model` the base config's, `model_given`
                   whether --landing-model stated it and `model_differs`
                   whether a stated model is not the configured one; both
                   flags are false for implement and review

A field that cannot be filled is null, or [] for a list.

The comment
-----------

After the record is written, the PR comment goes to stdout in `shadow`,
`judged` and `never`; `always` prints nothing there. Its heading is
`### Shadow record: landing` in `shadow` and `### Landing decision`
otherwise. It states the mode, that the decision changes nothing and that
the maintainer approves and the bot merges as before; the PR, head and base
commits; the floor (clean with its rule count, each hit rule with its
reasons, the rules not evaluated, or unusable with its problems); the
judgement rows of plan-approval/decision.py (Confidence as the band's name
and score with min_confidence and the decisive reason, Hard triggers and
Doubts); the would-be and the effective decision; the plan approver and
the plan approval time, which decision.py records but does not decide on,
and which the landing floor uses for coverage; and the harness, model and
effort of implement, review and landing, with the configured model after
the one that ran when they differ. Model, facts and session text is flattened
to one line, its backticks replaced, cut to 600 characters and marked
`… (truncated)` when cut, and set in a code span (decision.py's span), so
no line of the comment can be a steer marker. One status line goes to
stderr.

Exits 0 when `effective` is `auto` and 1 when it is `human`. Exits 2 on
unusable input, with no record unless a stale one could not be deleted: bad
arguments, an unreadable plan, an unresolvable base or head, a partial
clone, a failed fetch into the object store, no config at the base, an
invalid config, an empty, repeated or unknown component name, git or yq
missing or failing, a missing landing/floor.py, plan-approval/floor.py, landing/judgement.py or
plan-approval/decision.py, a record that cannot be deleted or written, or
any unexpected error. A missing or mismatched verdict, facts file or
landing.md is not unusable input: it gives a record with that layer
unusable. Exit 2 or no record counts as `human`. Standard library only,
apart from running git and yq through the floors.
"""

import hashlib
import importlib.util
import json
import os
import re
import sys
import tempfile
from pathlib import Path
from types import ModuleType

HERE = Path(__file__).resolve().parent
LANDING_FLOOR = HERE / "floor.py"
JUDGEMENT = HERE / "judgement.py"
RECORD = Path(".mintswarm/job/outputs/landing.json")
TOOL = "mintswarm-landing-decision"
VERSION = 4
STAGE = "landing"
CONFIG = ".mintswarm/config.yaml"
FLOOR_TOOL = "mintswarm-landing-floor"
FLOOR_VERSION = 3
FLOOR_KEYS = [
    "tool",
    "version",
    "plan",
    "facts",
    "components",
    "base",
    "head",
    "merge_base",
    "config",
    "mode",
    "verdict",
    "rules",
    "not_evaluated",
    "evidence",
]
FACTS_TOOL = "mintswarm-landing-facts"
FACTS_VERSION = 3
KICKOFF_KINDS = ("decision", "kickoff_floor", "assessment")
AGENT_STAGES = ("implement", "review", "landing")
AGENT_KEYS = ("harness", "model", "effort")
ARGS = ("--pr", "--components", "--plan", "--base", "--head", "--facts", "--landing")
OPTIONAL = ("--landing-model",)
NAMES = ARGS + OPTIONAL
USAGE = (
    "usage: decision.py --pr N --components NAMES --plan PATH --base REF --head REF "
    "--facts PATH --landing PATH [--landing-model MODEL]"
)
UNCHANGED = "This decision changes nothing: the maintainer approves this PR and the bot merges it as before."
MODE_REASONS = {
    "always": "mode always: every change goes to a human; neither the floor nor the judge is required",
    "shadow": "mode shadow: every change goes to a human; would_be is recorded for comparison and changes nothing",
    "judged": "mode judged: effective is would_be; nothing merges on effective yet",
    "never": "mode never: the judge is skipped and effective is auto exactly on a clean floor; "
    "nothing merges on effective yet",
}

floor: ModuleType  # landing/floor.py with its plan floor, set by main()
judgement: ModuleType  # landing/judgement.py, set by main()


class InputError(Exception):
    """Unusable input: exit 2, no record."""


def load(name: str, path: Path) -> ModuleType:
    sys.dont_write_bytecode = True  # leave no __pycache__ in the source tree
    if not path.is_file():
        raise InputError(f"{path} is missing")
    spec = importlib.util.spec_from_file_location(name, path)
    if spec is None or spec.loader is None:
        raise InputError(f"cannot load {path}")
    module = importlib.util.module_from_spec(spec)
    try:
        spec.loader.exec_module(module)
    except Exception as exc:
        message = " ".join(str(exc).split())
        raise InputError(f"cannot load {path}: {type(exc).__name__}: {message}") from exc
    return module


def load_modules() -> None:
    global floor, judgement
    floor = load("msw_landing_floor_for_decision", LANDING_FLOOR)
    try:
        floor.P = floor.load_plan_floor()
    except floor.InputError as exc:
        raise InputError(str(exc)) from exc
    judgement = load("msw_landing_judgement_for_decision", JUDGEMENT)


def sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def is_input_error(exc: Exception) -> bool:
    return isinstance(exc, (floor.InputError, floor.P.InputError))


# --- arguments -----------------------------------------------------------


def parse_args(argv: list) -> dict:
    args = {}
    i = 0
    while i < len(argv):
        name = argv[i]
        if name not in NAMES:
            raise InputError(f"unknown argument {name!r}; {USAGE}")
        if name in args:
            raise InputError(f"{name} given twice; {USAGE}")
        if i + 1 >= len(argv):
            raise InputError(f"{name} needs a value; {USAGE}")
        value = argv[i + 1]
        if value == "" or value in NAMES:
            raise InputError(f"{name} needs a value; {USAGE}")
        args[name] = value
        i += 2
    missing = [name for name in ARGS if name not in args]
    if missing:
        raise InputError(f"missing {', '.join(missing)}; {USAGE}")
    if not re.fullmatch(r"[1-9][0-9]*", args["--pr"]):
        raise InputError(f"--pr must be a positive decimal integer: {args['--pr']!r}")
    for name in ("--base", "--head"):
        if args[name].startswith("-"):
            raise InputError(f"{name} must not start with '-': {args[name]!r}")
    return {
        "pr": int(args["--pr"]),
        "components": args["--components"].split(","),
        "plan": args["--plan"],
        "base": args["--base"],
        "head": args["--head"],
        "facts": args["--facts"],
        "landing": args["--landing"],
        "landing_model": args.get("--landing-model"),
    }


def configured_agents(config: dict) -> dict:
    """{stage: {harness, model, effort}} for implement, review and landing, from the base config."""

    def mapping(value) -> dict:
        return value if isinstance(value, dict) else {}

    agents = mapping(config.get("agents"))
    default = mapping(agents.get("default"))
    stages = mapping(agents.get("stages"))
    result = {}
    for stage in AGENT_STAGES:
        own = mapping(stages.get(stage))
        entry = {}
        for key in AGENT_KEYS:
            value = own[key] if key in own else default.get(key)
            entry[key] = value if isinstance(value, str) else None
        result[stage] = entry
    return result


def agents(config: dict, landing_model) -> dict:
    """The configured agents, with the landing model that ran when the caller states it."""
    result = {}
    for stage, entry in configured_agents(config).items():
        given = stage == STAGE and landing_model is not None
        model = landing_model if given else entry["model"]
        result[stage] = {
            **entry,
            "model": model,
            "configured_model": entry["model"],
            "model_given": given,
            "model_differs": given and model != entry["model"],
        }
    return result


# --- facts ---------------------------------------------------------------


def read_facts(path: str, pr: int, base: str, head: str) -> tuple:
    """(sha256 or None, parsed facts or None, problems) for the facts file."""
    try:
        data = floor.read_bytes_nofollow(path, "the facts file")
    except floor.InputError:
        return None, None, ["the facts file is missing or cannot be read"]
    digest = sha256(data)
    try:
        facts = floor.strict_json(data, "the facts file")
    except floor.InputError:
        return digest, None, ["the facts file is not strict JSON"]
    if not isinstance(facts, dict):
        return digest, None, ["the facts file is not a JSON object"]
    problems = []
    version = facts.get("version")
    if facts.get("tool") != FACTS_TOOL or type(version) is not int or version != FACTS_VERSION:
        problems.append("the facts file's tool or version is not collect-facts.py's")
    if facts.get("head") != head:
        problems.append("the facts file's head does not match --head")
    if facts.get("base") != base:
        problems.append("the facts file's base does not match --base")
    if type(facts.get("pr")) is not int or facts.get("pr") != pr:
        problems.append("the facts file's pr does not match --pr")
    return digest, (None if problems else facts), problems


def approval_inputs(facts) -> dict:
    """The approval the facts record, as an input.

    No branch here decides on it; the landing floor uses it for coverage.
    """
    approval = facts.get("approval") if isinstance(facts, dict) else None
    if not isinstance(approval, dict):
        return {"approver": None, "approved_at": None, "approval_ref": None, "kickoff_records": []}

    def text(value):
        return value if isinstance(value, str) else None

    ref = approval.get("approval_ref")
    records = []
    for kind in KICKOFF_KINDS:
        entry = approval.get(kind)
        if isinstance(entry, dict):
            records.append({"kind": kind, "path": text(entry.get("path")), "sha256": text(entry.get("sha256"))})
    return {
        "approver": text(approval.get("approver")),
        "approved_at": text(approval.get("approved_at")),
        "approval_ref": ref if isinstance(ref, (str, int)) and not isinstance(ref, bool) else None,
        "kickoff_records": records,
    }


# --- floor layer ---------------------------------------------------------


def is_rule(item) -> bool:
    if not isinstance(item, dict) or list(item) != ["rule", "result", "reasons"]:
        return False
    if not isinstance(item["rule"], str) or item["result"] not in ("hit", "clean"):
        return False
    reasons = item["reasons"]
    if not isinstance(reasons, list) or not all(isinstance(r, str) for r in reasons):
        return False
    return (item["result"] == "hit") == bool(reasons)


def bound(value, key: str, expected) -> bool:
    return expected is not None and isinstance(value, dict) and value.get(key) == expected


def verdict_problems(verdict, expected: dict) -> list:
    if not isinstance(verdict, dict):
        return ["the floor verdict is not a JSON object"]
    if list(verdict) != FLOOR_KEYS:
        return ["the floor verdict's keys are not landing/floor.py's, in order"]
    problems = []
    if verdict["tool"] != FLOOR_TOOL or type(verdict["version"]) is not int or verdict["version"] != FLOOR_VERSION:
        problems.append("the floor verdict's tool or version is not landing/floor.py's")
    if not bound(verdict["plan"], "sha256", expected["plan"]):
        problems.append("the floor verdict's plan.sha256 does not match the plan")
    if not bound(verdict["facts"], "sha256", expected["facts"]):
        problems.append("the floor verdict's facts.sha256 does not match the facts file")
    if not bound(verdict["base"], "commit", expected["base"]):
        problems.append("the floor verdict's base.commit does not match the base")
    if not bound(verdict["head"], "commit", expected["head"]):
        problems.append("the floor verdict's head.commit does not match the head")
    if expected["merge_base"] is None or verdict["merge_base"] != expected["merge_base"]:
        problems.append("the floor verdict's merge_base does not match the merge base")
    if verdict["components"] != expected["components"]:
        problems.append("the floor verdict's components do not match --components")
    if verdict["mode"] != expected["mode"]:
        problems.append("the floor verdict's mode does not match the base config")
    if verdict["config"] != CONFIG:
        problems.append(f"the floor verdict's config is not {CONFIG}")
    rules = verdict["rules"]
    skipped = verdict["not_evaluated"]
    if not isinstance(rules, list) or not all(is_rule(r) for r in rules):
        problems.append("the floor verdict's rules are not well formed")
        return problems
    if not isinstance(skipped, list) or not all(isinstance(s, str) for s in skipped):
        problems.append("the floor verdict's not_evaluated is not a list of rule identifiers")
        return problems
    names = [r["rule"] for r in rules]
    if skipped not in ([], list(floor.DIFF_RULES)) or names != [n for n in floor.RULES if n not in skipped]:
        problems.append(
            "the floor verdict's rule identifiers are not landing/floor.py's, in order, "
            "with none or only the diff rules not evaluated"
        )
        return problems
    if bool(skipped) != (rules[0]["result"] == "hit"):
        problems.append("the floor verdict's not_evaluated does not follow its footprint result")
    evidence = verdict["evidence"]
    if not isinstance(evidence, list) or not all(isinstance(e, str) for e in evidence):
        problems.append("the floor verdict's evidence is not a list of strings")
    elif evidence and rules[0]["result"] == "hit":
        problems.append("the floor verdict's evidence is not empty after a footprint hit")
    hit = any(r["result"] == "hit" for r in rules)
    if verdict["verdict"] != ("hit" if hit else "clean"):
        problems.append("the floor verdict's verdict does not agree with its rules")
    return problems


def floor_layer(expected: dict, facts_problems: list) -> dict:
    layer = {
        "status": "unusable",
        "problems": [],
        "path": str(floor.VERDICT),
        "sha256": None,
        "verdict": None,
        "rules": [],
        "not_evaluated": [],
        "evidence": [],
    }
    try:
        data = floor.VERDICT.read_bytes()
    except FileNotFoundError:
        layer["problems"] = ["the floor verdict is missing"] + facts_problems
        return layer
    except OSError:
        layer["problems"] = ["the floor verdict cannot be read"] + facts_problems
        return layer
    layer["sha256"] = sha256(data)
    try:
        verdict = floor.strict_json(data, "the floor verdict")
    except floor.InputError:
        layer["problems"] = ["the floor verdict is not strict JSON"] + facts_problems
        return layer
    problems = verdict_problems(verdict, expected) + facts_problems
    if problems:
        layer["problems"] = problems
        return layer
    layer.update(
        status="usable",
        verdict=verdict["verdict"],
        rules=verdict["rules"],
        not_evaluated=verdict["not_evaluated"],
        evidence=verdict["evidence"],
    )
    return layer


# --- judgement layer -----------------------------------------------------


def judgement_layer(path: str) -> dict:
    layer = {"status": "unusable", "problems": [], "stage": STAGE, "path": path, "sha256": None}
    fields = {"confidence": None, "band": None, "hard_triggers": [], "doubts": [], "reasons": []}
    try:
        data = Path(path).read_bytes()
    except FileNotFoundError:
        layer["problems"] = ["the landing document is missing"]
        return {**layer, **fields}
    except OSError:
        layer["problems"] = ["the landing document cannot be read"]
        return {**layer, **fields}
    layer["sha256"] = sha256(data)
    judged = judgement.judgement(data)
    layer["status"] = judged["status"]
    layer["problems"] = judged["problems"]
    for key in fields:
        fields[key] = judged[key]
    return {**layer, **fields}


# --- decision ------------------------------------------------------------


def floor_result(layer: dict) -> str:
    return layer["verdict"] if layer["status"] == "usable" else "unusable"


def hit_rules(layer: dict) -> list:
    return [r for r in layer["rules"] if r["result"] == "hit"]


def decide(mode: str, min_confidence: int, floor_: dict, judged: dict) -> tuple:
    """(would_be, effective, reasons)."""
    verdict = floor_["verdict"] if floor_["status"] == "usable" else None
    would_be = judgement.judged_decision(verdict, judged, min_confidence)
    if mode == "judged":
        effective = would_be
    elif mode == "never":
        effective = "auto" if verdict == "clean" else "human"
    else:
        effective = "human"
    reasons = [MODE_REASONS[mode]]
    result = floor_result(floor_)
    if result == "clean":
        reasons.append(f"floor: clean, {len(floor_['rules'])} of {len(floor.RULES)} rules")
    elif result == "hit":
        reasons.append("floor: hit on " + ", ".join(r["rule"] for r in hit_rules(floor_)))
        for rule in hit_rules(floor_):
            reasons.append(f"floor hit {rule['rule']}: " + "; ".join(rule["reasons"]))
    else:
        reasons.append("floor: unusable")
        reasons += [f"floor: {problem}" for problem in floor_["problems"]]
    reasons += [f"floor evidence: {entry}" for entry in floor_["evidence"]]
    if judged["status"] == "usable":
        reasons.append(f"judgement: {judged['band']} {judged['confidence']}: {judged['reasons'][0]}")
        authority = [t for t in judged["hard_triggers"] if t in judgement.AUTHORITY_TRIGGERS]
        reasons += [
            f"judgement: hard trigger {trigger}"
            for trigger in judged["hard_triggers"]
            if trigger not in judgement.AUTHORITY_TRIGGERS
        ]
        if authority:
            reasons.append(f"judgement: requires the maintainer: {', '.join(authority)}")
        if not judged["hard_triggers"] and judged["confidence"] < min_confidence:
            reasons.append(f"judgement: confidence {judged['confidence']} below min_confidence {min_confidence}")
    else:
        reasons.append("judgement: unusable")
        reasons += [f"judgement: {problem}" for problem in judged["problems"]]
    reasons.append(f"would_be: {would_be}")
    reasons.append(f"effective: {effective}")
    return would_be, effective, reasons


# --- comment -------------------------------------------------------------


def span(value) -> str:
    return judgement.decision.span(value)


def maybe(value) -> str:
    return span(value) if isinstance(value, (str, int)) and not isinstance(value, bool) else "not known"


def floor_rows(layer: dict) -> list:
    result = floor_result(layer)
    if result == "unusable":
        return [f"- **Floor:** unusable: {'; '.join(layer['problems'])}"]  # fixed strings from this script
    if result == "clean":
        rows = [f"- **Floor:** clean, {len(layer['rules'])} of {len(floor.RULES)} rules"]
    else:
        hits = hit_rules(layer)
        names = ", ".join("`" + r["rule"] + "`" for r in hits)
        rows = [f"- **Floor:** hit on {names}"]
        for rule in hits:
            rows.append(f"- **Floor hit `{rule['rule']}`:** " + "; ".join(span(reason) for reason in rule["reasons"]))
    if layer["evidence"]:
        rows.append("- **Floor evidence:** " + "; ".join(span(entry) for entry in layer["evidence"]))
    if layer["not_evaluated"]:
        rows.append("- **Not evaluated:** " + ", ".join(f"`{name}`" for name in layer["not_evaluated"]))
    return rows


def agent_row(entry: dict) -> str:
    model = maybe(entry["model"])
    if entry["model_differs"]:
        model += f" (configured {maybe(entry['configured_model'])})"
    return f"{maybe(entry['harness'])} {model} {maybe(entry['effort'])}"


def comment(record: dict) -> str:
    subject = record["subject"]
    floor_ = record["layers"]["floor"]
    judged = record["layers"]["judgement"]
    approval = record["inputs"]["approval"]
    mode = record["mode"]
    heading = "### Shadow record: landing" if mode == "shadow" else "### Landing decision"
    suffix = f": {'; '.join(judged['problems'])}" if judged["status"] != "usable" else ""
    agents = "; ".join(f"{stage} {agent_row(entry)}" for stage, entry in record["agents"].items())
    lines = [
        heading,
        "",
        f"Mode `{mode}`. {UNCHANGED}",
        "",
        f"- **PR:** #{subject['pr']}, head commit `{subject['head']['commit']}`, "
        f"base commit `{subject['base']['commit']}`",
        *floor_rows(floor_),
        *judgement.decision.judgement_rows(
            judged, record["min_confidence"], suffix, authority=judgement.AUTHORITY_TRIGGERS
        ),
        f"- **Would-be decision:** `{record['would_be']}`",
        f"- **Effective decision:** `{record['effective']}`",
        f"- **Plan approval (input; the landing floor uses it for coverage):** approver {maybe(approval['approver'])}, "
        f"approved at {maybe(approval['approved_at'])}",
        f"- **Agents (harness, model, effort):** {agents}",
    ]
    return "\n".join(lines) + "\n"


# --- record --------------------------------------------------------------


def remove(path) -> None:
    """Delete a file if it exists; raises OSError when it cannot."""
    try:
        os.unlink(path)
    except FileNotFoundError:
        pass


def write_record(record: dict) -> None:
    try:
        RECORD.parent.mkdir(parents=True, exist_ok=True)
        fd, tmp = tempfile.mkstemp(prefix=".landing.", suffix=".json", dir=RECORD.parent)
    except OSError as exc:
        raise InputError(f"cannot write the record {RECORD}: {exc}") from exc
    try:
        with os.fdopen(fd, "w", encoding="utf-8") as out:
            out.write(json.dumps(record, indent=2, ensure_ascii=False) + "\n")
        os.replace(tmp, RECORD)
    except BaseException as exc:
        try:
            remove(tmp)
        except OSError:
            pass
        if isinstance(exc, OSError):
            raise InputError(f"cannot write the record {RECORD}: {exc}") from exc
        raise


def read_base(args: dict) -> tuple:
    """(config, landing, base, head, merge base or None, plan bytes), all read from a fresh object store."""
    P = floor.P
    try:
        with P.job_object_store({"base": args["base"], "head": args["head"]}) as (store, commits):
            base, head = commits["base"], commits["head"]
            config = P.load_config(base, git_dir=store)
            cfg = P.validate_config(config, args["components"])
            landing = floor.validate_landing(config, cfg["paths"])
            try:
                merge = floor.merge_base(base, head, store)
            except floor.InputError:
                merge = None  # the floor exits 2 here too; no verdict can match
            plan = P.read_plan(args["plan"])
    except Exception as exc:
        if is_input_error(exc):
            raise InputError(str(exc)) from exc
        raise
    return config, landing, base, head, merge, plan


def run(argv: list) -> int:
    try:
        remove(RECORD)
    except OSError as exc:
        raise InputError(f"cannot delete the stale record {RECORD}: {exc}") from exc
    args = parse_args(argv)
    config, landing, base, head, merge, plan = read_base(args)
    mode = landing["mode"]
    min_confidence = landing["min_confidence"]
    plan_sha = sha256(plan)

    facts_sha, facts, facts_problems = read_facts(args["facts"], args["pr"], base, head)
    expected = {
        "plan": plan_sha,
        "facts": facts_sha,
        "base": base,
        "head": head,
        "merge_base": merge,
        "components": args["components"],
        "mode": mode,
    }
    floor_ = floor_layer(expected, facts_problems)
    judged = judgement_layer(args["landing"])
    would_be, effective, reasons = decide(mode, min_confidence, floor_, judged)
    issue = facts.get("issue") if facts is not None else None
    record = {
        "tool": TOOL,
        "version": VERSION,
        "approval": "landing",
        "subject": {
            "pr": args["pr"],
            "issue": issue if type(issue) is int else None,
            "components": args["components"],
            "plan": {"path": args["plan"], "sha256": plan_sha},
            "base": {"ref": args["base"], "commit": base},
            "head": {"ref": args["head"], "commit": head},
            "config": {"path": CONFIG, "commit": base},
        },
        "mode": mode,
        "min_confidence": min_confidence,
        "layers": {"floor": floor_, "judgement": judged},
        "would_be": would_be,
        "effective": effective,
        "reasons": reasons,
        "acts": False,
        "inputs": {
            "facts": {"path": args["facts"], "sha256": facts_sha},
            "approval": approval_inputs(facts),
        },
        "agents": agents(config, args["landing_model"]),
    }
    text = comment(record) if mode != "always" else None
    write_record(record)
    judged_as = f"{judged['band']} {judged['confidence']}" if judged["status"] == "usable" else "unusable"
    print(
        f"landing decision: {effective} (mode {mode}, floor {floor_result(floor_)}, "
        f"judgement {judged_as}, would_be {would_be})",
        file=sys.stderr,
    )
    if text is not None:
        sys.stdout.write(text)
        sys.stdout.flush()
    return 0 if effective == "auto" else 1


def main(argv: list) -> int:
    try:
        load_modules()
        return run(argv)
    except InputError as exc:
        print(f"landing/decision.py: {exc}", file=sys.stderr)
        return 2
    except Exception as exc:  # a crash must never read as a decision
        try:
            remove(RECORD)
        except Exception:  # the handler itself must always end in exit 2
            pass
        message = " ".join(str(exc).split())
        print(f"landing/decision.py: unexpected error: {type(exc).__name__}: {message}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
