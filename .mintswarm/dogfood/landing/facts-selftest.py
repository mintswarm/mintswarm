#!/usr/bin/env python3
"""Offline self-test of keep-round.py and collect-facts.py.

    facts-selftest.py

Needs python3, `git` and `yq` on PATH, and bash for bot-gh.sh; no network.
It works in a temporary sandbox with its own HOME, no system or global git
config and a fixed identity: a throwaway repository with a base and a head
commit and the job layout under .mintswarm/job/, and a runner-state
directory beside it, outside the repository, where the rounds are kept.
Both scripts run as `sys.executable SCRIPT ...` with that repository as
their working directory; jobs made elsewhere are repositories of their own.

Checks:

- keeping rounds: three gates rounds and two review rounds are kept (review
  2 with --no-address) in the runner-state directory, and every copy, the
  gates input as gates.input.json included, is byte-identical under its
  round number, with mode 0444 and a version 2 round.json holding the head;
  keeping gates 2 again exits 1 and leaves bytes, modes and mtimes
  unchanged; a wrong review title, a stale address title without
  --no-address, a gates.md older than inputs/gates.json and a missing
  review.md each exit 2 and leave no final directory and no temporary one;
  a gates round with neither output keeps only its input, with a warning;
  bad arguments and a missing inputs/gates.json exit 2;
- where rounds are kept: keep-round.py without --rounds, with a relative
  one, one inside the worktree, one inside the main checkout when run from
  a nested worktree (`git worktree add`) and a symbolic link each exit 2,
  naming the cause, and create nothing there, while the same round kept in
  runner state succeeds;
- the head: a gates --head that is not HEAD, a gates keep with a modified
  tracked file or an untracked file outside .mintswarm/job/, a review
  --head that is not an ancestor of HEAD and a --head that is not a full
  hash each exit 2, naming the cause, and keep nothing;
- commit binding: rounds kept at the base (gates with the base checked
  out, review as an ancestor) and at the head give `commit` and `current`,
  false for the base's rounds, with their raw verdicts;
- run-gates.py agreement: in a job of its own, a report short of the
  input's gates, tests named out of the input's order, an input with no
  gates and a report that meets every condition are each checked with the
  real run-gates.py --verdict and then kept; assemble gives every gate
  round, there and in the main job, the verdict and reason run-gates.py
  gave;
- fetch: a stub `gh` that records its argv as JSON lines and serves
  hand-written pages goes first on PATH, and the test aborts unless `gh`
  resolves to it; MSW_TOKEN is the dummy ghs_selftest and the real
  .mintswarm/dogfood/github/bot-gh.sh runs. Every recorded argv is
  `api --method GET PATH` with no field, input or paginate flag, the issue
  comments come in two pages (100 and 5 items), and the pages are saved
  byte for byte; a failing call, a page that is not JSON and more than 30
  pages exit 1 and leave no directory; a second fetch replaces the first;
- assemble, on the saved responses: fetched_at from request.json; checks
  from check runs, the latest per app and name, and statuses, and an empty
  list when there are none; gate and review rounds in round order with
  verdicts, failed tests and severity counts; the steer cases (a human
  after the approval, a human before and at the approval time, comments
  edited after the approval and before it, the approval by URL and by id,
  a review comment sharing the approval's id, bots without the marker,
  mintswarm-dev[bot] with the marker line, indented and inside a code
  fence, the marker mid-line, another bot with the marker, a pending
  review, an inline review comment, a deleted user); every round with
  `commit` the head and `current` true; the approval with and without
  decision, kickoff floor and assessment, and by mintswarm-dev[bot]; exit 2
  with no facts file for a head mismatch, a bad fetched_at, a tampered kept
  file, a version 1 round.json, a gates round without its input, a gap in
  round numbers, an unreadable review verdict, a bad --approved-at or
  --approver, and a decision or kickoff floor of the wrong tool;
- reading rounds from runner state: assemble without --rounds, with a
  relative one, a symbolic link, one inside the worktree and one inside the
  main checkout from a nested worktree exits 2 with no facts file, naming
  the cause; a full copy of the rounds under .mintswarm/job/rounds/,
  rewritten to pass with recomputed sha256 values, leaves the facts
  unchanged, and given as --rounds exits 2 (the same copy outside the
  repository does read as passed);
- static: every import of the three scripts is in the standard library,
  keep-round.py's docstring names report-gates.py --clean and the next
  review round, and both scripts' docstrings say that keeping rounds out of
  the worktree is not an OS barrier.

Prints one `ok` or `FAIL` line per check, then `N passed, M failed`, and
exits 0 only when none failed.
"""

import ast
import hashlib
import json
import os
import shutil
import stat
import subprocess
import sys
import tempfile
from pathlib import Path
from typing import Optional

HERE = Path(__file__).resolve().parent
KEEP = HERE / "keep-round.py"
COLLECT = HERE / "collect-facts.py"
BOT_GH = HERE.parent / "github" / "bot-gh.sh"
RUN_GATES = HERE.parent / "run-gates.py"
SCRIPTS = (KEEP, COLLECT, Path(__file__).resolve())
JOB = Path(".mintswarm/job")
FACTS = JOB / "inputs" / "landing" / "facts.json"
RESPONSES = JOB / "landing" / "responses"
DUMMY_TOKEN = "ghs_selftest"
PR, ISSUE = 52, 47
APPROVED_AT = "2026-11-22T12:00:00Z"
APPROVAL_ID = 9001
APPROVAL_URL = f"https://github.com/mintswarm/mintswarm/pull/{PR}#issuecomment-{APPROVAL_ID}"
MARKER = "<!-- mintswarm: steer -->"
FORBIDDEN = {"-f", "-F", "--field", "--raw-field", "--input", "--paginate", "-X"}
FACTS_KEYS = ["tool", "version", "pr", "issue", "head", "base", "fetched_at", "checks",
              "gate_rounds", "review_rounds", "steers", "approval"]
ROUND_KEYS = ["tool", "version", "kind", "round", "head", "kept_at", "files"]
GATE_KEYS = ["round", "commit", "current", "verdict", "failed_tests", "reason", "files"]
REVIEW_KEYS = ["round", "commit", "current", "verdict", "findings", "files"]
DEFAULT = object()  # keep(): the sandbox's head or rounds
T0 = 1_800_000_000


class SelfTest:
    def __init__(self):
        self.passed = 0
        self.failed = 0

    def check(self, name: str, condition: bool, detail: str = "") -> bool:
        if condition:
            self.passed += 1
            print(f"ok   {name}")
        else:
            self.failed += 1
            print(f"FAIL {name}" + (f": {detail}" if detail else ""))
        return condition


T = SelfTest()


# --- sandbox --------------------------------------------------------------

