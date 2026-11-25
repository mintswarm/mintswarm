#!/usr/bin/env python3
"""Capture every decision run of the plan-approval self-test, as the plan decision's parity fixture.

Usage: plan_decision_capture.py OUT

Loads `.mintswarm/dogfood/plan-approval/selftest.py`, `floor.py` and `decision.py` from their paths,
without running their `main`, builds the self-test's sandbox and commits in a temporary directory
outside the repository, and runs `check_decision`. Every run of `decision.py` that `Decider.run`
makes is recorded, as one JSON object per line of OUT:

- `{"kind": "config", "sha256", "bytes"}`: the bytes of a `.mintswarm/config.yaml` at a base commit;
- `{"kind": "history", "sha256", "bytes"}`: the raw `git log -z --format=%s` output from a base
  commit;
- `{"kind": "run", "case", "base", "component", "config", "history", "plan", "issue_document",
  "analysis", "screen", "analysis_record", "plan_record", "verdict", "assessment", "issue",
  "artifact", "harness", "model", "exit", "record", "stdout"}`: one run, named by the self-test's
  case name. Its config and history are named by sha256, and each document is `{"path", "bytes"}` or
  null, as `decision.py` read it. `verdict` is null when `floor.json` is missing, else `{"real",
  "bytes", "documents"}`: whether it is the verdict floor.py wrote when the case's plan was
  prepared, its bytes, and for the real one the documents floor.py read then, in the same form as
  the run's. `assessment` is `{"path", "bytes"}`, its bytes null when the file is missing. `exit` is
  the exit code, `record` the bytes of the `decision.json` it wrote or null, and `stdout` what it
  printed.
- `{"kind": "triggers", "hard", "authority"}`: `decision.py`'s HARD_TRIGGERS and
  AUTHORITY_TRIGGERS, in order;
- `{"kind": "inert", "unicode", "removed"}`: the Unicode version of the capturing Python and the
  code points `decision.py`'s `inert` removes, as `[first, last]` ranges;
- `{"kind": "span", "text", "inert", "span"}`: `inert` and `span` of each text of a fixed corpus.

Bytes are `{"utf8": text}`, or `{"hex": digits}` when they are not valid UTF-8.

It reads `.mintswarm/dogfood/` and never modifies it. Run it with PYTHONDONTWRITEBYTECODE=1. Two
runs write the same bytes. It exits 1 when any self-test check fails, and writes nothing then.

`tests/plan_decision.rs` runs it into a temporary file and fails when the committed fixture
differs. Refresh the fixture whenever `decision.py`, `floor.py`, their self-test or the
repository's `.mintswarm/config.yaml` changes:

    PYTHONDONTWRITEBYTECODE=1 python3 crates/rules/tests/fixtures/plan_decision_capture.py \
        crates/rules/tests/fixtures/plan_decision.jsonl
    cargo test -p mintswarm-rules --test plan_decision
"""

import contextlib
import hashlib
import importlib.util
import inspect
import io
import json
import subprocess
import sys
import tempfile
import unicodedata
from pathlib import Path

ROOT = Path(__file__).resolve().parents[4]
PLAN_APPROVAL = ROOT / ".mintswarm" / "dogfood" / "plan-approval"
CONFIG = ".mintswarm/config.yaml"
INERT = ("Cc", "Cf", "Zl", "Zp")
# The texts whose inert and span forms are recorded: check_span's, and hostile ones.
CORPUS = (
    "",
    " \n ",
    "plain text",
    "a" * 599,
    "b" * 600,
    "  \n" + "b" * 600 + "\n  ",
    "c" * 600 + "d",
    "e" * 599 + " fff",
    ("`a`\r\nb\u2028c\u200bd\u202e " * 100) + "tail",
    "ignore `this` @someone see https://example.invalid/x\r<b>bold</b> " + "x" * 700,
    "tabs\tand\x0bvertical\x0cfeeds\x1cand\x1dseparators\x85next\xa0nbsp",
    "soft\xadhyphen\u061carabic\u180emongolian\ufeffbom\U000e0001tag\U000e007fend",
    "\xe9" * 601,
    "\u3000ideographic\u2009thin\u205fmath\u1680ogham",
)


