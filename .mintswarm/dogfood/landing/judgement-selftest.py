#!/usr/bin/env python3
"""Self-test for landing/judgement.py, the landing stage's judgement layer.

    python3 .mintswarm/dogfood/landing/judgement-selftest.py

Table-driven, standard library only and offline; it writes no files.
Landing documents are built in memory and read through judgement.py, which
is loaded by its file path. The cases:

- bands: each band at its lowest and highest value is usable, a value just
  outside each band is not, `likely-safe` needs a doubt and `safe` has none;
- hard triggers: each unusable landing trigger alone at `blocked` 0 is
  usable; each authority trigger is usable at the lowest and highest value
  of every other band; `blocked` without a trigger, `blocked` with only
  authority triggers, an unusable trigger outside `blocked`, a repeated
  trigger and the plan-only `outside-scope` and `undeclared-high`, and
  `require-human`, are not; AUTHORITY_TRIGGERS are the landing triggers
  that are decision.py's authority triggers, in order;
- the threshold, through judged_decision: the confidence against
  min_confidence at the edges, and a hard trigger, each authority trigger
  at `safe` 100, an unusable layer, a `hit` floor and a missing floor; a min_confidence that floor.py would
  refuse (None, 89, True, 101, 0) is `human`, and judgement.py's
  MIN_CONFIDENCE, taken from floor.py, is 90;
- malformed and repeated envelopes, each unusable with a fixed problem that
  names the landing document;
- consumable as is: judgement() gives the envelope's own values, with the
  keys of plan-approval/decision.py's judgement layer, in its order;
- stage agreement: the band ranges under "## Bands" in
  .mintswarm/stages/landing.md are judgement.py's, its envelope template has
  the envelope keys in order, and no line of it starts with `Risk:`; in
  landing.md and assess.md the unusable triggers under band 1 and the
  authority triggers listed before the bands are judgement.py's and
  decision.py's respectively, in order, "## Bands" says an authority
  trigger does not change the confidence or the band, and "## Output" says
  the band is `blocked` exactly when an unusable trigger is listed, without
  the old non-empty rule; under "## Bands" in both the value starts at the
  band's highest value and only goes down, with no reasons for raising it
  and none of the old "starts at the lowest value" wording, and only the
  landing `weakens-safeguard` trigger excepts a declared change; in landing.md
  that exception covers only a declared gate or floor change, never a gate
  that failed and was then changed, a step that only names the file does not
  declare it, and the Care ground and the likely-safe condition except a
  declared gate and what a listed `weakens-safeguard` names; in assess.md
  the likely-safe revert condition excepts what a listed `not-revertible`
  names;
- decomposition stage agreement: the band lines under "## Bands" in
  .mintswarm/stages/assess-decomposition.md (identifier, display name and
  range) are assess.md's, in order, with both start-at-top phrases; its
  hard triggers are exactly criterion-uncovered, contradicts-design,
  outside-scope and work-dropped; its envelope template has the envelope
  keys in order and the stage has one json fence; "## Inputs" lists
  exactly the four inputs and says the decomposer's reasoning is never
  received; the template has the five dimension headings; and "## Bands"
  says require_human and floor hits do not lower the confidence.

Exits 0 when every case passes, 1 otherwise.
"""

import ast
import copy
import importlib.util
import json
import re
import sys
from pathlib import Path

sys.dont_write_bytecode = True  # leave no __pycache__ in the source tree

HERE = Path(__file__).resolve().parent
JUDGEMENT = HERE / "judgement.py"
STAGE = HERE.parent.parent / "stages" / "landing.md"
ASSESS = HERE.parent.parent / "stages" / "assess.md"
DECOMPOSITION = HERE.parent.parent / "stages" / "assess-decomposition.md"
NOUN = "landing document"


class Tally:
    def __init__(self):
        self.passed = 0
        self.failed = 0

    def check(self, name: str, ok: bool, detail: str = "") -> None:
        if ok:
            self.passed += 1
            print(f"ok   {name}")
        else:
            self.failed += 1
            print(f"FAIL {name}: {detail}")


T = Tally()


def load_judgement():
    spec = importlib.util.spec_from_file_location("msw_landing_judgement_for_selftest", JUDGEMENT)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


