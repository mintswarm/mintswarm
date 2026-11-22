#!/usr/bin/env python3
"""Decide whether a plan needs a human, from the floor verdict and the assessment.

    decision.py --issue N --component NAME --plan PATH --assessment PATH
                --assessment-artifact REF --base REF --harness NAME --model NAME

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

- REF given to --base, the base ref. The mode is
  `pipeline.plan_approval.mode` of .mintswarm/config.yaml at the commit it
  resolves to, read and validated by floor.py's own code (loaded from the
  file next to this one), so the worktree's config is never opened. NAME
  given to --component must be a defined component.
- PATH given to --plan, the plan; only its sha256 is used.
- The floor verdict, .mintswarm/job/outputs/floor.json, from floor.py.
- PATH given to --assessment, the assess stage's assessment.md, and REF
  given to --assessment-artifact, where it was published.
- --harness and --model, what the session actually ran assess on. They are
  compared with `agents.stages.assess` over `agents.default` in the base
  config; a mismatch is recorded and never changes the decision.

The floor layer is usable only if the verdict is strict JSON (no duplicate
keys, NaN or Infinity) and all of these hold: its keys are floor.py's, in
order; `tool` is `mintswarm-plan-floor` and `version` is 1; `plan.sha256`
is the sha256 of the plan's bytes; `base.commit` is the resolved base;
`component` is NAME; `mode` is the base config's mode; every rule is
{`rule`, `result`, `reasons`} with `result` `hit` or `clean` and string
reasons exactly when it is a hit; the identifiers across `rules` and
`not_evaluated` are floor.py's 13, in order; rules are skipped only after a
`footprint` hit; and `verdict` agrees with the rules. A usable verdict is
therefore clean only when all 13 rules ran and are clean.

The judgement layer is usable only if the assessment is UTF-8 and, with
lines split at LF only and trailing CR, space and tab removed from each:
exactly one line starts with `Risk:` at column 0 and it is exactly
`Risk: low`, `Risk: medium` or `Risk: high`; and the first non-blank line is
a `# ` title, and the next non-blank line is `<level> risk: <reason>` with
the same level and a reason that is not empty. That reason is the layer's
only entry in `reasons`.

Problems in either layer are fixed strings that never quote the files.

`judged_decision` is `auto` only when the floor layer is usable and clean
and the judgement layer is usable with risk `low`; otherwise `human`.
`decision` follows the mode: `always` and `shadow` give `human`, `never`
gives `auto`, and `judged` gives `judged_decision`. Both layers are always
read and recorded, whatever the mode.

The record is .mintswarm/job/outputs/decision.json, written to a temporary
file and renamed onto it, with these keys in this order: `tool`
(`mintswarm-approval-decision`), `version` (1), `approval` (`plan`),
`subject` ({`issue`, `component`, `plan` {`path`, `sha256`}, `base`
{`ref`, `commit`}}), `mode`, `layers`, `judged_decision`, `decision` and
`reasons` (the mode's rule and each layer's outcome). `layers.floor` holds
`status` (`usable` or `unusable`), `problems`, `path`, `sha256` (of the
verdict's bytes), `verdict`, `rules` (the verdict's rule objects,
unchanged) and `not_evaluated`. `layers.judgement` holds `status`,
`problems`, `stage` (`assess`), `path`, `sha256`, `artifact`, `harness`,
`model`, `expected` ({`harness`, `model`}), `as_configured`, `risk` and
`reasons`. A field that cannot be filled, including the content of an
unusable layer, is null, or [] for a list.

One status line goes to stderr. For `auto` only, after the record is
written, the issue comment the session posts goes to stdout: it says the
plan was approved without a human, the mode and base commit, the floor
result, the risk with its reason, the assessment artifact and who judged,
and how to veto. Model and session text in it is flattened to one line,
stripped of control characters, its backticks replaced, cut to 200
characters and set in a code span. Nothing else is ever printed to stdout.

Exits 0 for `auto` and 1 for `human`. Exits 2 on unusable input, with no
record: bad arguments, an unresolvable base ref, no config at it, an invalid
config, an unknown component, an unreadable plan, git or yq missing or
failing, a record that cannot be deleted or written, or any unexpected
error. A missing or unusable verdict or assessment is not unusable input: it
gives a record with that layer unusable. The interpreter itself exits 1 when
it crashes outside main(), so exit 0 counts only together with a record
whose `decision` is `auto`; exit 2 or no record counts as `human`. Standard
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

RECORD = Path(".mintswarm/job/outputs/decision.json")
VERDICT = Path(".mintswarm/job/outputs/floor.json")
FLOOR = Path(__file__).resolve().parent / "floor.py"
TOOL = "mintswarm-approval-decision"
VERSION = 1
FLOOR_TOOL = "mintswarm-plan-floor"
FLOOR_VERSION = 1
FLOOR_KEYS = [
    "tool", "version", "plan", "component", "base", "config", "mode",
    "verdict", "rules", "not_evaluated",
]
RISKS = ("low", "medium", "high")
SPAN_LIMIT = 200
ARGS = (
    "--issue", "--component", "--plan", "--assessment", "--assessment-artifact",
    "--base", "--harness", "--model",
)
USAGE = (
    "usage: decision.py --issue N --component NAME --plan PATH --assessment PATH "
    "--assessment-artifact REF --base REF --harness NAME --model NAME"
)
MODE_REASONS = {
    "always": "mode always: every plan goes to a human",
    "shadow": "mode shadow: every plan goes to a human; judged_decision is recorded for comparison",
    "judged": "mode judged: the decision is judged_decision",
    "never": "mode never: every plan is approved without a human",
}
SUMMARY = re.compile(r"(low|medium|high) risk: (.*)", re.DOTALL)

floor = None  # floor.py, loaded by main()


class InputError(Exception):
    """Unusable input: exit 2, no record."""


def load_floor():
    sys.dont_write_bytecode = True  # leave no __pycache__ next to floor.py
    spec = importlib.util.spec_from_file_location("msw_plan_floor", FLOOR)
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


def verdict_problems(verdict, plan_sha: str, commit: str, component: str, mode: str) -> list:
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
    if [r["rule"] for r in rules] + skipped != list(floor.RULES):
        problems.append("the floor verdict's rule identifiers are not floor.py's 13, in order")
        return problems
    if skipped and not (len(rules) == 1 and rules[0]["result"] == "hit"):
        problems.append("the floor verdict skips rules without a footprint hit")
    hit = any(r["result"] == "hit" for r in rules)
    if verdict["verdict"] != ("hit" if hit else "clean"):
        problems.append("the floor verdict's verdict does not agree with its rules")
    return problems


def floor_layer(plan_sha: str, commit: str, component: str, mode: str) -> dict:
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
    problems = verdict_problems(verdict, plan_sha, commit, component, mode)
    if problems:
        layer["problems"] = problems
        return layer
    layer.update(status="usable", verdict=verdict["verdict"], rules=verdict["rules"],
                 not_evaluated=verdict["not_evaluated"])
    return layer


# --- judgement layer -----------------------------------------------------

def assessment_problems(text: str) -> tuple:
    """(problems, risk, reason) for an assessment's text."""
    lines = [line.rstrip("\r \t") for line in text.split("\n")]
    risk_lines = [line for line in lines if line.startswith("Risk:")]
    if not risk_lines:
        return ["the assessment has no line starting with 'Risk:'"], None, None
    if len(risk_lines) > 1:
        return ["the assessment has more than one line starting with 'Risk:'"], None, None
    values = {f"Risk: {level}": level for level in RISKS}
    if risk_lines[0] not in values:
        return ["the assessment's Risk: line is not 'Risk: low', 'Risk: medium' or 'Risk: high'"], None, None
    risk = values[risk_lines[0]]
    content = [line for line in lines if line.strip()]
    if not content or not content[0].startswith("# "):
        return ["the assessment does not start with a '# ' title"], None, None
    summary = SUMMARY.fullmatch(content[1]) if len(content) > 1 else None
    if summary is None:
        return ["the line after the assessment's title is not '<level> risk: <reason>'"], None, None
    if summary.group(1) != risk:
        return ["the assessment's summary level differs from its Risk: line"], None, None
    reason = summary.group(2).strip()
    if not reason:
        return ["the assessment's summary gives no reason"], None, None
    return [], risk, reason


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
        "risk": None,
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
    problems, risk, reason = assessment_problems(text)
    if problems:
        layer["problems"] = problems
        return layer
    layer.update(status="usable", risk=risk, reasons=[reason])
    return layer


