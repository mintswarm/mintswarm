#!/usr/bin/env python3
"""Decide whether a plan needs a human, from the floor verdict and the assessment.

    decision.py --issue N --component NAME --plan PATH --issue-document PATH
                --analysis PATH --assessment PATH --assessment-artifact REF
                --base REF --harness NAME --model NAME

Dogfooding stand-in for the control plane's plan-approval decision, which
combines the two layers of plan approval in code: the deterministic floor
(floor.py) and the judgement of the assess stage. Paths are relative to the
working directory, which is the worktree root. Every argument is required,
each once, as `--name value`; anything else, an empty value or a value that
is an argument name is unusable input. N is a positive decimal integer, the
issue's number, and REF given to --base must not start with `-`. The first
action is to delete any stale .mintswarm/job/outputs/decision.json; when
that fails, the input is unusable.

Inputs:

- REF given to --base, the base ref. The mode and the threshold are
  `pipeline.plan_approval.mode` and `pipeline.plan_approval.min_confidence`
  (an integer from 90 to 100, 90 when absent) of .mintswarm/config.yaml at
  the commit it resolves to, read and validated by floor.py's own code (loaded from the
  file next to this one, its `prepare`, which also reads the packages present
  at the base as the floor does), so the worktree's config is never opened. NAME
  given to --component must be a defined component.
- PATH given to --plan, the plan, and the PATHs given to --issue-document
  and --analysis, the issue document and the analysis the floor read, all
  read as UTF-8; an unreadable one is unusable input.
- The history of the base commit, read by floor.py's own code.

The base commit, the config at it and the history are read as floor.py
reads them: through floor.py's job_object_store, from a fresh bare store in
the system temp directory, filled by a by-hash protocol v2 fetch over
`file://` that re-hashes every object, its closure then walked in the
store, under floor.py's neutral git
environment, and deleted afterwards. The trust boundary that remains is
floor.py's: ref names are resolved in the job repository, the store is
out of the stages' reach by convention only, and a forged object in the
history makes every run exit 2.
- The floor verdict, .mintswarm/job/outputs/floor.json, from floor.py.
- PATH given to --assessment, the assess stage's assessment.md, and REF
  given to --assessment-artifact, where it was published.
- --harness and --model, what the session actually ran assess on. They are
  compared with `agents.stages.assess` over `agents.default` in the base
  config; a mismatch is recorded, and under `judged` it gives `human`.

The floor layer is usable only if the verdict is strict JSON (no duplicate
keys, NaN or Infinity) and all of these hold: its keys are floor.py's, in
order; `tool` is `mintswarm-plan-floor` and `version` is 2; `plan.sha256`
is the sha256 of the plan's bytes; `issue_document.sha256` and
`analysis.sha256` are those of the documents given here, so a verdict made
without them or with other ones does not count; `base.commit` is the
resolved base; `component` is NAME; `mode` is the base config's mode; every
rule is {`rule`, `result`, `reasons`} with `result` `hit` or `clean` and
string reasons exactly when it is a hit; `rules` holds floor.py's rule
identifiers in order, and `not_evaluated` is empty, or exactly the rules
that read the footprint (floor.FOOTPRINT_RULES) after a `footprint` hit;
`verdict` agrees with the rules; and `rules` and `not_evaluated` equal what
floor.py's own conclude gives for the plan, the documents, the history and
the validated base config, recomputed here, so an edited verdict that is
consistent in itself still does not count. A usable verdict is therefore
clean only when all of floor.py's rules ran and are clean.

The judgement layer is usable only if the assessment is UTF-8 and, with
lines split at LF only and trailing CR, space and tab removed from each
(envelope_problems, which landing/collect-facts.py loads by path; landing/
judgement.py loads it too, with its own hard triggers and naming the
landing document in the problems):

- no line starts with `Risk:`;
- the first non-blank line is a `# ` title, and the next non-blank line is
  `<confidence> confidence: <reason>`, with a reason that is not empty once
  whitespace, control and format characters are removed;
- exactly one line is the fence opener ```json, closed by the next line of
  exactly three backticks; between them is the envelope, strict JSON (no
  duplicate keys, NaN or Infinity) with no string holding a lone surrogate
  (an escape such as \\ud800, which UTF-8 cannot encode), one object with
  exactly the keys `confidence`, `band`, `hard_triggers`, `reason` and
  `doubts`, in order;
- `confidence` is an integer (not a boolean) from 0 to 100, equal to the
  summary line's, and inside the range of `band`, one of the BANDS
  identifiers: `blocked` (0), `uncertain` (1-49), `needs-review` (50-79),
  `likely-safe` (80-94) or `safe` (95-100);
- `hard_triggers` holds unique HARD_TRIGGERS identifiers and is non-empty
  exactly when the band is `blocked`;
- `reason` is a non-empty string as above;
- `doubts` holds at most 10 objects, each with exactly the non-empty strings
  `statement`, `consequence` and `resolution`, in order; `likely-safe` has
  at least one doubt and `safe` none.

The envelope's reason is the layer's only entry in `reasons`.

Problems in either layer are fixed strings that never quote the files.

`judged_decision` is `auto` only when the floor layer is usable and clean,
the judgement layer is usable with no hard trigger and a confidence of at
least `min_confidence`, and the judge's harness and model are the
configured ones (`as_configured`); otherwise `human`.
`decision` follows the mode: `always` and `shadow` give `human`, `never`
gives `auto`, and `judged` gives `judged_decision`. Both layers are always
read and recorded, whatever the mode.

The record is .mintswarm/job/outputs/decision.json, written to a temporary
file and renamed onto it, with these keys in this order: `tool`
(`mintswarm-approval-decision`), `version` (2), `approval` (`plan`),
`subject` ({`issue`, `component`, `plan` {`path`, `sha256`}, `base`
{`ref`, `commit`}}), `mode`, `min_confidence`, `layers`, `judged_decision`,
`decision` and `reasons` (the mode's rule, each layer's outcome, the
judgement's confidence and band, and a hard trigger or a confidence below
`min_confidence`). `layers.floor` holds
`status` (`usable` or `unusable`), `problems`, `path`, `sha256` (of the
verdict's bytes), `verdict`, `rules` (the verdict's rule objects,
unchanged) and `not_evaluated`. `layers.judgement` holds `status`,
`problems`, `stage` (`assess`), `path`, `sha256`, `artifact`, `harness`,
`model`, `expected` ({`harness`, `model`}), `as_configured`, `confidence`,
`band` (the identifier), `hard_triggers`, `doubts` and `reasons`. A field that cannot be filled, including the content of an
unusable layer, is null, or [] for a list.

One status line goes to stderr, with the band and confidence or
`unusable`. After the record is written, the issue
comment the session posts goes to stdout, in two cases only:

- for `auto`, the auto-approval comment: it says the plan was approved
  without a human, the mode and base commit, the floor result, the
  judgement rows, the assessment artifact and who judged, and how to veto;
- in `shadow` mode, the shadow-record comment: the plan's path, sha256 and
  footprint in brief, the assess harness, model and artifact and whether
  they are as configured, the judgement rows, the floor's base, result,
  exit code and hit and clean rules, `judged_decision` and `decision`, and a
  pending maintainer decision.

The judgement rows are `Confidence` (the band's display name and the score
as `<name> · <score>`, the threshold and the reason, or `unusable` with the
problems), and for a usable judgement `Hard triggers` and one `Doubt` row
per doubt (its statement, consequence if wrong and resolution).

Model and session text in either is flattened to one line, stripped of
control and format characters, its backticks replaced, cut to 200
characters and set in a code span; layer problems are this script's fixed
strings. Nothing else is ever printed to stdout.

Exits 0 for `auto` and 1 for `human`. Exits 2 on unusable input, with no
record unless a stale one could not be deleted: bad arguments, an unresolvable base ref, a partial
clone, a failed fetch into the object store (a forged or missing object, a shallow history), no config at it, an invalid
config, an unknown component, an unreadable plan, issue document or
analysis, git or yq missing or failing (reading the history too), a record that cannot be deleted or written, or any unexpected
error. A missing or unusable verdict or assessment is not unusable input: it
gives a record with that layer unusable. The interpreter itself exits 1 when
it crashes outside main(), so exit 0 counts only together with a record
whose `decision` is `auto`; exit 2 never comes with a valid record, and exit
2 or no record counts as `human`. Standard
library only, apart from running git and yq through floor.py.
"""