J = load_judgement()

DOUBT = {"statement": "the helper is unused", "consequence": "dead code lands", "resolution": "grep for callers"}


def envelope(band: str, confidence: int, triggers=None, doubts=None, reason="the change does what the plan says"):
    if triggers is None:
        triggers = ["criterion-not-met"] if band == "blocked" else []
    if doubts is None:
        doubts = [DOUBT] if band == "likely-safe" else []
    return {"confidence": confidence, "band": band, "hard_triggers": triggers, "reason": reason, "doubts": doubts}


def document(env, summary=None, body: str = None, raw_envelope: str = None) -> str:
    """A landing document around an envelope; summary defaults to the envelope's confidence."""
    if summary is None:
        summary = f"{env['confidence']} confidence: {env['reason']}" if isinstance(env, dict) else "90 confidence: x"
    fenced = raw_envelope if raw_envelope is not None else json.dumps(env, indent=2, ensure_ascii=False)
    rest = body if body is not None else "## Acceptance criteria\n## Conformance\n## Notes\nNone.\n"
    return f"# Landing: a change\n\n{summary}\n\n## Envelope\n\n```json\n{fenced}\n```\n\n{rest}"


def judge(text) -> dict:
    return J.judgement(text if isinstance(text, bytes) else text.encode("utf-8"))


def expect_usable(name: str, text) -> dict:
    layer = judge(text)
    T.check(f"{name}: usable", layer["status"] == "usable" and layer["problems"] == [], json.dumps(layer["problems"]))
    return layer


def expect_unusable(name: str, text, problem: str = None) -> None:
    layer = judge(text)
    problems = layer["problems"]
    ok = (
        layer["status"] == "unusable"
        and len(problems) == 1
        and (problem is None or problems[0] == problem)
        and (problem is not None or NOUN in problems[0])
        and all(layer[k] is None for k in ("confidence", "band"))
        and all(layer[k] == [] for k in ("hard_triggers", "doubts", "reasons"))
    )
    T.check(f"{name}: unusable", ok, json.dumps(layer, ensure_ascii=True))


# --- groups ---------------------------------------------------------------


def check_bands() -> None:
    for band, (_, low, high) in J.BANDS.items():
        for value in sorted({low, high}):
            expect_usable(f"band {band} at {value}", document(envelope(band, value)))
        outside = [v for v in (low - 1, high + 1) if 0 <= v <= 100]
        for value in outside:
            expect_unusable(
                f"band {band} at {value}",
                document(envelope(band, value)),
                f"the {NOUN}'s confidence is outside its band's range",
            )
    expect_unusable(
        "likely-safe without a doubt",
        document(envelope("likely-safe", 85, doubts=[])),
        f"the {NOUN}'s band is likely-safe without a doubt",
    )
    expect_unusable(
        "safe with a doubt",
        document(envelope("safe", 97, doubts=[DOUBT])),
        f"the {NOUN}'s band is safe with a doubt",
    )


UNUSABLE_TRIGGERS = tuple(t for t in J.HARD_TRIGGERS if t not in J.AUTHORITY_TRIGGERS)