# --- decision ------------------------------------------------------------

def floor_result(layer: dict) -> str:
    if layer["status"] != "usable":
        return "unusable"
    return layer["verdict"]


def hit_rules(layer: dict) -> list:
    return [r["rule"] for r in layer["rules"] if r["result"] == "hit"]


def decide(mode: str, floor_: dict, judgement: dict) -> tuple:
    clean = floor_result(floor_) == "clean"
    low = judgement["status"] == "usable" and judgement["risk"] == "low"
    judged = "auto" if clean and low else "human"
    decision = {"always": "human", "shadow": "human", "never": "auto", "judged": judged}[mode]
    reasons = [MODE_REASONS[mode]]
    result = floor_result(floor_)
    if result == "clean":
        reasons.append(f"floor: clean, {len(floor_['rules'])} of {len(floor.RULES)} rules")
    elif result == "hit":
        reasons.append(f"floor: hit on {', '.join(hit_rules(floor_))}")
    else:
        reasons.append("floor: unusable")
    if judgement["status"] == "usable":
        reasons.append(f"judgement: risk {judgement['risk']}")
    else:
        reasons.append("judgement: unusable")
    reasons.append(f"judged_decision: {judged}")
    return judged, decision, reasons


# --- comment -------------------------------------------------------------

def span(text) -> str:
    """Model or session text as one bounded, inert Markdown code span."""
    flat = " ".join(str(text).split())
    flat = "".join(c for c in flat if unicodedata.category(c) not in ("Cc", "Cf", "Zl", "Zp"))
    flat = flat.replace("`", "'")[:SPAN_LIMIT].strip()
    return f"`{flat}`" if flat else "`(empty)`"