import hashlib
import importlib.util
import json
import os
import re
import sys
import tempfile
import unicodedata
from pathlib import Path
from types import ModuleType

RECORD = Path(".mintswarm/job/outputs/decision.json")
VERDICT = Path(".mintswarm/job/outputs/floor.json")
FLOOR = Path(__file__).resolve().parent / "floor.py"
TOOL = "mintswarm-approval-decision"
VERSION = 2
FLOOR_TOOL = "mintswarm-plan-floor"
FLOOR_VERSION = 2
FLOOR_KEYS = [
    "tool",
    "version",
    "plan",
    "issue_document",
    "analysis",
    "component",
    "base",
    "config",
    "mode",
    "verdict",
    "rules",
    "not_evaluated",
]
# The assess stage's named bands: identifier -> (display name, lowest, highest confidence).
BANDS = {
    "blocked": ("Blocked", 0, 0),
    "uncertain": ("Uncertain", 1, 49),
    "needs-review": ("Needs review", 50, 79),
    "likely-safe": ("Likely safe", 80, 94),
    "safe": ("Safe", 95, 100),
}
HARD_TRIGGERS = (
    "criterion-not-met",
    "outside-scope",
    "not-revertible",
    "credentials",
    "weakens-safeguard",
    "undeclared-high",
)
ENVELOPE_KEYS = ["confidence", "band", "hard_triggers", "reason", "doubts"]
DOUBT_KEYS = ["statement", "consequence", "resolution"]
MAX_DOUBTS = 10
FENCE_OPEN = "```json"
FENCE_CLOSE = "```"
SPAN_LIMIT = 200
ARGS = (
    "--issue",
    "--component",
    "--plan",
    "--issue-document",
    "--analysis",
    "--assessment",
    "--assessment-artifact",
    "--base",
    "--harness",
    "--model",
)
USAGE = (
    "usage: decision.py --issue N --component NAME --plan PATH --issue-document PATH "
    "--analysis PATH --assessment PATH --assessment-artifact REF --base REF "
    "--harness NAME --model NAME"
)
MODE_REASONS = {
    "always": "mode always: every plan goes to a human",
    "shadow": "mode shadow: every plan goes to a human; judged_decision is recorded for comparison",
    "judged": "mode judged: the decision is judged_decision",
    "never": "mode never: every plan is approved without a human",
}
SUMMARY = re.compile(r"(0|[1-9][0-9]{0,2}) confidence: (.*)", re.DOTALL)