def check_triggers() -> None:
    exactly = f"the {NOUN}'s band must be blocked exactly when hard_triggers lists an unusable trigger"
    for trigger in UNUSABLE_TRIGGERS:
        layer = expect_usable(f"trigger {trigger} alone", document(envelope("blocked", 0, [trigger])))
        T.check(f"trigger {trigger}: recorded", layer["hard_triggers"] == [trigger], json.dumps(layer))
        for band, value in (("uncertain", 30), ("needs-review", 60), ("likely-safe", 85), ("safe", 100)):
            expect_unusable(
                f"unusable trigger {trigger} in {band} {value}", document(envelope(band, value, [trigger])), exactly
            )
    for trigger in J.AUTHORITY_TRIGGERS:
        for band, (_, low, high) in J.BANDS.items():
            if band == "blocked":
                continue
            for value in sorted({low, high}):
                layer = expect_usable(
                    f"authority trigger {trigger} in {band} {value}", document(envelope(band, value, [trigger]))
                )
                T.check(
                    f"authority trigger {trigger} in {band} {value}: recorded",
                    layer["hard_triggers"] == [trigger] and layer["confidence"] == value,
                    json.dumps(layer),
                )
        expect_unusable(
            f"authority trigger {trigger} alone in blocked", document(envelope("blocked", 0, [trigger])), exactly
        )
    expect_unusable(
        "blocked with only authority triggers",
        document(envelope("blocked", 0, list(J.AUTHORITY_TRIGGERS))),
        exactly,
    )
    expect_usable("every landing trigger together", document(envelope("blocked", 0, list(J.HARD_TRIGGERS))))
    expect_usable("every authority trigger in safe 100", document(envelope("safe", 100, list(J.AUTHORITY_TRIGGERS))))
    expect_unusable("blocked without a trigger", document(envelope("blocked", 0, [])), exactly)
    expect_unusable(
        "a trigger in needs-review", document(envelope("needs-review", 60, ["process-not-passed"])), exactly
    )
    expect_unusable(
        "an unusable and an authority trigger in safe",
        document(envelope("safe", 100, ["credentials", "process-not-passed"])),
        exactly,
    )
    expect_unusable(
        "a repeated trigger",
        document(envelope("blocked", 0, ["credentials", "credentials"])),
        f"the {NOUN}'s hard_triggers repeats a trigger",
    )
    unknown = f"the {NOUN}'s hard_triggers is not a list of known hard trigger identifiers"
    for trigger in ("outside-scope", "undeclared-high", "require-human"):
        expect_unusable(
            f"trigger {trigger} is not a landing trigger", document(envelope("blocked", 0, [trigger])), unknown
        )
    T.check(
        "triggers: eight, unique",
        len(J.HARD_TRIGGERS) == 8 and len(set(J.HARD_TRIGGERS)) == 8,
        repr(J.HARD_TRIGGERS),
    )
    T.check(
        "authority triggers: three, unique, all landing triggers",
        len(J.AUTHORITY_TRIGGERS) == 3
        and len(set(J.AUTHORITY_TRIGGERS)) == 3
        and set(J.AUTHORITY_TRIGGERS) <= set(J.HARD_TRIGGERS),
        repr(J.AUTHORITY_TRIGGERS),
    )
    T.check(
        "authority triggers: the landing triggers that are decision.py's authority triggers, in order",
        J.AUTHORITY_TRIGGERS == tuple(t for t in J.HARD_TRIGGERS if t in J.decision.AUTHORITY_TRIGGERS),
        f"{J.AUTHORITY_TRIGGERS} against {J.decision.AUTHORITY_TRIGGERS}",
    )


def check_threshold() -> None:
    def layer(band: str, confidence: int, triggers=None):
        return judge(document(envelope(band, confidence, triggers)))

    cases = [
        ("94 at 95", "clean", layer("likely-safe", 94), 95, "human"),
        ("95 at 95", "clean", layer("safe", 95), 95, "auto"),
        ("89 at 90", "clean", layer("likely-safe", 89), 90, "human"),
        ("90 at 90", "clean", layer("likely-safe", 90), 90, "auto"),
        ("99 at 100", "clean", layer("safe", 99), 100, "human"),
        ("100 at 100", "clean", layer("safe", 100), 100, "auto"),
        ("a hard trigger", "clean", layer("blocked", 0, ["not-revertible"]), 90, "human"),
        ("an unusable layer", "clean", judge("not a landing document"), 90, "human"),
        ("a hit floor", "hit", layer("safe", 100), 90, "human"),
        ("a missing floor", None, layer("safe", 100), 90, "human"),
        ("a None threshold", "clean", layer("safe", 100), None, "human"),
        ("an 89 threshold", "clean", layer("safe", 100), 89, "human"),
        ("a True threshold", "clean", layer("safe", 100), True, "human"),
        ("a 101 threshold", "clean", layer("safe", 100), 101, "human"),
        ("a 0 threshold", "clean", layer("likely-safe", 90), 0, "human"),
    ]
    for name, floor_verdict, judged, minimum, want in cases:
        got = J.judged_decision(floor_verdict, judged, minimum)
        T.check(f"threshold {name}: {want}", got == want, f"got {got}")
    T.check("threshold: the lowest allowed is 90", J.MIN_CONFIDENCE == 90, repr(J.MIN_CONFIDENCE))
    for trigger in J.AUTHORITY_TRIGGERS:
        got = J.judged_decision("clean", layer("safe", 100, [trigger]), 90)
        T.check(f"threshold: authority trigger {trigger} at safe 100 is human", got == "human", f"got {got}")
    forged = dict(layer("safe", 100), hard_triggers=["credentials"])
    T.check(
        "threshold: a usable layer that names a trigger is human",
        J.judged_decision("clean", forged, 90) == "human",
        "",
    )


