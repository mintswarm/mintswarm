"""Read the landing stage's auto-approval confidence, and apply the landing threshold.

Dogfooding stand-in for the judgement layer of the landing decision. It is a
library, not a script: the landing decision loads it by its file path, as
landing/collect-facts.py loads plan-approval/decision.py, and calls the
functions below. It writes no files and runs nothing. Standard library only;
it loads ../plan-approval/decision.py, found next to this file through its
own path, with that module's floor.py set, and sets sys.dont_write_bytecode
first, so no __pycache__ is left behind.

The landing document (.mintswarm/stages/landing.md, "## Output") carries the
same envelope as the assess stage's assessment, parsed by decision.py's own
envelope_problems with the landing hard triggers and with the problems
naming the `landing document`:

- `confidence`, an integer from 0 to 100 equal to the summary line's;
- `band`, one of BANDS: `blocked` (0), `uncertain` (1-49), `needs-review`
  (50-79), `likely-safe` (80-94, at least one doubt) or `safe` (95-100, no
  doubt);
- `hard_triggers`, unique HARD_TRIGGERS identifiers, non-empty exactly when
  the band is `blocked`;
- `reason`, the decisive reason;
- `doubts`, at most 10 {`statement`, `consequence`, `resolution`}.

The contract the landing decision consumes:

- HARD_TRIGGERS: the landing trigger identifiers, stable, as recorded.
- judgement(data): the judgement layer's fields for the document's bytes,
  with the keys and order of decision.py's judgement layer that come from
  the document: `status` (`usable` or `unusable`), `problems` (fixed
  strings that never quote the document), `confidence`, `band`,
  `hard_triggers`, `doubts` and `reasons` (the envelope's reason as its only
  entry). For an unusable document every field but `status` and `problems`
  is null, or [] for a list. A missing, repeated or malformed envelope, a
  line starting with `Risk:` and bytes that are not UTF-8 are all unusable.
- judged_decision(floor_verdict, layer, min_confidence): `auto` only when
  the landing floor's verdict is `clean`, the layer is usable with no hard
  trigger and its confidence is at least min_confidence
  (pipeline.landing.min_confidence, validated by landing/floor.py's
  validate_landing); otherwise `human`.

The landing decision, its record and its shadow comment are not here.
"""

import importlib.util
import sys
from pathlib import Path
from types import ModuleType

DECISION = Path(__file__).resolve().parent.parent / "plan-approval" / "decision.py"
NOUN = "landing document"
HARD_TRIGGERS = (
    "criterion-not-met",
    "outside-footprint",
    "footprint-unusable",
    "weakens-safeguard",
    "credentials",
    "not-revertible",
    "process-not-passed",
    "inconsistent-inputs",
)
LAYER_KEYS = ["status", "problems", "confidence", "band", "hard_triggers", "doubts", "reasons"]


def load_decision() -> ModuleType:
    """plan-approval/decision.py, loaded by path, with its floor set."""
    sys.dont_write_bytecode = True  # leave no __pycache__ next to decision.py
    spec = importlib.util.spec_from_file_location("msw_approval_decision_for_landing", DECISION)
    if spec is None or spec.loader is None:
        raise ImportError(f"cannot load {DECISION}")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    module.floor = module.load_floor()
    return module


decision = load_decision()
BANDS = decision.BANDS
ENVELOPE_KEYS = decision.ENVELOPE_KEYS
DOUBT_KEYS = decision.DOUBT_KEYS


def envelope_problems(text: str) -> tuple:
    """(problems, envelope) for a landing document's text; envelope is None when unusable."""
    return decision.envelope_problems(text, triggers=HARD_TRIGGERS, noun=NOUN)


def judgement(data: bytes) -> dict:
    """The judgement layer's fields for a landing document's bytes."""
    layer = {
        "status": "unusable",
        "problems": [],
        "confidence": None,
        "band": None,
        "hard_triggers": [],
        "doubts": [],
        "reasons": [],
    }
    try:
        text = data.decode("utf-8")
    except UnicodeDecodeError:
        layer["problems"] = [f"the {NOUN} is not UTF-8"]
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


def judged_decision(floor_verdict, layer: dict, min_confidence: int) -> str:
    """`auto` for a clean floor and a usable, trigger-free judgement at or above min_confidence; else `human`."""
    if floor_verdict != "clean" or layer.get("status") != "usable" or layer.get("hard_triggers"):
        return "human"
    confidence = layer.get("confidence")
    if type(confidence) is not int or confidence < min_confidence:
        return "human"
    return "auto"