floor: ModuleType  # floor.py, set by main() before anything uses it


class InputError(Exception):
    """Unusable input: exit 2, no record."""


def load_floor() -> ModuleType:
    sys.dont_write_bytecode = True  # leave no __pycache__ next to floor.py
    spec = importlib.util.spec_from_file_location("msw_plan_floor", FLOOR)
    if spec is None or spec.loader is None:
        raise InputError(f"cannot load {FLOOR}")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


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
    if not re.fullmatch(r"[1-9][0-9]*", args["--issue"]):
        raise InputError(f"--issue must be a positive decimal integer: {args['--issue']!r}")
    if args["--base"].startswith("-"):
        raise InputError(f"--base must not start with '-': {args['--base']!r}")
    return {
        "issue": int(args["--issue"]),
        "component": args["--component"],
        "plan": args["--plan"],
        "issue_document": args["--issue-document"],
        "analysis": args["--analysis"],
        "assessment": args["--assessment"],
        "artifact": args["--assessment-artifact"],
        "base": args["--base"],
        "harness": args["--harness"],
        "model": args["--model"],
    }


def expected_judge(config: dict) -> dict:
    """The assess harness and model the base config names; null where it names none."""
    agents = config.get("agents")
    agents = agents if isinstance(agents, dict) else {}
    default = agents.get("default")
    default = default if isinstance(default, dict) else {}
    stages = agents.get("stages")
    stages = stages if isinstance(stages, dict) else {}
    assess = stages.get("assess")
    assess = assess if isinstance(assess, dict) else {}
    expected = {}
    for key in ("harness", "model"):
        value = assess.get(key, default.get(key))
        expected[key] = value if isinstance(value, str) else None
    return expected


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


