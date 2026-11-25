#!/usr/bin/env python3
"""Capture every pure-floor run of the plan-approval self-test, as the plan floor's parity fixture.

Usage: plan_floor_capture.py OUT

Loads `.mintswarm/dogfood/plan-approval/selftest.py` and `floor.py` from their paths, without running
their `main`, builds the self-test's sandbox and commits in a temporary directory outside the
repository, and runs the check groups that exercise the pure floor: GROUPS below. Every run of
`floor.py` they make is recorded, as one JSON object per line of OUT:

- `{"kind": "config", "sha256", "bytes"}`: the bytes of a `.mintswarm/config.yaml` at a base commit;
- `{"kind": "history", "sha256", "bytes"}`: the raw `git log -z --format=%s` output from a base
  commit;
- `{"kind": "run", "group", "case", "base", "component", "config", "history", "plan",
  "issue_document", "analysis", "screen", "analysis_record", "plan_record", "exit", "floor"}`: one
  run, its config and history named by sha256, each document `{"path", "bytes"}` or null, its exit
  code, and the exact text of the `floor.json` it wrote, or null when it wrote none.

Bytes are `{"utf8": text}`, or `{"hex": digits}` when they are not valid UTF-8. Each config and
history line is written once, before the runs. A run whose arguments `floor.py` refuses, whose
base does not resolve to a commit or whose base has no config is not recorded: those test the
extraction of the bundle, not the floor.

It reads `.mintswarm/dogfood/` and never modifies it. Run it with PYTHONDONTWRITEBYTECODE=1. Two
runs write the same bytes. It exits 1 when any self-test check fails, and writes nothing then.

`tests/plan_floor.rs` runs it into a temporary file and fails when the committed fixture differs.
Refresh the fixture whenever `floor.py`, its self-test or the repository's `.mintswarm/config.yaml`
changes:

    PYTHONDONTWRITEBYTECODE=1 python3 crates/rules/tests/fixtures/plan_floor_capture.py \
        crates/rules/tests/fixtures/plan_floor.jsonl
    cargo test -p mintswarm-rules --test plan_floor
"""

import contextlib
import hashlib
import importlib.util
import io
import json
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[4]
PLAN_APPROVAL = ROOT / ".mintswarm" / "dogfood" / "plan-approval"
CONFIG = ".mintswarm/config.yaml"
DOCUMENTS = (("issue_document", "issue_document"), ("analysis", "analysis"), ("screen", "screen"))
# The pure-floor check groups, in the self-test's order; the last two run inside check_blocking_questions.
GROUPS = (
    "check_clean_base",
    "check_single_rules",
    "check_production_lines",
    "check_dependency_screen",
    "check_component_limits",
    "check_unusable_footprints",
    "check_historic",
    "check_blocking_questions",
    "check_question_markers",
    "check_question_records",
    "check_acceptance_criteria",
    "check_first_of_kind",
    "check_size_floor",
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


class Capture:
    def __init__(self, floor):
        self.floor = floor
        self.groups = []
        self.cases = {}
        self.blobs = {"config": {}, "history": {}}
        self.runs = []

    def tagged(self, name: str, function):
        def run(*args, **kwargs):
            self.groups.append(name)
            try:
                return function(*args, **kwargs)
            finally:
                self.groups.pop()

        return run

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
        return {"path": path, "bytes": blob(file.read_bytes())} if file.exists() else None

    def record(self, sb, argv: list, proc) -> None:
        if not self.groups:
            return
        try:
            args = self.floor.parse_args(argv)
        except self.floor.InputError:
            return
        resolved = self.git(sb, "rev-parse", "--verify", "--quiet", "--end-of-options", f"{args['base']}^{{commit}}")
        if resolved.returncode != 0:
            return
        commit = resolved.stdout.decode("ascii").strip()
        config = self.git(sb, "show", f"{commit}:{CONFIG}")
        if config.returncode != 0:
            return
        history = self.git(sb, "-c", "log.showSignature=false", "log", "-z", "--format=%s", commit, "--")
        if history.returncode != 0:
            raise RuntimeError(f"git log failed at {commit}")
        group = self.groups[-1]
        self.cases[group] = self.cases.get(group, 0) + 1
        verdict = sb.repo / self.floor.VERDICT
        run = {
            "kind": "run",
            "group": group,
            "case": self.cases[group],
            "base": {"ref": args["base"], "commit": commit},
            "component": args["component"],
            "config": self.keep("config", config.stdout),
            "history": self.keep("history", history.stdout),
            "plan": self.document(sb, args["plan"]),
        }
        for key, arg in DOCUMENTS:
            run[key] = self.document(sb, args[arg])
        analysis = args["analysis"]
        run["analysis_record"] = None if analysis is None else self.document(sb, str(self.floor.record_path(analysis)))
        run["plan_record"] = self.document(sb, str(self.floor.record_path(args["plan"])))
        run["exit"] = proc.returncode
        run["floor"] = verdict.read_bytes().decode("utf-8") if verdict.exists() else None
        self.runs.append(run)

    def lines(self) -> list:
        out = []
        for kind in ("config", "history"):
            for digest, data in self.blobs[kind].items():
                out.append({"kind": kind, "sha256": digest, "bytes": blob(data)})
        return out + self.runs


def main(argv: list) -> int:
    if len(argv) != 2:
        print("usage: plan_floor_capture.py OUT", file=sys.stderr)
        return 2
    floor = load("msw_capture_plan_floor", PLAN_APPROVAL / "floor.py")
    selftest = load("msw_capture_plan_selftest", PLAN_APPROVAL / "selftest.py")
    capture = Capture(floor)
    exec_ = selftest.Sandbox.exec

    def exec_and_record(sb, args: list, env: dict = None, script: Path = selftest.FLOOR):
        proc = exec_(sb, args, env, script)
        if script == selftest.FLOOR:
            capture.record(sb, args, proc)
        return proc

    selftest.Sandbox.exec = exec_and_record
    for name in GROUPS:
        setattr(selftest, name, capture.tagged(name, getattr(selftest, name)))

    output = io.StringIO()
    with tempfile.TemporaryDirectory(prefix="msw-plan-floor-capture-") as tmp, contextlib.redirect_stdout(output):
        sb = selftest.Sandbox(Path(tmp))
        commits, limits = selftest.build_commits(sb)
        runner = selftest.Runner(sb, commits)
        selftest.check_clean_base(runner)
        selftest.check_single_rules(runner, limits)
        selftest.check_production_lines(runner, limits)
        selftest.check_dependency_screen(runner, sb, commits)
        selftest.check_component_limits(runner, limits)
        selftest.check_unusable_footprints(runner)
        selftest.check_historic(runner)
        selftest.check_blocking_questions(runner)
        selftest.check_acceptance_criteria(runner)
        selftest.check_first_of_kind(runner)
        selftest.check_size_floor(runner)
    if selftest.T.failed:
        failures = [line for line in output.getvalue().splitlines() if line.startswith("FAIL")]
        print("\n".join(failures), file=sys.stderr)
        print(f"plan_floor_capture.py: {selftest.T.failed} self-test checks failed", file=sys.stderr)
        return 1
    text = "".join(json.dumps(line, ensure_ascii=False) + "\n" for line in capture.lines())
    Path(argv[1]).write_bytes(text.encode("utf-8"))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