def comment(record: dict) -> str:
    layers = record["layers"]
    floor_ = layers["floor"]
    judgement = layers["judgement"]
    result = floor_result(floor_)
    if result == "clean":
        floor_line = f"clean, {len(floor_['rules'])} of {len(floor.RULES)} rules"
    elif result == "hit":
        floor_line = "hit on " + ", ".join(f"`{rule}`" for rule in hit_rules(floor_))
    else:
        floor_line = "unusable"
    if judgement["status"] == "usable":
        risk_line = f"`{judgement['risk']}`, {span(judgement['reasons'][0])}"
    else:
        risk_line = "unusable"
    lines = [
        "### Plan approved without a human",
        "",
        "mintswarm's plan-approval decision approved the plan for this issue without a human.",
        "",
        f"- **Mode:** `{record['mode']}`, base commit `{record['subject']['base']['commit']}`",
        f"- **Floor:** {floor_line}",
        f"- **Risk:** {risk_line}",
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
        commit = floor.resolve_base(args["base"])
        config = floor.load_config(commit)
        mode = floor.validate_config(config, args["component"])["mode"]
        plan = floor.read_plan(args["plan"])
    except floor.InputError as exc:
        raise InputError(str(exc)) from exc
    plan_sha = sha256(plan)

    floor_ = floor_layer(plan_sha, commit, args["component"], mode)
    judgement = judgement_layer(args, expected_judge(config))
    judged, decision, reasons = decide(mode, floor_, judgement)
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
        "layers": {"floor": floor_, "judgement": judgement},
        "judged_decision": judged,
        "decision": decision,
        "reasons": reasons,
    }
    text = comment(record) if decision == "auto" else None
    write_record(record)
    print(f"decision: {decision} (mode {mode}, floor {floor_result(floor_)}, "
          f"judgement {judgement['risk'] or 'unusable'}, judged {judged})", file=sys.stderr)
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