def check_malformed() -> None:
    good = envelope("safe", 97)
    text = document(good)
    body = json.dumps(good, indent=2)
    cases = [
        ("no envelope", text.replace("```json\n", "```text\n"), f"the {NOUN} has no envelope"),
        (
            "two json fences",
            text + "\n```json\n{}\n```\n",
            f"the {NOUN} has more than one envelope",
        ),
        ("an unclosed fence", document(good, body="").rstrip("\n").rsplit("```", 1)[0], None),
        ("not strict JSON", document(good, raw_envelope=body.rstrip("}") + ",}"), None),
        (
            "a duplicate key",
            document(good, raw_envelope=body.replace('"band"', '"confidence": 97,\n  "band"', 1)),
            None,
        ),
        ("NaN", document(good, raw_envelope=body.replace('"confidence": 97', '"confidence": NaN')), None),
        ("a lone surrogate", document(good, raw_envelope=body.replace("the change", "\\ud800 change")), None),
        (
            "keys out of order",
            document({k: good[k] for k in ["band", "confidence", "hard_triggers", "reason", "doubts"]}),
            None,
        ),
        ("an extra key", document(dict(good, risk="low")), None),
        ("a boolean confidence", document(dict(good, confidence=True), summary="1 confidence: x"), None),
        ("a band that is not a string", document(dict(good, band=5)), None),
        ("a band range", document(dict(good, band="95-100")), None),
        ("a band display name", document(dict(good, band="Safe")), None),
        ("a summary confidence that differs", document(good, summary="96 confidence: close"), None),
        ("a missing summary", document(good, summary="## Envelope"), None),
        ("a blank reason", document(dict(good, reason=" \t​")), None),
        (
            "a malformed doubt",
            document(envelope("likely-safe", 85, doubts=[{"statement": "s", "consequence": "c"}])),
            "a doubt is not an object with exactly non-empty statement, consequence, resolution, in order",
        ),
        ("11 doubts", document(envelope("likely-safe", 85, doubts=[DOUBT] * 11)), None),
        (
            "a line starting with Risk:",
            text + "Risk: low\n",
            f"the {NOUN} has a line starting with 'Risk:'",
        ),
        ("not UTF-8", text.encode("utf-8") + b"\xff\xfe", f"the {NOUN} is not UTF-8"),
        ("no title", text.replace("# Landing: a change\n", ""), None),
        ("empty", "", None),
    ]
    for name, case, problem in cases:
        expect_unusable(f"malformed {name}", case, problem)


def decision_layer_keys() -> list:
    """The keys of decision.py's judgement layer, in order, from its source."""
    tree = ast.parse(J.DECISION.read_text(encoding="utf-8"))
    for node in ast.walk(tree):
        if isinstance(node, ast.FunctionDef) and node.name == "judgement_layer":
            for stmt in node.body:
                if isinstance(stmt, ast.Assign) and isinstance(stmt.value, ast.Dict):
                    return [k.value for k in stmt.value.keys]
    return []


