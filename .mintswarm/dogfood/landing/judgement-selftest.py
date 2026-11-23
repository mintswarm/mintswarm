#!/usr/bin/env python3
"""Self-test for landing/judgement.py, the landing stage's judgement layer.

    python3 .mintswarm/dogfood/landing/judgement-selftest.py

Table-driven, standard library only and offline; it writes no files.
Landing documents are built in memory and read through judgement.py, which
is loaded by its file path. The cases:

- bands: each band at its lowest and highest value is usable, a value just
  outside each band is not, `likely-safe` needs a doubt and `safe` has none;
- hard triggers: each landing trigger alone at `blocked` 0 is usable;
  `blocked` without a trigger, a trigger outside `blocked`, a repeated
  trigger and the plan-only `outside-scope` and `undeclared-high`, and
  `require-human`, are not;
- the threshold, through judged_decision: the confidence against
  min_confidence at the edges, and a hard trigger, an unusable layer, a
  `hit` floor and a missing floor;
- malformed and repeated envelopes, each unusable with a fixed problem that
  names the landing document;
- consumable as is: judgement() gives the envelope's own values, with the
  keys of plan-approval/decision.py's judgement layer, in its order;
- stage agreement: the hard triggers and band ranges under "## Bands" in
  .mintswarm/stages/landing.md are judgement.py's, its envelope template has
  the envelope keys in order, and no line of it starts with `Risk:`.

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


def check_triggers() -> None:
    for trigger in J.HARD_TRIGGERS:
        layer = expect_usable(f"trigger {trigger} alone", document(envelope("blocked", 0, [trigger])))
        T.check(f"trigger {trigger}: recorded", layer["hard_triggers"] == [trigger], json.dumps(layer))
    expect_usable("every landing trigger together", document(envelope("blocked", 0, list(J.HARD_TRIGGERS))))
    exactly = f"the {NOUN}'s hard_triggers must be non-empty exactly when the band is blocked"
    expect_unusable("blocked without a trigger", document(envelope("blocked", 0, [])), exactly)
    expect_unusable(
        "a trigger in needs-review", document(envelope("needs-review", 60, ["process-not-passed"])), exactly
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
    ]
    for name, floor_verdict, judged, minimum, want in cases:
        got = J.judged_decision(floor_verdict, judged, minimum)
        T.check(f"threshold {name}: {want}", got == want, f"got {got}")
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
    start = next(i for i, line in enumerate(bands_section) if line.startswith("1. `blocked`"))
    end = next(i for i, line in enumerate(bands_section) if line.startswith("2. "))
    triggers = tuple(
        re.match(r"   - `([a-z-]+)`:", line).group(1) for line in bands_section[start:end] if line.startswith("   - `")
    )
    T.check(
        "stage: hard triggers are judgement.py's, in order",
        triggers == J.HARD_TRIGGERS,
        f"stage {triggers}, HARD_TRIGGERS {J.HARD_TRIGGERS}",
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
    check_stdlib()
    print(f"{T.passed} passed, {T.failed} failed")
    return 0 if T.failed == 0 else 1


if __name__ == "__main__":
    sys.exit(main())