def bound(value, sha: str) -> bool:
    return isinstance(value, dict) and value.get("sha256") == sha


def verdict_problems(verdict, plan_sha: str, commit: str, component: str, mode: str, documents: dict) -> list:
    if not isinstance(verdict, dict):
        return ["the floor verdict is not a JSON object"]
    if list(verdict) != FLOOR_KEYS:
        return ["the floor verdict's keys are not floor.py's, in order"]
    problems = []
    if verdict["tool"] != FLOOR_TOOL or type(verdict["version"]) is not int or verdict["version"] != FLOOR_VERSION:
        problems.append("the floor verdict's tool or version is not floor.py's")
    plan = verdict["plan"]
    if not isinstance(plan, dict) or plan.get("sha256") != plan_sha:
        problems.append("the floor verdict's plan.sha256 does not match the plan")
    if not bound(verdict["issue_document"], documents["issue_document"]):
        problems.append("the floor verdict's issue_document does not match --issue-document")
    if not bound(verdict["analysis"], documents["analysis"]):
        problems.append("the floor verdict's analysis does not match --analysis")
    base = verdict["base"]
    if not isinstance(base, dict) or base.get("commit") != commit:
        problems.append("the floor verdict's base.commit does not match the base")
    if verdict["component"] != component:
        problems.append("the floor verdict's component does not match --component")
    if verdict["mode"] != mode:
        problems.append("the floor verdict's mode does not match the base config")
    rules = verdict["rules"]
    skipped = verdict["not_evaluated"]
    if not isinstance(rules, list) or not all(is_rule(r) for r in rules):
        problems.append("the floor verdict's rules are not well formed")
        return problems
    if not isinstance(skipped, list) or not all(isinstance(s, str) for s in skipped):
        problems.append("the floor verdict's not_evaluated is not a list of rule identifiers")
        return problems
    names = [r["rule"] for r in rules]
    if skipped not in ([], list(floor.FOOTPRINT_RULES)) or names != [n for n in floor.RULES if n not in skipped]:
        problems.append(
            "the floor verdict's rule identifiers are not floor.py's, in order, "
            "with none or only the footprint rules not evaluated"
        )
        return problems
    if skipped and rules[0]["result"] != "hit":
        problems.append("the floor verdict skips rules without a footprint hit")
    hit = any(r["result"] == "hit" for r in rules)
    if verdict["verdict"] != ("hit" if hit else "clean"):
        problems.append("the floor verdict's verdict does not agree with its rules")
    return problems


def floor_rules(
    plan: bytes, issue: bytes, analysis: bytes, component: str, cfg: dict, commit: str, git_dir=None
) -> tuple:
    """What floor.py concludes for these inputs: (rules, not_evaluated, footprint or None).

    The history is read from git_dir, the object store, when given."""
    subjects = floor.history(commit, git_dir=git_dir)
    return floor.conclude(
        plan.decode("utf-8"), issue.decode("utf-8"), analysis.decode("utf-8"), component, cfg, subjects
    )


def floor_layer(plan_sha: str, commit: str, component: str, mode: str, documents: dict, expected: tuple) -> dict:
    layer = {
        "status": "unusable",
        "problems": [],
        "path": str(VERDICT),
        "sha256": None,
        "verdict": None,
        "rules": [],
        "not_evaluated": [],
    }
    try:
        data = VERDICT.read_bytes()
    except FileNotFoundError:
        layer["problems"] = ["the floor verdict is missing"]
        return layer
    except OSError:
        layer["problems"] = ["the floor verdict cannot be read"]
        return layer
    layer["sha256"] = sha256(data)
    try:
        verdict = floor.strict_json(data.decode("utf-8"))
    except (UnicodeDecodeError, ValueError, RecursionError):
        layer["problems"] = ["the floor verdict is not strict JSON"]
        return layer
    problems = verdict_problems(verdict, plan_sha, commit, component, mode, documents)
    if not problems and (verdict["rules"], verdict["not_evaluated"]) != expected:
        problems = ["the floor verdict's rules differ from what floor.py gives for this plan and base"]
    if problems:
        layer["problems"] = problems
        return layer
    layer.update(
        status="usable", verdict=verdict["verdict"], rules=verdict["rules"], not_evaluated=verdict["not_evaluated"]
    )
    return layer