class Sandbox:
    def __init__(self, tmp: Path):
        self.tmp = tmp
        self.repo = tmp / "repo"
        self.stubs = tmp / "stubs"
        self.state = tmp / "stub-state"
        home = tmp / "home"
        for path in (self.repo, self.stubs, self.state, home):
            path.mkdir()
        drop = ("GIT_", "GH_", "GITHUB_", "MSW_")
        env = {k: v for k, v in os.environ.items() if not k.startswith(drop)}
        env.pop("GITHUB_TOKEN", None)
        env.update({
            "HOME": str(home),
            "TMPDIR": str(tmp),
            "GIT_CONFIG_NOSYSTEM": "1",
            "GIT_CONFIG_GLOBAL": os.devnull,
            "GIT_TERMINAL_PROMPT": "0",
            "GIT_AUTHOR_NAME": "facts selftest",
            "GIT_AUTHOR_EMAIL": "selftest@example.invalid",
            "GIT_COMMITTER_NAME": "facts selftest",
            "GIT_COMMITTER_EMAIL": "selftest@example.invalid",
            "GIT_AUTHOR_DATE": "2026-01-01T00:00:00Z",
            "GIT_COMMITTER_DATE": "2026-01-01T00:00:00Z",
        })
        self.env = env
        self.git("init", "-q", "-b", "master")
        write(self.repo / "README.md", "a tracked file\n")
        self.git("add", "README.md")
        self.git("commit", "-q", "-m", "base")
        self.base = self.git("rev-parse", "HEAD")
        self.git("commit", "-q", "--allow-empty", "-m", "head")
        self.head = self.git("rev-parse", "HEAD")
        self.outputs = self.repo / JOB / "outputs"
        self.inputs = self.repo / JOB / "inputs"
        self.runner_state = tmp / "runner-state"  # outside every repository, as the session's state is
        self.rounds = self.runner_state / "jobs" / "repo" / "rounds"
        self.outputs.mkdir(parents=True)
        self.inputs.mkdir(parents=True)
        self.gate_verdicts = []
        self.short_verdicts = []

    def git(self, *args: str, cwd: Optional[Path] = None) -> str:
        proc = subprocess.run(["git", *args], cwd=cwd or self.repo, env=self.env, capture_output=True, text=True)
        if proc.returncode != 0:
            raise RuntimeError(f"git {' '.join(args)} failed: {proc.stderr.strip()}")
        return proc.stdout.strip()

    def job(self, path: Path) -> str:
        """Make `path` a job repository of its own with one commit; return its head."""
        (path / JOB / "outputs").mkdir(parents=True)
        self.git("init", "-q", "-b", "master", cwd=path)
        self.git("commit", "-q", "--allow-empty", "-m", "job", cwd=path)
        return self.git("rev-parse", "HEAD", cwd=path)

    def run(self, script: Path, args: list, env: Optional[dict] = None, cwd: Optional[Path] = None):
        return subprocess.run(
            [sys.executable, str(script), *args],
            cwd=cwd or self.repo, env=env or self.env, capture_output=True, text=True,
        )


def write(path: Path, data, mtime: Optional[int] = None) -> bytes:
    path.parent.mkdir(parents=True, exist_ok=True)
    raw = data.encode("utf-8") if isinstance(data, str) else data
    path.write_bytes(raw)
    if mtime is not None:
        os.utime(path, (mtime, mtime))
    return raw