def check_consumable() -> None:
    doubts = [DOUBT, dict(DOUBT, statement="the count is close")]
    for env in (
        envelope("likely-safe", 88, doubts=doubts),
        envelope("safe", 100),
        envelope("blocked", 0, ["outside-footprint", "process-not-passed"], doubts=[DOUBT]),
        envelope("needs-review", 50),
        envelope("uncertain", 49, doubts=[DOUBT]),
    ):
        before = copy.deepcopy(env)
        layer = judge(document(env))
        want = {
            "status": "usable",
            "problems": [],
            "confidence": before["confidence"],
            "band": before["band"],
            "hard_triggers": before["hard_triggers"],
            "doubts": before["doubts"],
            "reasons": [before["reason"]],
        }
        T.check(
            f"consumable {env['band']} {env['confidence']}: the envelope's values", layer == want, json.dumps(layer)
        )
    keys = decision_layer_keys()
    ours = list(judge(document(envelope("safe", 95))))
    T.check(
        "consumable: keys are decision.py's judgement layer's, in its order",
        ours == J.LAYER_KEYS and set(ours) <= set(keys) and [k for k in keys if k in ours] == ours,
        f"ours {ours}, decision.py {keys}",
    )
    T.check(
        "consumable: an unusable layer has the same keys",
        list(judge(b"\xff")) == ours,
        json.dumps(list(judge(b"\xff"))),
    )
    T.check(
        "consumable: bands, envelope and doubt keys are decision.py's",
        J.BANDS is J.decision.BANDS
        and J.ENVELOPE_KEYS is J.decision.ENVELOPE_KEYS
        and J.DOUBT_KEYS is J.decision.DOUBT_KEYS,
        "",
    )


def section(lines: list, title: str, until: str) -> list:
    """The lines from the heading `title` to the heading `until`, which the template's own headings do not end."""
    start = lines.index(title) + 1
    return lines[start : lines.index(until, start)]


def trigger_lists(path: Path) -> tuple:
    """(band 1's unusable triggers, the authority triggers) as listed under "## Bands" in a stage."""
    bands_section = section(path.read_text(encoding="utf-8").split("\n"), "## Bands", "## Output")
    start = next(i for i, line in enumerate(bands_section) if line.startswith("1. `blocked`"))
    end = next(i for i, line in enumerate(bands_section) if line.startswith("2. "))
    unusable = tuple(m.group(1) for line in bands_section[start:end] if (m := re.match(r"   - `([a-z-]+)`:", line)))
    heads = [i for i, line in enumerate(bands_section) if line == "The authority triggers:"]
    listed = ()
    if len(heads) == 1:
        rest = bands_section[heads[0] + 1 :]
        items = []
        for line in rest[1:] if rest and rest[0] == "" else rest:
            m = re.match(r"- `([a-z-]+)`:", line)
            if not m:
                break
            items.append(m.group(1))
        listed = tuple(items)
    return unusable, listed


def check_stage() -> None:
    text = STAGE.read_text(encoding="utf-8")
    lines = text.split("\n")
    T.check(
        "stage: no line starts with Risk:",
        not any(line.startswith("Risk:") for line in lines),
        "",
    )
    T.check(
        "stage: no Rubric or level",
        "## Rubric" not in lines and "Risk: low" not in text and "<level>" not in text,
        "",
    )
    bands_section = section(lines, "## Bands", "## Output")
    bands = {}
    for line in bands_section:
        match = re.match(r"\d\. `([a-z-]+)` \(shown as ([^)]+)\), confidence (\d+)(?: to (\d+))?,", line)
        if match:
            low = int(match.group(3))
            high = int(match.group(4)) if match.group(4) else low
            bands[match.group(1)] = (match.group(2), low, high)
    T.check("stage: band ranges are judgement.py's", bands == J.BANDS, f"stage {bands}, BANDS {J.BANDS}")
    triggers = ()
    for name, path, hard, authority in (
        ("landing", STAGE, J.HARD_TRIGGERS, J.AUTHORITY_TRIGGERS),
        ("assess", ASSESS, J.decision.HARD_TRIGGERS, J.decision.AUTHORITY_TRIGGERS),
    ):
        unusable, listed = trigger_lists(path)
        triggers += unusable + listed
        want = tuple(t for t in hard if t not in authority)
        T.check(
            f"stage: {name} band 1 lists the unusable triggers, in order",
            unusable == want,
            f"stage {unusable}, code {want}",
        )
        T.check(
            f"stage: {name} lists the authority triggers, in order",
            listed == authority,
            f"stage {listed}, code {authority}",
        )
        stage_lines = path.read_text(encoding="utf-8").split("\n")
        stage_bands = "\n".join(section(stage_lines, "## Bands", "## Output"))
        T.check(
            f"stage: {name} says an authority trigger does not change the confidence or the band",
            "does not change the confidence or the band" in stage_bands,
            "",
        )
        stage_output = "\n".join(section(stage_lines, "## Output", "## Status"))
        T.check(
            f"stage: {name} Output ties blocked to an unusable trigger",
            "exactly when it lists an unusable trigger" in stage_output
            and "non-empty exactly when the band is `blocked`" not in stage_output,
            "",
        )
    T.check(
        "stage: require_human is not a trigger",
        "require-human" not in triggers and "require_human" not in triggers,
        repr(triggers),
    )
    output = section(lines, "## Output", "## Status")
    opens = [i for i, line in enumerate(output) if line == "```json"]
    T.check("stage: one json fence in the stage", [line for line in lines if line == "```json"] == ["```json"], "")
    keys = []
    if len(opens) == 1:
        close = next(i for i in range(opens[0] + 1, len(output)) if output[i] == "```")
        keys = [m.group(1) for line in output[opens[0] + 1 : close] if (m := re.match(r'  "([a-z_]+)":', line))]
    T.check("stage: the envelope template has the envelope keys, in order", keys == J.ENVELOPE_KEYS, repr(keys))
    for heading in ("## Envelope", "## Acceptance criteria", "## Conformance", "## Notes"):
        T.check(f"stage: the template has {heading}", heading in output, "")