# --- judgement layer -----------------------------------------------------


def is_text(value) -> bool:
    """A string that is not empty once whitespace, control and format characters are removed."""
    return isinstance(value, str) and bool(inert(value))


def envelope_fields(envelope, triggers=HARD_TRIGGERS, noun="assessment") -> list:
    """Problems with the envelope's parsed content, as fixed strings naming the document by `noun`."""
    if not isinstance(envelope, dict) or list(envelope) != ENVELOPE_KEYS:
        return [f"the {noun}'s envelope is not an object with exactly " + ", ".join(ENVELOPE_KEYS) + ", in order"]
    confidence = envelope["confidence"]
    band = envelope["band"]
    if type(confidence) is not int or not 0 <= confidence <= 100:
        return [f"the {noun}'s confidence is not an integer from 0 to 100"]
    if not isinstance(band, str) or band not in BANDS:
        return [f"the {noun}'s band is not one of " + ", ".join(BANDS)]
    _, low, high = BANDS[band]
    if not low <= confidence <= high:
        return [f"the {noun}'s confidence is outside its band's range"]
    hard = envelope["hard_triggers"]
    if not isinstance(hard, list) or not all(isinstance(t, str) and t in triggers for t in hard):
        return [f"the {noun}'s hard_triggers is not a list of known hard trigger identifiers"]
    if len(set(hard)) != len(hard):
        return [f"the {noun}'s hard_triggers repeats a trigger"]
    if bool(hard) != (band == "blocked"):
        return [f"the {noun}'s hard_triggers must be non-empty exactly when the band is blocked"]
    if not is_text(envelope["reason"]):
        return [f"the {noun}'s envelope gives no reason"]
    doubts = envelope["doubts"]
    if not isinstance(doubts, list) or len(doubts) > MAX_DOUBTS:
        return [f"the {noun}'s doubts is not a list of at most {MAX_DOUBTS}"]
    for doubt in doubts:
        if not isinstance(doubt, dict) or list(doubt) != DOUBT_KEYS or not all(is_text(doubt[k]) for k in DOUBT_KEYS):
            return ["a doubt is not an object with exactly non-empty " + ", ".join(DOUBT_KEYS) + ", in order"]
    if band == "likely-safe" and not doubts:
        return [f"the {noun}'s band is likely-safe without a doubt"]
    if band == "safe" and doubts:
        return [f"the {noun}'s band is safe with a doubt"]
    return []


def envelope_problems(text: str, triggers=HARD_TRIGGERS, noun="assessment") -> tuple:
    """(problems, fields) for a document's text; fields is the envelope's object, or None when unusable.

    `triggers` are the hard trigger identifiers the envelope may name and `noun` names the document in
    the problems; the defaults are the assess stage's.
    """
    lines = [line.rstrip("\r \t") for line in text.split("\n")]
    if any(line.startswith("Risk:") for line in lines):
        return [f"the {noun} has a line starting with 'Risk:'"], None
    content = [line for line in lines if line.strip()]
    if not content or not content[0].startswith("# "):
        return [f"the {noun} does not start with a '# ' title"], None
    summary = SUMMARY.fullmatch(content[1]) if len(content) > 1 else None
    if summary is None:
        return [f"the line after the {noun}'s title is not '<confidence> confidence: <reason>'"], None
    if not inert(summary.group(2)):
        return [f"the {noun}'s summary gives no reason"], None
    opens = [i for i, line in enumerate(lines) if line == FENCE_OPEN]
    if not opens:
        return [f"the {noun} has no envelope"], None
    if len(opens) > 1:
        return [f"the {noun} has more than one envelope"], None
    start = opens[0] + 1
    closes = [i for i in range(start, len(lines)) if lines[i] == FENCE_CLOSE]
    if not closes:
        return [f"the {noun}'s envelope is not closed"], None
    try:
        envelope = floor.strict_json("\n".join(lines[start : closes[0]]))
    except (ValueError, RecursionError):
        return [f"the {noun}'s envelope is not strict JSON"], None
    try:
        json.dumps(envelope, ensure_ascii=False).encode("utf-8")
    except UnicodeEncodeError:  # a \uD800-style escape decodes to a lone surrogate
        return [f"the {noun}'s envelope holds a string with a lone surrogate"], None
    problems = envelope_fields(envelope, triggers, noun)
    if problems:
        return problems, None
    if int(summary.group(1)) != envelope["confidence"]:
        return [f"the {noun}'s summary confidence differs from its envelope"], None
    return [], envelope