def sha(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def snapshot(directory: Path) -> dict:
    result = {"dir": (stat.S_IMODE(directory.stat().st_mode), directory.stat().st_mtime_ns)}
    for path in sorted(directory.iterdir()):
        st = path.stat()
        result[path.name] = (path.read_bytes(), stat.S_IMODE(st.st_mode), st.st_mtime_ns)
    return result


def leftovers(directory: Path) -> list:
    return sorted(p.name for p in directory.iterdir() if p.name.startswith(".tmp-")) if directory.is_dir() else []


def detail(proc) -> str:
    return f"exit {proc.returncode}: {(proc.stderr or proc.stdout).strip()[:300]}"


# --- rounds ---------------------------------------------------------------

def ctrf(statuses: dict) -> str:
    tests = []
    for name, status in statuses.items():
        test = {"name": name, "status": status, "duration": 5, "start": 1, "stop": 6}
        if status == "failed":
            test["message"] = "exit code 1"
            test["trace"] = "failure"
        tests.append(test)
    passed = sum(1 for s in statuses.values() if s == "passed")
    report = {
        "reportFormat": "CTRF", "specVersion": "0.3.0", "reportId": "00000000-0000-0000-0000-000000000000",
        "timestamp": "2026-11-22T10:00:00+00:00", "generatedBy": "mintswarm run-gates.py",
        "results": {
            "tool": {"name": "mintswarm-gates"},
            "summary": {"tests": len(tests), "passed": passed, "failed": len(tests) - passed,
                        "skipped": 0, "pending": 0, "other": 0, "start": 1, "stop": 6, "duration": 5},
            "tests": tests,
        },
    }
    return json.dumps(report, indent=2) + "\n"


GATE_ROUNDS = [
    {"whitespace": "failed", "agents-drift": "passed"},
    {"whitespace": "passed", "agents-drift": "failed"},
    {"whitespace": "passed", "agents-drift": "passed"},
]
EXPECTED_GATES = [
    ("failed", ["whitespace"], "summary.passed is 1 of 2"),
    ("failed", ["agents-drift"], "summary.passed is 1 of 2"),
    ("passed", [], None),
]

REVIEW_1 = """# Review round 1: Keep rounds

Changes required: 1 major, 2 minor, 1 nit.

## Acceptance criteria
| Criterion | Verdict | Evidence |
| --- | --- | --- |
| AC 1 | met | the self-test |

## Findings

```yaml
findings:
  - id: F1
    severity: major           # blocker | major | minor | nit
    location: a.py:1
    claim: "a claim: with a colon"
    evidence: "…"
    suggested_fix: "…"
  - id: F2
    severity: minor
    location: a.py:2
    claim: "x"
    evidence: "y"
    suggested_fix: "z"
  - id: F3
    severity: "minor"
    location: a.py:3
    claim: "x"
    evidence: "y"
    suggested_fix: "z"
  - id: F4
    severity: nit
    location: a.py:4
    claim: "x"
    evidence: "y"
    suggested_fix: "z"
```

A quoted template is not the verdict:

```markdown
## Verdict
pass
```

## Verdict

changes_required
"""
ADDRESS_1 = """# Address round 1: Keep rounds

2 fixed, 2 disputed.

| Finding | Response | Detail |
| --- | --- | --- |
| F1 | fixed | commit `abc1234` |
"""
REVIEW_2 = """# Review round 2: Keep rounds

Pass, no findings.

## Findings

```yaml
findings: []
```

## Verdict
pass
"""


def keep(sb: Sandbox, *args: str, cwd: Optional[Path] = None, head=DEFAULT, rounds=DEFAULT):
    """keep-round.py with --head (the sandbox's head) and --rounds (its runner state); None leaves one out."""
    argv = list(args)
    head = sb.head if head is DEFAULT else head
    rounds = sb.rounds if rounds is DEFAULT else rounds
    if head is not None:
        argv += ["--head", head]
    if rounds is not None:
        argv += ["--rounds", str(rounds)]
    return sb.run(KEEP, argv, cwd=cwd)


def check_round(name: str, sb: Sandbox, kind: str, number: int, expected: dict,
                rounds: Optional[Path] = None, head: Optional[str] = None) -> None:
    path = (rounds or sb.rounds) / f"{kind}-{number}"
    problems = []
    if not path.is_dir():
        T.check(name, False, f"{path} is missing")
        return
    names = sorted(p.name for p in path.iterdir())
    if names != sorted([*expected, "round.json"]):
        problems.append(f"holds {names}")
    for file, data in expected.items():
        if (path / file).is_file() and (path / file).read_bytes() != data:
            problems.append(f"{file} differs")
    for p in path.iterdir():
        if stat.S_IMODE(p.stat().st_mode) != 0o444:
            problems.append(f"{p.name} mode {oct(stat.S_IMODE(p.stat().st_mode))}")
    try:
        record = json.loads((path / "round.json").read_text(encoding="utf-8"))
        if list(record) != ROUND_KEYS:
            problems.append(f"round.json keys {list(record)}")
        want = {"tool": "mintswarm-keep-round", "version": 2, "kind": kind, "round": number,
                "head": head or sb.head, "files": [{"name": f, "sha256": sha(d)} for f, d in sorted(expected.items())]}
        for key, value in want.items():
            if record.get(key) != value:
                problems.append(f"round.json {key} is {record.get(key)!r}")
    except (OSError, ValueError) as exc:
        problems.append(f"round.json unreadable: {exc}")
    T.check(name, not problems, "; ".join(problems))


def gates_input_text(names: list) -> str:
    return json.dumps({"gates": [{"name": n, "command": "true"} for n in names]}) + "\n"


def run_gates_verdict(sb: Sandbox, cwd: Path) -> tuple:
    """run-gates.py --verdict on the round in place: (exit code, stdout)."""
    proc = sb.run(RUN_GATES, ["--verdict"], cwd=cwd)
    return proc.returncode, proc.stdout.strip()


def keep_tests(sb: Sandbox) -> None:
    gates_json = sb.inputs / "gates.json"
    gates_input = gates_input_text(list(GATE_ROUNDS[0]))
    kept = {}
    for number, statuses in enumerate(GATE_ROUNDS, 1):
        for old in sb.outputs.glob("gates.*"):
            old.unlink()
        start = T0 + 100 * number
        files = {
            "gates.input.json": write(gates_json, gates_input, start),
            "gates.ctrf.json": write(sb.outputs / "gates.ctrf.json", ctrf(statuses), start + 10),
            "gates.md": write(sb.outputs / "gates.md", f"# Gates: Keep rounds\n\nRound {number}.\n", start + 20),
        }
        sb.gate_verdicts.append(run_gates_verdict(sb, sb.repo))
        proc = keep(sb, "gates", str(number))
        T.check(f"keep: gates {number} is kept", proc.returncode == 0, detail(proc))
        kept[("gates", number)] = files

    files = {"review.md": write(sb.outputs / "review.md", REVIEW_1),
             "address.md": write(sb.outputs / "address.md", ADDRESS_1)}
    proc = keep(sb, "review", "1")
    T.check("keep: review 1 is kept with its address", proc.returncode == 0, detail(proc))
    kept[("review", 1)] = files

    files = {"review.md": write(sb.outputs / "review.md", REVIEW_2)}
    proc = keep(sb, "review", "2")
    T.check("keep: review 2 with the stale address 1 and no --no-address exits 2",
            proc.returncode == 2 and not (sb.rounds / "review-2").exists() and not leftovers(sb.rounds),
            detail(proc))
    proc = keep(sb, "review", "2", "--no-address")
    T.check("keep: review 2 is kept with --no-address", proc.returncode == 0, detail(proc))
    kept[("review", 2)] = files

    for (kind, number), files in kept.items():
        check_round(f"keep: {kind}-{number} holds byte-identical copies", sb, kind, number, files)

    before = snapshot(sb.rounds / "gates-2")
    proc = keep(sb, "gates", "2")
    T.check("keep: keeping gates 2 again exits 1", proc.returncode == 1, detail(proc))
    T.check("keep: gates-2 is unchanged in bytes, modes and mtimes", snapshot(sb.rounds / "gates-2") == before)
    proc = keep(sb, "review", "1")
    T.check("keep: keeping review 1 again exits 1", proc.returncode == 1, detail(proc))

    write(sb.outputs / "review.md", REVIEW_2)
    proc = keep(sb, "review", "3", "--no-address")
    T.check("keep: a review.md titled round 2 kept as round 3 exits 2",
            proc.returncode == 2 and not (sb.rounds / "review-3").exists(), detail(proc))
    (sb.outputs / "review.md").unlink()
    proc = keep(sb, "review", "3", "--no-address")
    T.check("keep: a missing review.md exits 2",
            proc.returncode == 2 and not (sb.rounds / "review-3").exists(), detail(proc))

    write(gates_json, gates_input, T0 + 1000)
    write(sb.outputs / "gates.ctrf.json", ctrf(GATE_ROUNDS[2]), T0 + 1010)
    write(sb.outputs / "gates.md", "# Gates: Keep rounds\n\nOld.\n", T0 + 900)
    proc = keep(sb, "gates", "4")
    T.check("keep: a gates.md older than inputs/gates.json exits 2",
            proc.returncode == 2 and not (sb.rounds / "gates-4").exists(), detail(proc))
    T.check("keep: no temporary directory is left", not leftovers(sb.rounds), str(leftovers(sb.rounds)))

    for args in (["gates", "0"], ["gates", "01"], ["gates"], ["review", "1", "--extra"],
                 ["gates", "5", "--no-address"], ["other", "1"], ["gates", "5", "--head", sb.head],
                 ["gates", "5", "--rounds"]):
        proc = keep(sb, *args)
        T.check(f"keep: bad arguments {args[:4]} exit 2", proc.returncode == 2, detail(proc))

    other = sb.tmp / "other"
    other_head = sb.job(other)
    other_rounds = sb.runner_state / "jobs" / "other" / "rounds"
    proc = keep(sb, "gates", "1", cwd=other, head=other_head, rounds=other_rounds)
    T.check("keep: a gates round without inputs/gates.json exits 2",
            proc.returncode == 2 and not (other_rounds / "gates-1").exists(), detail(proc))
    data = write(other / JOB / "inputs" / "gates.json", gates_input)
    proc = keep(sb, "gates", "1", cwd=other, head=other_head, rounds=other_rounds)
    empty = other_rounds / "gates-1"
    ok = proc.returncode == 0 and "warning" in proc.stderr and empty.is_dir()
    if ok:
        record = json.loads((empty / "round.json").read_text(encoding="utf-8"))
        ok = record["files"] == [{"name": "gates.input.json", "sha256": sha(data)}] \
            and sorted(p.name for p in empty.iterdir()) == ["gates.input.json", "round.json"]
    T.check("keep: a gates round with neither output keeps only its input, with a warning", ok, detail(proc))


def ready_gates(job: Path, start: int) -> None:
    """A gates round in `job` that keep-round.py would keep: an input and both outputs, newer than it."""
    write(job / JOB / "inputs" / "gates.json", gates_input_text(list(GATE_ROUNDS[2])), start)
    write(job / JOB / "outputs" / "gates.ctrf.json", ctrf(GATE_ROUNDS[2]), start + 10)
    write(job / JOB / "outputs" / "gates.md", "# Gates: Keep rounds\n\nReady.\n", start + 20)


def nested_worktree(sb: Sandbox) -> Path:
    """A second worktree of the sandbox repository at its head, outside it, made once."""
    nested = sb.tmp / "nested"
    if not nested.exists():
        sb.git("worktree", "add", "-q", "--detach", str(nested), "HEAD")
        (nested / JOB / "outputs").mkdir(parents=True)
    return nested


def refused(name: str, proc, fragment: str, absent: list) -> None:
    gone = [str(path) for path in absent if os.path.lexists(path)]
    T.check(name, proc.returncode == 2 and fragment in proc.stderr and not gone,
            detail(proc) + (f"; left {gone}" if gone else ""))


def location_tests(sb: Sandbox) -> None:
    """keep-round.py keeps rounds only in runner state, outside the worktree and the main checkout."""
    ready_gates(sb.repo, T0 + 3000)
    inside = sb.repo / JOB / "rounds"
    target = sb.runner_state / "linked"
    target.mkdir(parents=True)
    link = sb.runner_state / "rounds-link"
    link.symlink_to(target)
    cases = [
        ("without --rounds", None, "missing --rounds", [sb.rounds / "gates-9"]),
        ("with a relative --rounds", "state/rounds", "must be an absolute path", [sb.repo / "state"]),
        ("with --rounds inside the worktree", inside, "inside the worktree", [inside]),
        ("with a symlinked --rounds", link, "is a symbolic link", [target / "gates-9"]),
    ]
    for name, rounds, fragment, absent in cases:
        refused(f"keep: {name} exits 2 and keeps nothing", keep(sb, "gates", "9", rounds=rounds), fragment, absent)

    nested = nested_worktree(sb)
    ready_gates(nested, T0 + 3000)
    in_main = sb.repo / "state" / "rounds"
    refused("keep: from a nested worktree, --rounds inside the main checkout exits 2 and keeps nothing",
            keep(sb, "gates", "1", cwd=nested, rounds=in_main), "inside the main checkout", [sb.repo / "state"])
    control = sb.runner_state / "jobs" / "nested" / "rounds"
    proc = keep(sb, "gates", "1", cwd=nested, rounds=control)
    T.check("keep: from a nested worktree, the same round kept in runner state succeeds",
            proc.returncode == 0 and (control / "gates-1" / "round.json").is_file(), detail(proc))
    T.check("keep: the rounds directory is made mode 0700", stat.S_IMODE(control.stat().st_mode) == 0o700,
            oct(stat.S_IMODE(control.stat().st_mode)))


def head_tests(sb: Sandbox) -> None:
    """keep-round.py records only a head the round can have judged."""
    rounds = sb.runner_state / "jobs" / "refused" / "rounds"
    ready_gates(sb.repo, T0 + 4000)
    tree = sb.git("rev-parse", "HEAD^{tree}")
    unrelated = sb.git("commit-tree", tree, "-m", "unrelated")
    write(sb.outputs / "review.md", REVIEW_2.replace("round 2", "round 9"))
    refused("keep: a gates --head that is not HEAD exits 2", keep(sb, "gates", "9", head=sb.base, rounds=rounds),
            "is not HEAD", [rounds / "gates-9"])
    refused("keep: a --head that is not a full hash exits 2",
            keep(sb, "gates", "9", head=sb.head[:12], rounds=rounds), "must be a full commit hash",
            [rounds / "gates-9"])
    refused("keep: a --head HEAD exits 2", keep(sb, "gates", "9", head="HEAD", rounds=rounds),
            "must be a full commit hash", [rounds / "gates-9"])
    refused("keep: a review --head that is not an ancestor of HEAD exits 2",
            keep(sb, "review", "9", "--no-address", head=unrelated, rounds=rounds), "not HEAD or an ancestor",
            [rounds / "review-9"])
    write(sb.repo / "README.md", "changed after the gates ran\n")
    refused("keep: a gates keep with a modified tracked file exits 2", keep(sb, "gates", "9", rounds=rounds),
            "README.md", [rounds / "gates-9"])
    sb.git("checkout", "-q", "--", "README.md")
    write(sb.repo / "stray.txt", "untracked\n")
    refused("keep: a gates keep with an untracked file outside .mintswarm/job/ exits 2",
            keep(sb, "gates", "9", rounds=rounds), "stray.txt", [rounds / "gates-9"])
    (sb.repo / "stray.txt").unlink()
    (sb.outputs / "review.md").unlink()


def stale_tests(sb: Sandbox) -> Path:
    """Keep rounds at the base and at the head in their own runner state; return its rounds directory."""
    rounds = sb.runner_state / "jobs" / "stale" / "rounds"
    sb.git("checkout", "-q", "--detach", sb.base)
    ready_gates(sb.repo, T0 + 5000)
    proc = keep(sb, "gates", "1", head=sb.base, rounds=rounds)
    T.check("keep: a gates round kept with the base checked out", proc.returncode == 0, detail(proc))
    sb.git("checkout", "-q", "master")
    ready_gates(sb.repo, T0 + 5100)
    proc = keep(sb, "gates", "2", rounds=rounds)
    T.check("keep: a later gates round kept at the head", proc.returncode == 0, detail(proc))
    write(sb.outputs / "review.md", REVIEW_2.replace("round 2", "round 1"))
    proc = keep(sb, "review", "1", "--no-address", head=sb.base, rounds=rounds)
    T.check("keep: a review round kept at an ancestor of HEAD", proc.returncode == 0, detail(proc))
    check_round("keep: the review round records the ancestor it judged", sb, "review", 1,
                {"review.md": (sb.outputs / "review.md").read_bytes()}, rounds, sb.base)
    (sb.outputs / "review.md").unlink()
    return rounds


SHORT_ROUNDS = [
    ("a report short of the input's gates", ["whitespace", "agents-drift"], {"whitespace": "passed"}, 1),
    ("tests named out of the input's order", ["whitespace", "agents-drift"],
     {"agents-drift": "passed", "whitespace": "passed"}, 1),
    ("an input with no gates", [], {"whitespace": "passed"}, 2),
    ("a report that meets every condition", ["whitespace", "agents-drift"],
     {"whitespace": "passed", "agents-drift": "passed"}, 0),
]


def short_rounds(sb: Sandbox) -> Path:
    """Keep the SHORT_ROUNDS in their own job; return its rounds directory."""
    job = sb.tmp / "short"
    head = sb.job(job)
    rounds = sb.runner_state / "jobs" / "short" / "rounds"
    for number, (name, gates, statuses, want) in enumerate(SHORT_ROUNDS, 1):
        start = T0 + 100 * number
        write(job / JOB / "inputs" / "gates.json", gates_input_text(gates), start)
        write(job / JOB / "outputs" / "gates.ctrf.json", ctrf(statuses), start + 10)
        verdict = run_gates_verdict(sb, job)
        T.check(f"keep: run-gates.py --verdict exits {want} on {name}", verdict[0] == want, str(verdict))
        sb.short_verdicts.append(verdict)
        proc = keep(sb, "gates", str(number), cwd=job, head=head, rounds=rounds)
        T.check(f"keep: {name} is kept as gates {number}", proc.returncode == 0, detail(proc))
    return rounds


def agrees_with_run_gates(round_facts: dict, verdict: tuple) -> bool:
    """Whether a gate round's facts say what run-gates.py --verdict said while it ran."""
    code, line = verdict
    if code == 0:
        return round_facts["verdict"] == "passed" and round_facts["reason"] is None
    if code == 1:
        return round_facts["verdict"] == "failed" and line == f"gates failed: {round_facts['reason']}"
    return round_facts["verdict"] == "failed" and str(round_facts["reason"]).startswith("unusable gates.input.json")


# --- GitHub responses -----------------------------------------------------

def user(login: str, kind: str = "User") -> dict:
    return {"login": login, "id": sum(map(ord, login)), "type": kind}


BOT_USER = user("mintswarm-dev[bot]", "Bot")


def comment(ident: int, who, created: str, body: str, where: str = "pull",
            updated: Optional[str] = None) -> dict:
    number = PR if where == "pull" else ISSUE
    kind = "pull" if where == "pull" else "issues"
    return {
        "id": ident,
        "html_url": f"https://github.com/mintswarm/mintswarm/{kind}/{number}#issuecomment-{ident}",
        "user": who,
        "created_at": created,
        "updated_at": updated or created,
        "body": body,
    }


def at(hhmmss: str) -> str:
    return f"2026-11-22T{hhmmss}Z"


def build_responses(head: str) -> dict:
    fillers = [comment(1000 + i, BOT_USER, at("12:05:00"), f"pipeline state {i}\n", "issue") for i in range(98)]
    issue_page_1 = fillers + [
        comment(1100, BOT_USER, at("12:06:00"), f"see {MARKER} here\n", "issue"),
        comment(1101, BOT_USER, at("12:10:00"), f"A quoted relay:\n\n```\n{MARKER}\n```\n", "issue"),
    ]
    issue_page_2 = [
        comment(1200, user("zmij"), at("11:00:00"), "before the approval\n", "issue"),
        comment(1201, user("zmij"), at("12:00:00"), "at the approval time\n", "issue"),
        comment(1202, BOT_USER, at("13:00:00"), f"{MARKER}\n## Decision\n\nDo it.\n", "issue"),
        comment(1203, user("erin"), at("11:30:00"), "edited into a steer after the approval\n", "issue",
                updated=at("12:50:00")),
        comment(1204, user("erin"), at("11:00:00"), "edited before the approval\n", "issue",
                updated=at("11:45:00")),
    ]
    pr_comments = [
        comment(APPROVAL_ID, user("zmij"), at("12:00:01"), "approved\n"),
        comment(2001, user("alice"), at("12:20:00"), "please also do X\n", updated=at("12:25:00")),
        comment(2002, BOT_USER, at("12:30:00"), "<!-- mintswarm: state -->\npipeline state\n"),
        comment(2003, BOT_USER, at("12:40:00"), f"## Decision\r\n   {MARKER}  \r\nchange it\r\n"),
        comment(2004, user("github-actions[bot]", "Bot"), at("12:45:00"), f"{MARKER}\n"),
    ]
    reviews = [
        {"id": 3001, "user": user("bob"), "state": "COMMENTED", "submitted_at": at("14:00:00"), "body": "x",
         "html_url": f"https://github.com/mintswarm/mintswarm/pull/{PR}#pullrequestreview-3001"},
        {"id": 3002, "user": user("bob"), "state": "PENDING", "body": "draft",
         "html_url": f"https://github.com/mintswarm/mintswarm/pull/{PR}#pullrequestreview-3002"},
        {"id": 3003, "user": user("carol"), "state": "APPROVED", "submitted_at": at("11:30:00"), "body": "",
         "html_url": f"https://github.com/mintswarm/mintswarm/pull/{PR}#pullrequestreview-3003"},
    ]
    review_comments = [
        {"id": 4001, "user": user("dave"), "created_at": at("15:00:00"), "body": "inline",
         "html_url": f"https://github.com/mintswarm/mintswarm/pull/{PR}#discussion_r4001"},
        {"id": 4002, "user": None, "created_at": at("15:30:00"), "body": "from a deleted user",
         "html_url": f"https://github.com/mintswarm/mintswarm/pull/{PR}#discussion_r4002"},
        {"id": 4003, "user": user("machine[bot]"), "created_at": at("15:40:00"), "body": "bot",
         "html_url": f"https://github.com/mintswarm/mintswarm/pull/{PR}#discussion_r4003"},
        {"id": APPROVAL_ID, "user": user("frank"), "created_at": at("15:50:00"), "body": "same id, other kind",
         "html_url": f"https://github.com/mintswarm/mintswarm/pull/{PR}#discussion_r{APPROVAL_ID}"},
    ]
    ci, other = {"id": 1, "slug": "ci"}, {"id": 2, "slug": "other-ci"}
    check_runs = {"total_count": 4, "check_runs": [
        {"id": 10, "name": "build", "status": "completed", "conclusion": "success", "app": ci},
        {"id": 11, "name": "lint", "status": "in_progress", "conclusion": None, "app": ci},
        {"id": 5, "name": "build", "status": "completed", "conclusion": "failure", "app": ci},
        {"id": 3, "name": "build", "status": "completed", "conclusion": "failure", "app": other},
    ]}
    status = {"state": "success", "sha": head, "total_count": 1,
              "statuses": [{"context": "ci/legacy", "state": "success"}]}
    pull = {"number": PR, "state": "open", "head": {"sha": head, "ref": "mintswarm/47-landing-facts"},
            "base": {"sha": "0" * 40, "ref": "master"}}
    repo = "repos/{owner}/{repo}"
    q = "per_page=100&page="
    pages = {
        (f"{repo}/pulls/{PR}", "pull.1.json"): pull,
        (f"{repo}/commits/{head}/check-runs?filter=latest&{q}1", "check-runs.1.json"): check_runs,
        (f"{repo}/commits/{head}/status?{q}1", "status.1.json"): status,
        (f"{repo}/issues/{PR}/comments?{q}1", "pr-comments.1.json"): pr_comments,
        (f"{repo}/pulls/{PR}/reviews?{q}1", "reviews.1.json"): reviews,
        (f"{repo}/pulls/{PR}/comments?{q}1", "review-comments.1.json"): review_comments,
        (f"{repo}/issues/{ISSUE}/comments?{q}1", "issue-comments.1.json"): issue_page_1,
        (f"{repo}/issues/{ISSUE}/comments?{q}2", "issue-comments.2.json"): issue_page_2,
    }
    return {key: json.dumps(value, indent=1) + "\n" for key, value in pages.items()}


def steer(kind: str, author: str, time: str, url: str, via: str, edited_at: Optional[str] = None) -> dict:
    return {"kind": kind, "author": author, "time": time, "edited_at": edited_at, "url": url, "via": via}


PULL_URL = f"https://github.com/mintswarm/mintswarm/pull/{PR}"
ISSUE_URL = f"https://github.com/mintswarm/mintswarm/issues/{ISSUE}"
EXPECTED_STEERS = [
    steer("issue-comment", "erin", at("11:30:00"), f"{ISSUE_URL}#issuecomment-1203", "account", at("12:50:00")),
    steer("issue-comment", "mintswarm-dev[bot]", at("12:10:00"), f"{ISSUE_URL}#issuecomment-1101", "marker"),
    steer("pr-comment", "alice", at("12:20:00"), f"{PULL_URL}#issuecomment-2001", "account", at("12:25:00")),
    steer("pr-comment", "mintswarm-dev[bot]", at("12:40:00"), f"{PULL_URL}#issuecomment-2003", "marker"),
    steer("issue-comment", "mintswarm-dev[bot]", at("13:00:00"), f"{ISSUE_URL}#issuecomment-1202", "marker"),
    steer("review", "bob", at("14:00:00"), f"{PULL_URL}#pullrequestreview-3001", "account"),
    steer("review-comment", "dave", at("15:00:00"), f"{PULL_URL}#discussion_r4001", "account"),
    steer("review-comment", "ghost", at("15:30:00"), f"{PULL_URL}#discussion_r4002", "account"),
    steer("review-comment", "frank", at("15:50:00"), f"{PULL_URL}#discussion_r{APPROVAL_ID}", "account"),
]
APPROVAL_STEER = steer("pr-comment", "zmij", at("12:00:01"), APPROVAL_URL, "account")
EXPECTED_CHECKS = [
    {"name": "build", "conclusion": "success", "source": "check-run", "app": "ci"},
    {"name": "build", "conclusion": "failure", "source": "check-run", "app": "other-ci"},
    {"name": "lint", "conclusion": None, "source": "check-run", "app": "ci"},
    {"name": "ci/legacy", "conclusion": "success", "source": "status", "app": None},
]

STUB_GH = """#!{python}
import json, os, sys
argv = sys.argv[1:]
with open(os.path.join(os.environ["MSW_STUB_STATE"], "gh-argv.log"), "a") as log:
    log.write(json.dumps({{"argv": argv, "token_ok": os.environ.get("GH_TOKEN") == "{token}",
                          "repo": os.environ.get("GH_REPO"), "msw_token_unset": "MSW_TOKEN" not in os.environ}}) + "\\n")
with open(os.environ["MSW_STUB_PAGES"]) as f:
    pages = json.load(f)
path = argv[-1] if argv else ""
if path not in pages:
    sys.stderr.write("gh stub: HTTP 404 for " + path + "\\n")
    sys.exit(1)
sys.stdout.write(pages[path])
"""


def fetch_env(sb: Sandbox, pages: dict, name: str) -> dict:
    served = sb.tmp / f"pages-{name}.json"
    served.write_text(json.dumps({path: body for (path, _), body in pages.items()}), encoding="utf-8")
    env = dict(sb.env)
    env.update({
        "PATH": f"{sb.stubs}{os.pathsep}{os.environ.get('PATH', '')}",
        "MSW_TOKEN": DUMMY_TOKEN,
        "MSW_STUB_STATE": str(sb.state),
        "MSW_STUB_PAGES": str(served),
    })
    return env


def recorded(sb: Sandbox) -> list:
    log = sb.state / "gh-argv.log"
    entries = [json.loads(line) for line in log.read_text().splitlines()] if log.exists() else []
    log.unlink(missing_ok=True)
    return entries


def fetch_tests(sb: Sandbox, pages: dict) -> bool:
    stub = sb.stubs / "gh"
    stub.write_text(STUB_GH.format(python=sys.executable, token=DUMMY_TOKEN), encoding="utf-8")
    stub.chmod(0o755)
    env = fetch_env(sb, pages, "good")
    resolved = shutil.which("gh", path=env["PATH"])
    if not T.check("fetch: gh resolves to the stub", resolved == str(stub), f"gh is {resolved}"):
        return False
    if not T.check("fetch: bot-gh.sh is present", BOT_GH.is_file(), str(BOT_GH)):
        return False

    base_args = ["fetch", "--pr", str(PR), "--issue", str(ISSUE), "--head", sb.head]
    proc = sb.run(COLLECT, base_args, env=env)
    calls = recorded(sb)
    T.check("fetch: exits 0", proc.returncode == 0, detail(proc))
    T.check("fetch: one call per page", len(calls) == len(pages), f"{len(calls)} calls")
    bad = [c["argv"] for c in calls
           if c["argv"][:3] != ["api", "--method", "GET"] or len(c["argv"]) != 4
           or FORBIDDEN & set(c["argv"]) or c["argv"][3].startswith("-")]
    T.check("fetch: every call is api --method GET PATH with no field, input or paginate flag", not bad, str(bad))
    T.check("fetch: bot-gh.sh passes the dummy token as GH_TOKEN and sets GH_REPO",
            all(c["token_ok"] and c["repo"] == "mintswarm/mintswarm" and c["msw_token_unset"] for c in calls))
    issue_paths = [c["argv"][3] for c in calls if f"/issues/{ISSUE}/comments" in c["argv"][3]]
    T.check("fetch: the issue comments come in two pages",
            issue_paths == [f"repos/{{owner}}/{{repo}}/issues/{ISSUE}/comments?per_page=100&page={k}" for k in (1, 2)],
            str(issue_paths))
    out = sb.repo / RESPONSES
    saved = sorted(p.name for p in out.iterdir()) if out.is_dir() else []
    T.check("fetch: every page and request.json are saved",
            saved == sorted([name for _, name in pages] + ["request.json"]), str(saved))
    T.check("fetch: pages are saved byte for byte",
            all((out / name).is_file() and (out / name).read_text(encoding="utf-8") == body
                for (_, name), body in pages.items()))
    try:
        request = json.loads((out / "request.json").read_text(encoding="utf-8"))
        ok = list(request) == ["pr", "issue", "head", "fetched_at"] and \
            (request["pr"], request["issue"], request["head"]) == (PR, ISSUE, sb.head)
    except (OSError, ValueError, KeyError):
        ok = False
    T.check("fetch: request.json records pr, issue, head and fetched_at", ok)
    T.check("fetch: the 100-item first page is read in full",
            len(json.loads((out / "issue-comments.1.json").read_text())) == 100)

    proc = sb.run(COLLECT, base_args, env=env)
    recorded(sb)
    parent = out.parent
    stray = [p.name for p in parent.iterdir() if p.name != out.name]
    T.check("fetch: a second fetch replaces the first and leaves nothing beside it",
            proc.returncode == 0 and out.is_dir() and not stray, detail(proc) + f" {stray}")

    failing = {k: v for k, v in pages.items() if k[1] != "reviews.1.json"}
    not_json = {k: (v if k[1] != "status.1.json" else "not json\n") for k, v in pages.items()}
    endless = {k: v for k, v in pages.items() if not k[1].startswith("issue-comments")}
    filler = json.dumps([comment(5000, BOT_USER, at("12:05:00"), "x", "issue")] * 100)
    for page in range(1, 32):
        endless[(f"repos/{{owner}}/{{repo}}/issues/{ISSUE}/comments?per_page=100&page={page}", f"p{page}")] = filler
    for name, served in (("a failing call", failing), ("a page that is not JSON", not_json),
                         ("more than 30 pages", endless)):
        target = sb.tmp / f"responses-{name.replace(' ', '-')}"
        proc = sb.run(COLLECT, [*base_args, "--out", str(target)], env=fetch_env(sb, served, name.replace(" ", "-")))
        calls = recorded(sb)
        stray = [p.name for p in sb.tmp.iterdir() if p.name.startswith(target.name)]
        T.check(f"fetch: {name} exits 1 and leaves no directory",
                proc.returncode == 1 and not stray, detail(proc) + f" {stray}")
        if name == "more than 30 pages":
            T.check("fetch: no page after the 30th is requested",
                    not any("page=31" in c["argv"][-1] for c in calls))
    for args in (["--head", "HEAD"], ["--pr", "0"], ["--extra", "1"]):
        bad_args = list(base_args)
        if args[0] in bad_args:
            bad_args[bad_args.index(args[0]) + 1] = args[1]
        else:
            bad_args += args
        proc = sb.run(COLLECT, bad_args, env=env)
        T.check(f"fetch: bad argument {args} exits 2 without calling gh",
                proc.returncode == 2 and not recorded(sb), detail(proc))
    return True


# --- assemble -------------------------------------------------------------

def assemble(sb: Sandbox, extra: Optional[list] = None, replace: Optional[dict] = None,
             cwd: Optional[Path] = None):
    args = {
        "--responses": str(RESPONSES), "--pr": str(PR), "--issue": str(ISSUE), "--head": "HEAD",
        "--base": "HEAD~1", "--approver": "zmij", "--approved-at": APPROVED_AT, "--approval-url": APPROVAL_URL,
        "--rounds": str(sb.rounds),
    }
    args.update(replace or {})
    argv = ["assemble"]
    for key, value in args.items():
        if value is not None:
            argv += [key, value]
    argv += extra or []
    facts = (cwd or sb.repo) / FACTS
    write(facts, '{"stale": true}\n')
    proc = sb.run(COLLECT, argv, cwd=cwd)
    try:
        data = json.loads(facts.read_text(encoding="utf-8")) if facts.exists() else None
    except ValueError:
        data = "unparsable"
    return proc, data


def round_files(sb: Sandbox, kind: str, number: int) -> list:
    return json.loads((sb.rounds / f"{kind}-{number}" / "round.json").read_text(encoding="utf-8"))["files"]


def copy_rounds(sb: Sandbox, name: str) -> Path:
    target = sb.tmp / name
    shutil.copytree(sb.rounds, target)
    for path in target.rglob("*"):
        if path.is_file():
            path.chmod(0o644)
    return target


def written(proc, facts) -> bool:
    return proc.returncode == 0 and isinstance(facts, dict)


def without_file(sb: Sandbox, name: str, round_dir: str, file: str) -> Path:
    """A copy of the kept rounds with one file removed from a round and its round.json."""
    target = copy_rounds(sb, name)
    (target / round_dir / file).unlink()
    record = json.loads((target / round_dir / "round.json").read_text(encoding="utf-8"))
    record["files"] = [entry for entry in record["files"] if entry["name"] != file]
    write(target / round_dir / "round.json", json.dumps(record, indent=2) + "\n")
    return target


def rewrite_to_pass(rounds: Path) -> None:
    """Rewrite every gates round under `rounds` to a passing report, with matching sha256 values."""
    for path in rounds.rglob("*"):
        path.chmod(0o755 if path.is_dir() else 0o644)
    for round_dir in sorted(rounds.glob("gates-*")):
        report = write(round_dir / "gates.ctrf.json", ctrf(GATE_ROUNDS[2]))
        record = json.loads((round_dir / "round.json").read_text(encoding="utf-8"))
        for entry in record["files"]:
            if entry["name"] == "gates.ctrf.json":
                entry["sha256"] = sha(report)
        write(round_dir / "round.json", json.dumps(record, indent=2) + "\n")


def runner_state_tests(sb: Sandbox, baseline: dict, stale: Path) -> None:
    """Rounds are read from runner state only, and a round from another commit is not current."""
    proc, facts = assemble(sb, replace={"--rounds": str(stale)})
    if written(proc, facts):
        got = [(g["round"], g["commit"], g["current"], g["verdict"]) for g in facts["gate_rounds"]]
        want = [(1, sb.base, False, "passed"), (2, sb.head, True, "passed")]
        T.check("assemble: a gate round kept at the base is not current and keeps its verdict", got == want, str(got))
        got = [(r["round"], r["commit"], r["current"], r["verdict"]) for r in facts["review_rounds"]]
        T.check("assemble: a review round kept at the base is not current and keeps its verdict",
                got == [(1, sb.base, False, "pass")], str(got))
    else:
        T.check("assemble: rounds kept at the base and the head", False, detail(proc))

    writable = sb.repo / JOB / "rounds"
    shutil.copytree(sb.rounds, writable)
    rewrite_to_pass(writable)
    outside = sb.tmp / "rounds-rewritten"
    shutil.copytree(writable, outside)
    proc, facts = assemble(sb, replace={"--rounds": str(outside)})
    T.check("assemble: the rewritten copy, outside the repository, reads as passed",
            written(proc, facts) and [g["verdict"] for g in facts["gate_rounds"]] == ["passed"] * 3, detail(proc))
    proc, facts = assemble(sb)
    T.check("assemble: a rewritten copy under .mintswarm/job/rounds/ leaves the facts unchanged",
            written(proc, facts) and {k: v for k, v in facts.items() if k != "fetched_at"}
            == {k: v for k, v in baseline.items() if k != "fetched_at"}, detail(proc))
    proc, facts = assemble(sb, replace={"--rounds": str(writable)})
    T.check("assemble: the rewritten copy under .mintswarm/job/rounds/ given as --rounds exits 2",
            proc.returncode == 2 and facts is None and "inside the worktree" in proc.stderr, detail(proc))

    link = sb.runner_state / "assemble-link"
    link.symlink_to(sb.rounds)
    for name, value, fragment in (("without --rounds", None, "missing --rounds"),
                                  ("with a relative --rounds", "rounds", "must be an absolute path"),
                                  ("with a symlinked --rounds", str(link), "is a symbolic link")):
        proc, facts = assemble(sb, replace={"--rounds": value})
        T.check(f"assemble: {name} exits 2 with no facts file",
                proc.returncode == 2 and facts is None and fragment in proc.stderr, detail(proc))

    nested = nested_worktree(sb)
    in_main = sb.repo / "state" / "rounds"
    shutil.copytree(sb.rounds, in_main)
    responses = str(sb.repo / RESPONSES)
    proc, facts = assemble(sb, replace={"--responses": responses, "--rounds": str(in_main)}, cwd=nested)
    T.check("assemble: from a nested worktree, --rounds inside the main checkout exits 2 with no facts file",
            proc.returncode == 2 and facts is None and "inside the main checkout" in proc.stderr, detail(proc))
    proc, facts = assemble(sb, replace={"--responses": responses}, cwd=nested)
    T.check("assemble: from a nested worktree, rounds in runner state are read",
            written(proc, facts) and facts["gate_rounds"] == baseline["gate_rounds"], detail(proc))
    for path in in_main.rglob("*"):
        path.chmod(0o755 if path.is_dir() else 0o644)
    shutil.rmtree(sb.repo / "state")


def assemble_tests(sb: Sandbox, short: Path, stale_rounds: Path) -> None:
    proc, facts = assemble(sb)
    if not T.check("assemble: exits 0 and writes facts.json", proc.returncode == 0 and isinstance(facts, dict),
                   detail(proc)):
        return
    T.check("assemble: keys in order", list(facts) == FACTS_KEYS, str(list(facts)))
    T.check("assemble: tool, version, pr and issue",
            (facts["tool"], facts["version"], facts["pr"], facts["issue"]) == ("mintswarm-landing-facts", 2, PR, ISSUE))
    T.check("assemble: head and base are the full hashes", (facts["head"], facts["base"]) == (sb.head, sb.base),
            f"{facts['head']} {facts['base']}")
    request = json.loads((sb.repo / RESPONSES / "request.json").read_text(encoding="utf-8"))
    T.check("assemble: fetched_at is request.json's", facts["fetched_at"] == request["fetched_at"],
            f"{facts['fetched_at']} {request['fetched_at']}")
    T.check("assemble: checks from check runs (latest per app and name) and statuses, sorted",
            facts["checks"] == EXPECTED_CHECKS, json.dumps(facts["checks"]))
    gates = facts["gate_rounds"]
    T.check("assemble: three gate rounds in round order", [g["round"] for g in gates] == [1, 2, 3],
            str([g.get("round") for g in gates]))
    for g, (verdict, failed, reason) in zip(gates, EXPECTED_GATES):
        T.check(f"assemble: gate round {g['round']} verdict, failed tests and reason",
                list(g) == GATE_KEYS and (g["commit"], g["current"]) == (sb.head, True)
                and (g["verdict"], g["failed_tests"], g["reason"]) == (verdict, failed, reason)
                and g["files"] == round_files(sb, "gates", g["round"]), json.dumps(g))
    T.check("assemble: every gate round says what run-gates.py --verdict said",
            len(gates) == len(sb.gate_verdicts)
            and all(agrees_with_run_gates(g, v) for g, v in zip(gates, sb.gate_verdicts)),
            f"{[(g['verdict'], g['reason']) for g in gates]} {sb.gate_verdicts}")
    reviews = facts["review_rounds"]
    expected_reviews = [
        (1, "changes_required", {"blocker": 0, "major": 1, "minor": 2, "nit": 1}),
        (2, "pass", {"blocker": 0, "major": 0, "minor": 0, "nit": 0}),
    ]
    T.check("assemble: two review rounds in round order", [r["round"] for r in reviews] == [1, 2])
    for r, (number, verdict, counts) in zip(reviews, expected_reviews):
        T.check(f"assemble: review round {number} verdict and counts by severity",
                list(r) == REVIEW_KEYS and (r["commit"], r["current"]) == (sb.head, True)
                and (r["round"], r["verdict"], r["findings"]) == (number, verdict, counts)
                and list(r["findings"]) == ["blocker", "major", "minor", "nit"]
                and r["files"] == round_files(sb, "review", number), json.dumps(r))
    T.check("assemble: steers (human, marked mintswarm-dev[bot], review, inline, ghost; not the rest)",
            facts["steers"] == EXPECTED_STEERS, json.dumps(facts["steers"], indent=1))
    baseline = facts
    proc, short_facts = assemble(sb, replace={"--rounds": str(short)})
    gates = short_facts.get("gate_rounds", []) if written(proc, short_facts) else []
    T.check("assemble: a report short of the input's gates is failed, as run-gates.py --verdict has it",
            bool(gates) and (gates[0]["verdict"], gates[0]["reason"])
            == ("failed", "summary.tests is 1, the input has 2 gates"), detail(proc) + f" {gates[:1]}")
    for g, v, (name, *_) in zip(gates, sb.short_verdicts, SHORT_ROUNDS):
        T.check(f"assemble: {name} says what run-gates.py --verdict said", agrees_with_run_gates(g, v),
                f"{g['verdict']} {g['reason']!r} against {v}")
    T.check("assemble: every short-report round is in the facts", len(gates) == len(SHORT_ROUNDS), str(len(gates)))
    T.check("assemble: the approval without kickoff floor or assessment",
            facts["approval"] == {"approver": "zmij", "approved_at": APPROVED_AT, "approval_ref": APPROVAL_URL}
            and list(facts["approval"]) == ["approver", "approved_at", "approval_ref"], json.dumps(facts["approval"]))

    proc, facts = assemble(sb, replace={"--approval-url": None, "--approval-id": str(APPROVAL_ID)})
    T.check("assemble: the approval excluded by id",
            written(proc, facts) and facts["steers"] == EXPECTED_STEERS
            and facts["approval"]["approval_ref"] == str(APPROVAL_ID), detail(proc))
    proc, facts = assemble(sb, replace={"--approval-url": None})
    T.check("assemble: without URL or id the approval comment after the time is a steer",
            written(proc, facts) and facts["steers"] == sorted(
                EXPECTED_STEERS + [APPROVAL_STEER], key=lambda s: (s["time"], s["kind"], s["url"]))
            and facts["approval"]["approval_ref"] is None, detail(proc))

    empty = sb.tmp / "responses-no-checks"
    shutil.copytree(sb.repo / RESPONSES, empty)
    write(empty / "check-runs.1.json", '{"total_count": 0, "check_runs": []}\n')
    write(empty / "status.1.json", '{"state": "pending", "total_count": 0, "statuses": []}\n')
    proc, facts = assemble(sb, replace={"--responses": str(empty)})
    T.check("assemble: no checks give an empty list", written(proc, facts) and facts.get("checks") == [],
            detail(proc))

    floor_record = {"tool": "mintswarm-plan-floor", "version": 1, "verdict": "hit"}
    floor_data = write(sb.tmp / "floor.json", json.dumps(floor_record) + "\n")
    assessment = write(sb.tmp / "assessment.md", "# Assessment: x\n\nmedium risk: y\n\nRisk: medium\n")
    decision_record = {"tool": "mintswarm-approval-decision", "version": 1, "decision": "auto"}
    decision_data = write(sb.tmp / "decision.json", json.dumps(decision_record) + "\n")
    proc, facts = assemble(sb, ["--kickoff-floor", str(sb.tmp / "floor.json"),
                                "--assessment", str(sb.tmp / "assessment.md"),
                                "--decision", str(sb.tmp / "decision.json")],
                           {"--approver": "mintswarm-dev[bot]"})
    ok = written(proc, facts) and list(facts["approval"]) == [
        "approver", "approved_at", "approval_ref", "decision", "kickoff_floor", "assessment"]
    if ok:
        ok = facts["approval"]["approver"] == "mintswarm-dev[bot]" and facts["approval"]["decision"] == {
            "path": str(sb.tmp / "decision.json"), "sha256": sha(decision_data), "record": decision_record,
        } and facts["approval"]["kickoff_floor"] == {
            "path": str(sb.tmp / "floor.json"), "sha256": sha(floor_data), "record": floor_record,
        } and facts["approval"]["assessment"] == {
            "path": str(sb.tmp / "assessment.md"), "sha256": sha(assessment), "risk": "medium",
        }
    T.check("assemble: an approval by mintswarm-dev[bot] with decision, kickoff floor and assessment", ok,
            detail(proc))
    write(sb.tmp / "assessment-2.md", "# Assessment: x\n\nRisk: low\nRisk: high\n")
    proc, facts = assemble(sb, ["--assessment", str(sb.tmp / "assessment-2.md")])
    T.check("assemble: an assessment with two Risk lines has risk null, and no decision or kickoff floor key",
            written(proc, facts) and facts["approval"]["assessment"]["risk"] is None
            and "kickoff_floor" not in facts["approval"] and "decision" not in facts["approval"], detail(proc))

    tampered = copy_rounds(sb, "rounds-tampered")
    with open(tampered / "gates-1" / "gates.md", "a", encoding="utf-8") as f:
        f.write("all passed\n")
    gap = copy_rounds(sb, "rounds-gap")
    shutil.rmtree(gap / "gates-2")
    verdict = copy_rounds(sb, "rounds-verdict")
    text = REVIEW_1.replace("\nchanges_required\n", "\nmaybe\n").encode("utf-8")
    write(verdict / "review-1" / "review.md", text)
    record = json.loads((verdict / "review-1" / "round.json").read_text(encoding="utf-8"))
    for entry in record["files"]:
        if entry["name"] == "review.md":
            entry["sha256"] = sha(text)
    write(verdict / "review-1" / "round.json", json.dumps(record, indent=2) + "\n")
    stray = copy_rounds(sb, "rounds-stray")
    write(stray / "gates-1" / "notes.txt", "extra\n")
    moved = sb.tmp / "responses-moved-head"
    shutil.copytree(sb.repo / RESPONSES, moved)
    pull = json.loads((moved / "pull.1.json").read_text(encoding="utf-8"))
    pull["head"]["sha"] = sb.base
    write(moved / "pull.1.json", json.dumps(pull) + "\n")
    stale = sb.tmp / "responses-bad-fetched-at"
    shutil.copytree(sb.repo / RESPONSES, stale)
    request = json.loads((stale / "request.json").read_text(encoding="utf-8"))
    request["fetched_at"] = "yesterday"
    write(stale / "request.json", json.dumps(request) + "\n")
    no_input = without_file(sb, "rounds-no-input", "gates-2", "gates.input.json")
    version_1 = copy_rounds(sb, "rounds-version-1")
    record = json.loads((version_1 / "gates-1" / "round.json").read_text(encoding="utf-8"))
    record["version"] = 1
    del record["head"]
    write(version_1 / "gates-1" / "round.json", json.dumps(record, indent=2) + "\n")
    cases = [
        ("a head that is not the one fetched", {"--head": "HEAD~1"}, []),
        ("a request.json without a usable fetched_at", {"--responses": str(stale)}, []),
        ("a gates round without its input", {"--rounds": str(no_input)}, []),
        ("an --approver that is not a login", {"--approver": "zmij[bot]x"}, []),
        ("a PR whose head is not the head", {"--responses": str(moved)}, []),
        ("a tampered kept file", {"--rounds": str(tampered)}, []),
        ("a gap in round numbers", {"--rounds": str(gap)}, []),
        ("a version 1 round.json", {"--rounds": str(version_1)}, []),
        ("an unreadable review verdict", {"--rounds": str(verdict)}, []),
        ("a file not listed in round.json", {"--rounds": str(stray)}, []),
        ("a bad --approved-at", {"--approved-at": "2026-11-22 12:00:00"}, []),
        ("an --approved-at that is not a date", {"--approved-at": "2026-02-30T12:00:00Z"}, []),
        ("a bad --approver", {"--approver": "zmij;rm"}, []),
        ("a request for another issue", {"--issue": "48"}, []),
        ("a missing --base", {"--base": None}, []),
        ("a base starting with -", {"--base": "--all"}, []),
    ]
    for name, replace, extra in cases:
        proc, facts = assemble(sb, extra, replace)
        T.check(f"assemble: {name} exits 2 with no facts file", proc.returncode == 2 and facts is None,
                detail(proc))
    proc, facts = assemble(sb, ["--kickoff-floor", str(sb.tmp / "assessment.md")])
    T.check("assemble: a kickoff floor that is not floor.py's exits 2 with no facts file",
            proc.returncode == 2 and facts is None, detail(proc))
    proc, facts = assemble(sb, ["--decision", str(sb.tmp / "floor.json")])
    T.check("assemble: a decision that is not decision.py's exits 2 with no facts file",
            proc.returncode == 2 and facts is None, detail(proc))
    runner_state_tests(sb, baseline, stale_rounds)


# --- static ---------------------------------------------------------------

def static_tests() -> None:
    for path in SCRIPTS:
        tree = ast.parse(path.read_text(encoding="utf-8"))
        names = set()
        for node in ast.walk(tree):
            if isinstance(node, ast.Import):
                names.update(alias.name.split(".")[0] for alias in node.names)
            elif isinstance(node, ast.ImportFrom):
                names.add((node.module or "").split(".")[0])
        outside = sorted(n for n in names if n not in sys.stdlib_module_names)
        T.check(f"static: {path.name} imports only the standard library", not outside, str(outside))
    doc = " ".join((ast.get_docstring(ast.parse(KEEP.read_text(encoding="utf-8"))) or "").split())
    T.check("static: keep-round.py's docstring states the session sequence",
            "report-gates.py --clean" in doc and "next review round" in doc
            and "run-gates.py and report-gates.py are unchanged" in doc)
    doc = " ".join((ast.get_docstring(ast.parse(COLLECT.read_text(encoding="utf-8"))) or "").split())
    T.check("static: collect-facts.py's docstring states the steer gap",
            "Steers given only in the session chat" in doc and MARKER in doc)
    for path in (KEEP, COLLECT):
        doc = " ".join((ast.get_docstring(ast.parse(path.read_text(encoding="utf-8"))) or "").split())
        T.check(f"static: {path.name}'s docstring says rounds outside the worktree are a convention",
                "not an OS barrier" in doc and "runner state" in doc)


def main() -> int:
    for tool in ("git", "yq", "bash"):
        if shutil.which(tool) is None:
            print(f"facts-selftest: {tool} is not on PATH", file=sys.stderr)
            return 2
    static_tests()
    with tempfile.TemporaryDirectory(prefix="msw-facts-selftest.") as tmp:
        sb = Sandbox(Path(tmp).resolve())
        keep_tests(sb)
        location_tests(sb)
        head_tests(sb)
        stale_rounds = stale_tests(sb)
        short = short_rounds(sb)
        pages = build_responses(sb.head)
        if fetch_tests(sb, pages):
            assemble_tests(sb, short, stale_rounds)
        else:
            print("facts-selftest: the fetch sandbox is not safe; assemble not run", file=sys.stderr)
            T.failed += 1
        for path in sb.runner_state.rglob("*"):
            if not path.is_symlink():
                path.chmod(0o755 if path.is_dir() else 0o644)
    print(f"{T.passed} passed, {T.failed} failed")
    return 0 if T.failed == 0 else 1


if __name__ == "__main__":
    sys.exit(main())