def check_stage_rules() -> None:
    for name, path in (("landing", STAGE), ("assess", ASSESS)):
        bands_section = section(path.read_text(encoding="utf-8").split("\n"), "## Bands", "## Output")
        text = "\n".join(bands_section)
        for phrase in (
            "starts at the band's highest value and only goes down",
            "State no reasons for raising the value",
        ):
            T.check(f"stage rules: {name} says {phrase!r}", phrase in text, "")
        for phrase in ("starts at the band's lowest value", "Raise it only"):
            T.check(f"stage rules: {name} no longer says {phrase!r}", phrase not in text, "")
        trigger = [line for line in bands_section if line.startswith("- `weakens-safeguard`:")]
        declared = len(trigger) == 1 and "declared" in trigger[0]
        T.check(
            f"stage rules: {name} weakens-safeguard {'excepts' if name == 'landing' else 'keeps'} a declared change",
            len(trigger) == 1 and declared == (name == "landing"),
            repr(trigger),
        )
    assess_bands = section(ASSESS.read_text(encoding="utf-8").split("\n"), "## Bands", "## Output")
    revert = [line for line in assess_bands if line.startswith("- a revert undoes the change")]
    T.check(
        "stage rules: assess likely-safe revert condition excepts a listed not-revertible",
        len(revert) == 1 and "a listed `not-revertible` trigger" in revert[0],
        repr(revert),
    )
    bands_section = section(STAGE.read_text(encoding="utf-8").split("\n"), "## Bands", "## Output")
    text = "\n".join(bands_section)
    exception = "other than a removed or loosened gate that a plan approved by a human declared"
    for label, prefix in (
        ("the Care ground", "- **Care:**"),
        ("the likely-safe condition", "- no undeclared dependency"),
    ):
        lines = [line for line in bands_section if line.startswith(prefix)]
        T.check(
            f"stage rules: landing {label} excepts a declared gate",
            len(lines) == 1 and exception in lines[0],
            repr(lines),
        )
        T.check(
            f"stage rules: landing {label} excepts a listed weakens-safeguard",
            len(lines) == 1 and "a listed `weakens-safeguard` trigger" in lines[0],
            repr(lines),
        )
    for phrase in (
        "every other weakening the trigger names stays the trigger whether the plan declared it or not",
        "a step that names the file but not the weakening does not declare it",
        "a gate that failed and was then changed stays this trigger even when the change was declared",
    ):
        T.check(f"stage rules: landing says {phrase!r}", phrase in text, "")


DECOMPOSITION_TRIGGERS = ("criterion-uncovered", "contradicts-design", "outside-scope", "work-dropped")
DECOMPOSITION_INPUTS = [
    ".mintswarm/job/inputs/issue.md",
    ".mintswarm/job/inputs/artefacts/analysis.md",
    ".mintswarm/job/inputs/parent/design.md",
    ".mintswarm/job/inputs/artefacts/decomposition.md",
]
BAND_LINE = re.compile(r"\d\. `([a-z-]+)` \(shown as ([^)]+)\), confidence (\d+(?: to \d+)?),")