def judgement_layer(args: dict, expected: dict) -> dict:
    layer = {
        "status": "unusable",
        "problems": [],
        "stage": "assess",
        "path": args["assessment"],
        "sha256": None,
        "artifact": args["artifact"],
        "harness": args["harness"],
        "model": args["model"],
        "expected": expected,
        "as_configured": args["harness"] == expected["harness"] and args["model"] == expected["model"],
        "confidence": None,
        "band": None,
        "hard_triggers": [],
        "doubts": [],
        "reasons": [],
    }
    try:
        data = Path(args["assessment"]).read_bytes()
    except FileNotFoundError:
        layer["problems"] = ["the assessment is missing"]
        return layer
    except OSError:
        layer["problems"] = ["the assessment cannot be read"]
        return layer
    layer["sha256"] = sha256(data)
    try:
        text = data.decode("utf-8")
    except UnicodeDecodeError:
        layer["problems"] = ["the assessment is not UTF-8"]
        return layer
    problems, envelope = envelope_problems(text)
    if problems:
        layer["problems"] = problems
        return layer
    layer.update(
        status="usable",
        confidence=envelope["confidence"],
        band=envelope["band"],
        hard_triggers=envelope["hard_triggers"],
        doubts=envelope["doubts"],
        reasons=[envelope["reason"]],
    )
    return layer


# --- decision ------------------------------------------------------------


def floor_result(layer: dict) -> str:
    if layer["status"] != "usable":
        return "unusable"
    return layer["verdict"]


def hit_rules(layer: dict) -> list:
    return [r["rule"] for r in layer["rules"] if r["result"] == "hit"]


def decide(mode: str, min_confidence: int, floor_: dict, judgement: dict) -> tuple:
    clean = floor_result(floor_) == "clean"
    usable = judgement["status"] == "usable"
    confident = usable and not judgement["hard_triggers"] and judgement["confidence"] >= min_confidence
    judged = "auto" if clean and confident and judgement["as_configured"] else "human"
    decision = {"always": "human", "shadow": "human", "never": "auto", "judged": judged}[mode]
    reasons = [MODE_REASONS[mode]]
    result = floor_result(floor_)
    if result == "clean":
        reasons.append(f"floor: clean, {len(floor_['rules'])} of {len(floor.RULES)} rules")
    elif result == "hit":
        reasons.append(f"floor: hit on {', '.join(hit_rules(floor_))}")
    else:
        reasons.append("floor: unusable")
    if usable:
        reasons.append(f"judgement: confidence {judgement['confidence']} (band {judgement['band']})")
        if judgement["hard_triggers"]:
            reasons.append(f"judgement: hard trigger {', '.join(judgement['hard_triggers'])}")
        elif judgement["confidence"] < min_confidence:
            reasons.append(f"judgement: confidence below min_confidence {min_confidence}")
    else:
        reasons.append("judgement: unusable")
    if not judgement["as_configured"]:
        reasons.append("judgement: harness or model not as configured")
    reasons.append(f"judged_decision: {judged}")
    return judged, decision, reasons


# --- comment -------------------------------------------------------------