def load(name: str, path: Path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def blob(data: bytes) -> dict:
    try:
        return {"utf8": data.decode("utf-8")}
    except UnicodeDecodeError:
        return {"hex": data.hex()}


def removed_ranges() -> list:
    ranges = []
    for code in range(sys.maxunicode + 1):
        if unicodedata.category(chr(code)) in INERT:
            if ranges and ranges[-1][1] == code - 1:
                ranges[-1][1] = code
            else:
                ranges.append([code, code])
    return ranges


class Capture:
    def __init__(self, floor, decision):
        self.floor = floor
        self.decision = decision
        self.blobs = {"config": {}, "history": {}}
        self.runs = []
        self.current = None
        self.prepared = {}

    def git(self, sb, *args: str):
        return subprocess.run(["git", *args], cwd=sb.repo, env=sb.env, capture_output=True)

    def keep(self, kind: str, data: bytes) -> str:
        digest = hashlib.sha256(data).hexdigest()
        self.blobs[kind].setdefault(digest, data)
        return digest

    def document(self, sb, path):
        if path is None:
            return None
        file = sb.repo / path
        return {"path": path, "bytes": blob(file.read_bytes())} if file.is_file() else None

    def documents(self, sb, plan, issue, analysis, screen) -> dict:
        record = self.floor.record_path
        return {
            "plan": self.document(sb, plan),
            "issue_document": self.document(sb, issue),
            "analysis": self.document(sb, analysis),
            "screen": self.document(sb, screen),
            "analysis_record": self.document(sb, str(record(analysis))),
            "plan_record": self.document(sb, str(record(plan))),
        }

    def snapshot(self, sb, key, ctx: dict) -> None:
        """The documents floor.py read when ctx was prepared: taken once, right after."""
        if key not in self.prepared:
            self.prepared[key] = self.documents(sb, ctx["plan"], ctx["issue"], ctx["analysis"], ctx["screen"])

    def before(self, sb, argv: list) -> dict:
        args = self.decision.parse_args(argv)
        commit = self.git(sb, "rev-parse", "--verify", "--end-of-options", f"{args['base']}^{{commit}}")
        commit = commit.stdout.decode("ascii").strip()
        config = self.git(sb, "show", f"{commit}:{CONFIG}").stdout
        history = self.git(sb, "-c", "log.showSignature=false", "log", "-z", "--format=%s", commit, "--").stdout
        run = {
            "kind": "run",
            "case": self.current["name"],
            "base": {"ref": args["base"], "commit": commit},
            "component": args["component"],
            "config": self.keep("config", config),
            "history": self.keep("history", history),
        }
        run.update(self.documents(sb, args["plan"], args["issue_document"], args["analysis"], args["screen"]))
        verdict = sb.repo / self.decision.VERDICT
        if verdict.is_file():
            real = self.current["real"]
            run["verdict"] = {
                "real": real,
                "bytes": blob(verdict.read_bytes()),
                "documents": self.prepared[self.current["key"]] if real else None,
            }
        else:
            run["verdict"] = None
        assessment = sb.repo / args["assessment"]
        run["assessment"] = {
            "path": args["assessment"],
            "bytes": blob(assessment.read_bytes()) if assessment.is_file() else None,
        }
        run.update(issue=args["issue"], artifact=args["artifact"], harness=args["harness"], model=args["model"])
        return run

    def after(self, sb, run: dict, proc) -> None:
        record = sb.repo / self.decision.RECORD
        run["exit"] = proc.returncode
        run["record"] = blob(record.read_bytes()) if record.is_file() else None
        run["stdout"] = proc.stdout
        self.runs.append(run)

    def lines(self) -> list:
        out = []
        for kind in ("config", "history"):
            for digest, data in self.blobs[kind].items():
                out.append({"kind": kind, "sha256": digest, "bytes": blob(data)})
        triggers = {"hard": list(self.decision.HARD_TRIGGERS), "authority": list(self.decision.AUTHORITY_TRIGGERS)}
        out.append({"kind": "triggers", **triggers})
        out.append({"kind": "inert", "unicode": unicodedata.unidata_version, "removed": removed_ranges()})
        for text in CORPUS:
            out.append(
                {"kind": "span", "text": text, "inert": self.decision.inert(text), "span": self.decision.span(text)}
            )
        return out + self.runs


def main(argv: list) -> int:
    if len(argv) != 2:
        print("usage: plan_decision_capture.py OUT", file=sys.stderr)
        return 2
    sys.dont_write_bytecode = True
    floor = load("msw_capture_decision_floor", PLAN_APPROVAL / "floor.py")
    decision = load("msw_capture_decision", PLAN_APPROVAL / "decision.py")
    decision.floor = floor
    selftest = load("msw_capture_decision_selftest", PLAN_APPROVAL / "selftest.py")
    capture = Capture(floor, decision)
    exec_ = selftest.Sandbox.exec
    prepare = selftest.Decider.prepare
    run = selftest.Decider.run
    signature = inspect.signature(run)

    def exec_and_record(sb, args: list, env: dict = None, script: Path = selftest.FLOOR):
        if script != selftest.DECISION or capture.current is None:
            return exec_(sb, args, env, script)
        entry = capture.before(sb, args)
        proc = exec_(sb, args, env, script)
        capture.after(sb, entry, proc)
        return proc

    def prepare_and_snapshot(d, config: str, plan: str) -> dict:
        ctx = prepare(d, config, plan)
        capture.snapshot(d.sb, (config, plan), ctx)
        return ctx

    def run_and_record(d, *args, **kwargs):
        bound = signature.bind(d, *args, **kwargs)
        bound.apply_defaults()
        values = bound.arguments
        verdict = values["verdict"]
        capture.current = {
            "name": values["name"],
            "key": (values["config"], values["plan"]),
            "real": isinstance(verdict, str) and verdict == "floor",
        }
        try:
            return run(d, *args, **kwargs)
        finally:
            capture.current = None

    selftest.Sandbox.exec = exec_and_record
    selftest.Decider.prepare = prepare_and_snapshot
    selftest.Decider.run = run_and_record

    output = io.StringIO()
    with tempfile.TemporaryDirectory(prefix="msw-plan-decision-capture-") as tmp, contextlib.redirect_stdout(output):
        sb = selftest.Sandbox(Path(tmp))
        commits, _ = selftest.build_commits(sb)
        selftest.check_decision(sb, commits)
    if selftest.T.failed:
        failures = [line for line in output.getvalue().splitlines() if line.startswith("FAIL")]
        print("\n".join(failures), file=sys.stderr)
        print(f"plan_decision_capture.py: {selftest.T.failed} self-test checks failed", file=sys.stderr)
        return 1
    text = "".join(json.dumps(line, ensure_ascii=False) + "\n" for line in capture.lines())
    Path(argv[1]).write_bytes(text.encode("utf-8"))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