def band_lines(bands_section: list) -> list:
    """The (identifier, display name, range) of each numbered band line, in order."""
    return [m.groups() for line in bands_section if (m := BAND_LINE.match(line))]


def check_decomposition_stage() -> None:
    lines = DECOMPOSITION.read_text(encoding="utf-8").split("\n")
    bands_section = section(lines, "## Bands", "## Output")
    ours = band_lines(bands_section)
    theirs = band_lines(section(ASSESS.read_text(encoding="utf-8").split("\n"), "## Bands", "## Output"))
    T.check(
        "decomposition: band lines are assess.md's, in order",
        len(ours) == 5 and ours == theirs,
        f"stage {ours}, assess.md {theirs}",
    )
    text = "\n".join(bands_section)
    for phrase in (
        "starts at the band's highest value and only goes down",
        "State no reasons for raising the value",
    ):
        T.check(f"decomposition: says {phrase!r}", phrase in text, "")
    starts = [i for i, line in enumerate(bands_section) if line.startswith("1. `blocked`")]
    ends = [i for i, line in enumerate(bands_section) if line.startswith("2. ")]
    triggers = ()
    if len(starts) == 1 and len(ends) == 1:
        triggers = tuple(
            m.group(1) for line in bands_section[starts[0] : ends[0]] if (m := re.match(r"   - `([a-z-]+)`:", line))
        )
    T.check(
        "decomposition: hard triggers are exactly the four",
        triggers == DECOMPOSITION_TRIGGERS,
        repr(triggers),
    )
    T.check(
        "decomposition: one json fence in the stage",
        [line for line in lines if line == "```json"] == ["```json"],
        "",
    )
    output = section(lines, "## Output", "## Status")
    keys = []
    opens = [i for i, line in enumerate(output) if line == "```json"]
    if len(opens) == 1:
        close = next((i for i in range(opens[0] + 1, len(output)) if output[i] == "```"), len(output))
        keys = [m.group(1) for line in output[opens[0] + 1 : close] if (m := re.match(r'  "([a-z_]+)":', line))]
    T.check(
        "decomposition: the envelope template has the envelope keys, in order",
        keys == J.ENVELOPE_KEYS,
        repr(keys),
    )
    inputs = section(lines, "## Inputs", "## Reading")
    paths = [m.group(1) for line in inputs if (m := re.match(r"- `([^`]+)`:", line))]
    T.check("decomposition: exactly the four inputs", paths == DECOMPOSITION_INPUTS, repr(paths))
    T.check(
        "decomposition: never the decomposer's reasoning",
        "decomposer's reasoning" in "\n".join(inputs),
        "",
    )
    for heading in ("## Coverage", "## Fidelity", "## Scope", "## Atomicity", "## Dependencies and order"):
        T.check(f"decomposition: the template has {heading}", heading in output, "")
    rule = [p for p in text.split("\n\n") if "require_human" in p]
    T.check(
        "decomposition: require_human and floor hits do not lower the confidence",
        len(rule) == 1 and "floor" in rule[0] and "do not lower" in rule[0],
        repr(rule),
    )
    T.check(
        "decomposition: no line starts with Risk:",
        not any(line.startswith("Risk:") for line in lines),
        "",
    )


def check_stdlib() -> None:
    allowed = set(sys.stdlib_module_names)
    for path in (JUDGEMENT, Path(__file__)):
        tree = ast.parse(path.read_text(encoding="utf-8"))
        names = set()
        for node in ast.walk(tree):
            if isinstance(node, ast.Import):
                names.update(alias.name.split(".")[0] for alias in node.names)
            elif isinstance(node, ast.ImportFrom) and node.module:
                names.add(node.module.split(".")[0])
        outside = sorted(names - allowed)
        T.check(f"standard library: {path.name}", not outside, f"not standard library: {outside}")


def main() -> int:
    check_bands()
    check_triggers()
    check_threshold()
    check_malformed()
    check_consumable()
    check_stage()
    check_stage_rules()
    check_decomposition_stage()
    check_stdlib()
    print(f"{T.passed} passed, {T.failed} failed")
    return 0 if T.failed == 0 else 1


if __name__ == "__main__":
    sys.exit(main())