def inert(text) -> str:
    """Text on one line, without control or format characters or backticks."""
    flat = " ".join(str(text).split())
    flat = "".join(c for c in flat if unicodedata.category(c) not in ("Cc", "Cf", "Zl", "Zp"))
    return " ".join(flat.replace("`", "'").split())


def span(text) -> str:
    """Model or session text as one bounded, inert Markdown code span."""
    flat = inert(text)[:SPAN_LIMIT].strip()
    return f"`{flat}`" if flat else "`(empty)`"


def floor_line(layer: dict) -> str:
    result = floor_result(layer)
    if result == "clean":
        return f"clean, {len(layer['rules'])} of {len(floor.RULES)} rules"
    if result == "hit":
        return "hit on " + ", ".join(f"`{rule}`" for rule in hit_rules(layer))
    return "unusable"


def confidence_line(layer: dict, min_confidence: int) -> str:
    if layer["status"] == "usable":
        name = BANDS[layer["band"]][0]
        return f"`{name} · {layer['confidence']}` (min_confidence {min_confidence}), {span(layer['reasons'][0])}"
    return f"unusable (min_confidence {min_confidence})"


def judgement_rows(layer: dict, min_confidence: int, suffix: str = "") -> list:
    """The Confidence, Hard triggers and Doubt rows of a comment."""
    rows = [f"- **Confidence:** {confidence_line(layer, min_confidence)}{suffix}"]
    if layer["status"] != "usable":
        return rows
    triggers = ", ".join(f"`{t}`" for t in layer["hard_triggers"]) or "none"
    rows.append(f"- **Hard triggers:** {triggers}")
    for doubt in layer["doubts"]:
        rows.append(
            f"- **Doubt:** {span(doubt['statement'])}; if wrong: {span(doubt['consequence'])}; "
            f"resolved by: {span(doubt['resolution'])}"
        )
    return rows


def comment(record: dict) -> str:
    floor_ = record["layers"]["floor"]
    judgement = record["layers"]["judgement"]
    lines = [
        "### Plan approved without a human",
        "",
        "mintswarm's plan-approval decision approved the plan for this issue without a human.",
        "",
        f"- **Mode:** `{record['mode']}`, base commit `{record['subject']['base']['commit']}`",
        f"- **Floor:** {floor_line(floor_)}",
        *judgement_rows(judgement, record["min_confidence"]),
        f"- **Assessment:** {span(judgement['artifact'])}, judged by "
        f"{span(judgement['harness'])} with {span(judgement['model'])}",
        "",
        "**To veto:** a trusted author comments on this issue with `veto` as the first line. "
        "Until implement finishes, a veto cancels the job and returns the issue to awaiting plan approval; "
        "after that, it is a rejection at PR approval.",
        "",
        "PR approval stays with a human in every mode.",
    ]
    return "\n".join(lines) + "\n"


def footprint_brief(footprint) -> str:
    if footprint is None:
        return "not readable (a `footprint` hit)"
    return (
        f"{len(footprint['files'])} files ({len(footprint['new_files'])} new), "
        f"about {footprint['estimated_lines']} lines, "
        f"{len(footprint['new_dependencies'])} new dependencies"
    )


def problems_line(layer: dict) -> str:
    return "; ".join(layer["problems"])  # fixed strings from this script


def shadow_comment(record: dict, footprint) -> str:
    floor_ = record["layers"]["floor"]
    judgement = record["layers"]["judgement"]
    subject = record["subject"]
    configured = (
        "as configured"
        if judgement["as_configured"]
        else (
            f"not as configured: expected {span(judgement['expected']['harness'])} "
            f"with {span(judgement['expected']['model'])}"
        )
    )
    assess = (
        f"{span(judgement['harness'])} with {span(judgement['model'])}, {configured}; "
        f"artifact {span(judgement['artifact'])}"
    )
    if floor_["status"] == "usable":
        clean = [r["rule"] for r in floor_["rules"] if r["result"] == "clean"]
        floor_detail = (
            f"{floor_line(floor_)} (`floor.py` exit {0 if floor_['verdict'] == 'clean' else 1}); "
            f"clean: {', '.join(f'`{rule}`' for rule in clean) or 'none'}"
        )
        if floor_["not_evaluated"]:
            floor_detail += f"; not evaluated: {', '.join(f'`{rule}`' for rule in floor_['not_evaluated'])}"
    else:
        floor_detail = f"unusable: {problems_line(floor_)}"
    suffix = f": {problems_line(judgement)}" if judgement["status"] != "usable" else ""
    lines = [
        "### Shadow record: plan approval",
        "",
        "Mode `shadow`: this plan goes to the maintainer. Below is what plan approval judged, for comparison "
        "with the maintainer's decision.",
        "",
        f"- **Plan:** {span(subject['plan']['path'])}, sha256 `{subject['plan']['sha256']}`; "
        f"footprint: {footprint_brief(footprint)}",
        f"- **Assess:** stage `assess`, {assess}",
        *judgement_rows(judgement, record["min_confidence"], suffix),
        f"- **Floor:** at base commit `{subject['base']['commit']}` (`--base` {span(subject['base']['ref'])}): "
        f"{floor_detail}",
        f"- **Would-be decision:** `{record['judged_decision']}` (`judged_decision`)",
        f"- **Decision:** `{record['decision']}`",
        "- **Maintainer decision:** pending",
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
        fd, tmp = tempfile.mkstemp(prefix=".decision.", suffix=".json", dir=RECORD.parent)
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


def run(argv: list) -> int:
    try:
        remove(RECORD)
    except OSError as exc:
        raise InputError(f"cannot delete the stale record {RECORD}: {exc}") from exc
    args = parse_args(argv)
    try:
        with floor.job_object_store({"base": args["base"]}) as (store, commits):
            commit = commits["base"]
            config, cfg = floor.prepare(commit, args["component"], git_dir=store)
            plan = floor.read_plan(args["plan"])
            issue = floor.read_document(args["issue_document"], "issue document")
            analysis = floor.read_document(args["analysis"], "analysis")
            rules, skipped, footprint = floor_rules(plan, issue, analysis, args["component"], cfg, commit, store)
    except floor.InputError as exc:
        raise InputError(str(exc)) from exc
    mode = cfg["mode"]
    plan_sha = sha256(plan)
    documents = {"issue_document": sha256(issue), "analysis": sha256(analysis)}

    floor_ = floor_layer(plan_sha, commit, args["component"], mode, documents, (rules, skipped))
    judgement = judgement_layer(args, expected_judge(config))
    judged, decision, reasons = decide(mode, cfg["min_confidence"], floor_, judgement)
    record = {
        "tool": TOOL,
        "version": VERSION,
        "approval": "plan",
        "subject": {
            "issue": args["issue"],
            "component": args["component"],
            "plan": {"path": args["plan"], "sha256": plan_sha},
            "base": {"ref": args["base"], "commit": commit},
        },
        "mode": mode,
        "min_confidence": cfg["min_confidence"],
        "layers": {"floor": floor_, "judgement": judgement},
        "judged_decision": judged,
        "decision": decision,
        "reasons": reasons,
    }
    if decision == "auto":
        text = comment(record)
    elif mode == "shadow":
        text = shadow_comment(record, footprint)
    else:
        text = None
    write_record(record)
    if judgement["status"] == "usable":
        judged_as = f"{judgement['band']} {judgement['confidence']}"
    else:
        judged_as = "unusable"
    print(
        f"decision: {decision} (mode {mode}, floor {floor_result(floor_)}, " f"judgement {judged_as}, judged {judged})",
        file=sys.stderr,
    )
    if text is not None:
        sys.stdout.write(text)
        sys.stdout.flush()
    return 0 if decision == "auto" else 1


def main(argv: list) -> int:
    global floor
    try:
        floor = load_floor()
        return run(argv)
    except InputError as exc:
        print(f"decision.py: {exc}", file=sys.stderr)
        return 2
    except Exception as exc:  # a crash must never read as a decision
        try:
            remove(RECORD)
        except Exception:  # the handler itself must always end in exit 2
            pass
        message = " ".join(str(exc).split())
        print(f"decision.py: unexpected error: {type(exc).__name__}: {message}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
