#!/usr/bin/env python3
"""Offline self-test of keep-round.py and collect-facts.py.

    facts-selftest.py

Needs python3, `git` and `yq` on PATH, and bash for bot-gh.sh; no network.
It works in a temporary sandbox with its own HOME, no system or global git
config and a fixed identity: a throwaway repository with a base and a head
commit and the job layout under .mintswarm/job/. Both scripts run as
`sys.executable SCRIPT ...` with that repository as their working directory.

Checks:

- keeping rounds: three gates rounds and two review rounds are kept (review
  2 with --no-address) and every copy is byte-identical under its round
  number, with mode 0444 and a matching round.json; keeping gates 2 again
  exits 1 and leaves bytes, modes and mtimes unchanged; a wrong review
  title, a stale address title without --no-address, a gates.md older than
  inputs/gates.json and a missing review.md each exit 2 and leave no final
  directory and no temporary one; an empty gates round is kept with a
  warning; bad arguments and a missing inputs/gates.json exit 2;
- fetch: a stub `gh` that records its argv as JSON lines and serves
  hand-written pages goes first on PATH, and the test aborts unless `gh`
  resolves to it; MSW_TOKEN is the dummy ghs_selftest and the real
  .mintswarm/dogfood/github/bot-gh.sh runs. Every recorded argv is
  `api --method GET PATH` with no field, input or paginate flag, the issue
  comments come in two pages (100 and 3 items), and the pages are saved
  byte for byte; a failing call, a page that is not JSON and more than 30
  pages exit 1 and leave no directory; a second fetch replaces the first;
- assemble, on the saved responses: checks from check runs and statuses,
  and an empty list when there are none; gate and review rounds in round
  order with verdicts, failed tests and severity counts; the steer cases
  (a human after the approval, a human before and at the approval time,
  the approval by URL and by id, bots without the marker, mintswarm-dev[bot]
  with the marker line, indented and inside a code fence, the marker
  mid-line, another bot with the marker, a pending review, an inline review
  comment, a deleted user); the approval with and without the kickoff floor
  and the assessment; exit 2 with no facts file for a head mismatch, a
  tampered kept file, a gap in round numbers, an unreadable review verdict
  and a bad --approved-at;
- static: every import of the three scripts is in the standard library,
  and keep-round.py's docstring names report-gates.py --clean and the next
  review round.

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

HERE = Path(__file__).resolve().parent
KEEP = HERE / "keep-round.py"
COLLECT = HERE / "collect-facts.py"
BOT_GH = HERE.parent / "github" / "bot-gh.sh"
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
FACTS_KEYS = ["tool", "version", "pr", "issue", "head", "base", "checks",
              "gate_rounds", "review_rounds", "steers", "approval"]
ROUND_KEYS = ["tool", "version", "kind", "round", "kept_at", "files"]
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
        self.git("commit", "-q", "--allow-empty", "-m", "base")
        self.base = self.git("rev-parse", "HEAD")
        self.git("commit", "-q", "--allow-empty", "-m", "head")
        self.head = self.git("rev-parse", "HEAD")
        self.outputs = self.repo / JOB / "outputs"
        self.inputs = self.repo / JOB / "inputs"
        self.rounds = self.repo / JOB / "rounds"
        self.outputs.mkdir(parents=True)
        self.inputs.mkdir(parents=True)

    def git(self, *args: str) -> str:
        proc = subprocess.run(["git", *args], cwd=self.repo, env=self.env, capture_output=True, text=True)
        if proc.returncode != 0:
            raise RuntimeError(f"git {' '.join(args)} failed: {proc.stderr.strip()}")
        return proc.stdout.strip()

    def run(self, script: Path, args: list, env: dict = None, cwd: Path = None):
        return subprocess.run(
            [sys.executable, str(script), *args],
            cwd=cwd or self.repo, env=env or self.env, capture_output=True, text=True,
        )


def write(path: Path, data, mtime: int = None) -> bytes:
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


def keep(sb: Sandbox, *args: str, cwd: Path = None):
    return sb.run(KEEP, list(args), cwd=cwd)


def check_round(name: str, sb: Sandbox, kind: str, number: int, expected: dict) -> None:
    path = sb.rounds / f"{kind}-{number}"
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
        want = {"tool": "mintswarm-keep-round", "version": 1, "kind": kind, "round": number,
                "files": [{"name": f, "sha256": sha(d)} for f, d in sorted(expected.items())]}
        for key, value in want.items():
            if record.get(key) != value:
                problems.append(f"round.json {key} is {record.get(key)!r}")
    except (OSError, ValueError) as exc:
        problems.append(f"round.json unreadable: {exc}")
    T.check(name, not problems, "; ".join(problems))


def keep_tests(sb: Sandbox) -> None:
    gates_json = sb.inputs / "gates.json"
    gates_input = json.dumps({"gates": [{"name": n, "command": "true"} for n in GATE_ROUNDS[0]]}) + "\n"
    kept = {}
    for number, statuses in enumerate(GATE_ROUNDS, 1):
        for old in sb.outputs.glob("gates.*"):
            old.unlink()
        start = T0 + 100 * number
        write(gates_json, gates_input, start)
        files = {
            "gates.ctrf.json": write(sb.outputs / "gates.ctrf.json", ctrf(statuses), start + 10),
            "gates.md": write(sb.outputs / "gates.md", f"# Gates: Keep rounds\n\nRound {number}.\n", start + 20),
        }
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
                 ["gates", "5", "--no-address"], ["other", "1"]):
        proc = keep(sb, *args)
        T.check(f"keep: bad arguments {args} exit 2", proc.returncode == 2, detail(proc))

    other = sb.tmp / "other"
    (other / JOB / "outputs").mkdir(parents=True)
    proc = keep(sb, "gates", "1", cwd=other)
    T.check("keep: a gates round without inputs/gates.json exits 2",
            proc.returncode == 2 and not (other / JOB / "rounds" / "gates-1").exists(), detail(proc))
    write(other / JOB / "inputs" / "gates.json", gates_input)
    proc = keep(sb, "gates", "1", cwd=other)
    empty = other / JOB / "rounds" / "gates-1"
    ok = proc.returncode == 0 and "warning" in proc.stderr and empty.is_dir()
    if ok:
        record = json.loads((empty / "round.json").read_text(encoding="utf-8"))
        ok = record["files"] == [] and [p.name for p in empty.iterdir()] == ["round.json"]
    T.check("keep: a gates round with neither file is kept empty, with a warning", ok, detail(proc))


# --- GitHub responses -----------------------------------------------------

def user(login: str, kind: str = "User") -> dict:
    return {"login": login, "id": sum(map(ord, login)), "type": kind}


BOT_USER = user("mintswarm-dev[bot]", "Bot")


def comment(ident: int, who, created: str, body: str, where: str = "pull") -> dict:
    number = PR if where == "pull" else ISSUE
    kind = "pull" if where == "pull" else "issues"
    return {
        "id": ident,
        "html_url": f"https://github.com/mintswarm/mintswarm/{kind}/{number}#issuecomment-{ident}",
        "user": who,
        "created_at": created,
        "updated_at": created,
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
    ]
    pr_comments = [
        comment(APPROVAL_ID, user("zmij"), at("12:00:01"), "approved\n"),
        comment(2001, user("alice"), at("12:20:00"), "please also do X\n"),
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
    ]
    check_runs = {"total_count": 3, "check_runs": [
        {"id": 10, "name": "build", "status": "completed", "conclusion": "success"},
        {"id": 11, "name": "lint", "status": "in_progress", "conclusion": None},
        {"id": 5, "name": "build", "status": "completed", "conclusion": "failure"},
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


def steer(kind: str, author: str, time: str, url: str, via: str) -> dict:
    return {"kind": kind, "author": author, "time": time, "url": url, "via": via}


PULL_URL = f"https://github.com/mintswarm/mintswarm/pull/{PR}"
ISSUE_URL = f"https://github.com/mintswarm/mintswarm/issues/{ISSUE}"
EXPECTED_STEERS = [
    steer("issue-comment", "mintswarm-dev[bot]", at("12:10:00"), f"{ISSUE_URL}#issuecomment-1101", "marker"),
    steer("pr-comment", "alice", at("12:20:00"), f"{PULL_URL}#issuecomment-2001", "account"),
    steer("pr-comment", "mintswarm-dev[bot]", at("12:40:00"), f"{PULL_URL}#issuecomment-2003", "marker"),
    steer("issue-comment", "mintswarm-dev[bot]", at("13:00:00"), f"{ISSUE_URL}#issuecomment-1202", "marker"),
    steer("review", "bob", at("14:00:00"), f"{PULL_URL}#pullrequestreview-3001", "account"),
    steer("review-comment", "dave", at("15:00:00"), f"{PULL_URL}#discussion_r4001", "account"),
    steer("review-comment", "ghost", at("15:30:00"), f"{PULL_URL}#discussion_r4002", "account"),
]
APPROVAL_STEER = steer("pr-comment", "zmij", at("12:00:01"), APPROVAL_URL, "account")
EXPECTED_CHECKS = [
    {"name": "build", "conclusion": "success", "source": "check-run"},
    {"name": "lint", "conclusion": None, "source": "check-run"},
    {"name": "ci/legacy", "conclusion": "success", "source": "status"},
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

def assemble(sb: Sandbox, extra: list = None, replace: dict = None):
    args = {
        "--responses": str(RESPONSES), "--pr": str(PR), "--issue": str(ISSUE), "--head": "HEAD",
        "--base": "HEAD~1", "--approver": "zmij", "--approved-at": APPROVED_AT, "--approval-url": APPROVAL_URL,
    }
    args.update(replace or {})
    argv = ["assemble"]
    for key, value in args.items():
        if value is not None:
            argv += [key, value]
    argv += extra or []
    facts = sb.repo / FACTS
    write(facts, '{"stale": true}\n')
    proc = sb.run(COLLECT, argv)
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


def assemble_tests(sb: Sandbox) -> None:
    proc, facts = assemble(sb)
    if not T.check("assemble: exits 0 and writes facts.json", proc.returncode == 0 and isinstance(facts, dict),
                   detail(proc)):
        return
    T.check("assemble: keys in order", list(facts) == FACTS_KEYS, str(list(facts)))
    T.check("assemble: tool, version, pr and issue",
            (facts["tool"], facts["version"], facts["pr"], facts["issue"]) == ("mintswarm-landing-facts", 1, PR, ISSUE))
    T.check("assemble: head and base are the full hashes", (facts["head"], facts["base"]) == (sb.head, sb.base),
            f"{facts['head']} {facts['base']}")
    T.check("assemble: checks from check runs (latest per name) and statuses, sorted",
            facts["checks"] == EXPECTED_CHECKS, json.dumps(facts["checks"]))
    gates = facts["gate_rounds"]
    T.check("assemble: three gate rounds in round order", [g["round"] for g in gates] == [1, 2, 3],
            str([g.get("round") for g in gates]))
    for g, (verdict, failed, reason) in zip(gates, EXPECTED_GATES):
        T.check(f"assemble: gate round {g['round']} verdict, failed tests and reason",
                list(g) == ["round", "verdict", "failed_tests", "reason", "files"]
                and (g["verdict"], g["failed_tests"], g["reason"]) == (verdict, failed, reason)
                and g["files"] == round_files(sb, "gates", g["round"]), json.dumps(g))
    reviews = facts["review_rounds"]
    expected_reviews = [
        (1, "changes_required", {"blocker": 0, "major": 1, "minor": 2, "nit": 1}),
        (2, "pass", {"blocker": 0, "major": 0, "minor": 0, "nit": 0}),
    ]
    T.check("assemble: two review rounds in round order", [r["round"] for r in reviews] == [1, 2])
    for r, (number, verdict, counts) in zip(reviews, expected_reviews):
        T.check(f"assemble: review round {number} verdict and counts by severity",
                list(r) == ["round", "verdict", "findings", "files"]
                and (r["round"], r["verdict"], r["findings"]) == (number, verdict, counts)
                and list(r["findings"]) == ["blocker", "major", "minor", "nit"]
                and r["files"] == round_files(sb, "review", number), json.dumps(r))
    T.check("assemble: steers (human, marked mintswarm-dev[bot], review, inline, ghost; not the rest)",
            facts["steers"] == EXPECTED_STEERS, json.dumps(facts["steers"], indent=1))
    T.check("assemble: the approval without kickoff floor or assessment",
            facts["approval"] == {"approver": "zmij", "approved_at": APPROVED_AT, "approval_ref": APPROVAL_URL}
            and list(facts["approval"]) == ["approver", "approved_at", "approval_ref"], json.dumps(facts["approval"]))

    proc, facts = assemble(sb, replace={"--approval-url": None, "--approval-id": str(APPROVAL_ID)})
    T.check("assemble: the approval excluded by id",
            proc.returncode == 0 and facts["steers"] == EXPECTED_STEERS
            and facts["approval"]["approval_ref"] == str(APPROVAL_ID), detail(proc))
    proc, facts = assemble(sb, replace={"--approval-url": None})
    T.check("assemble: without URL or id the approval comment after the time is a steer",
            proc.returncode == 0 and facts["steers"] == sorted(
                EXPECTED_STEERS + [APPROVAL_STEER], key=lambda s: (s["time"], s["kind"], s["url"]))
            and facts["approval"]["approval_ref"] is None, detail(proc))

    empty = sb.tmp / "responses-no-checks"
    shutil.copytree(sb.repo / RESPONSES, empty)
    write(empty / "check-runs.1.json", '{"total_count": 0, "check_runs": []}\n')
    write(empty / "status.1.json", '{"state": "pending", "total_count": 0, "statuses": []}\n')
    proc, facts = assemble(sb, replace={"--responses": str(empty)})
    T.check("assemble: no checks give an empty list", proc.returncode == 0 and facts.get("checks") == [],
            detail(proc))

    floor_record = {"tool": "mintswarm-plan-floor", "version": 1, "verdict": "hit"}
    floor_data = write(sb.tmp / "floor.json", json.dumps(floor_record) + "\n")
    assessment = write(sb.tmp / "assessment.md", "# Assessment: x\n\nmedium risk: y\n\nRisk: medium\n")
    proc, facts = assemble(sb, ["--kickoff-floor", str(sb.tmp / "floor.json"),
                                "--assessment", str(sb.tmp / "assessment.md")])
    ok = proc.returncode == 0 and list(facts["approval"]) == [
        "approver", "approved_at", "approval_ref", "kickoff_floor", "assessment"]
    if ok:
        ok = facts["approval"]["kickoff_floor"] == {
            "path": str(sb.tmp / "floor.json"), "sha256": sha(floor_data), "record": floor_record,
        } and facts["approval"]["assessment"] == {
            "path": str(sb.tmp / "assessment.md"), "sha256": sha(assessment), "risk": "medium",
        }
    T.check("assemble: the approval with kickoff floor and assessment", ok, detail(proc))
    write(sb.tmp / "assessment-2.md", "# Assessment: x\n\nRisk: low\nRisk: high\n")
    proc, facts = assemble(sb, ["--assessment", str(sb.tmp / "assessment-2.md")])
    T.check("assemble: an assessment with two Risk lines has risk null, and no kickoff floor key",
            proc.returncode == 0 and facts["approval"]["assessment"]["risk"] is None
            and "kickoff_floor" not in facts["approval"], detail(proc))

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
    cases = [
        ("a head that is not the one fetched", {"--head": "HEAD~1"}, []),
        ("a PR whose head is not the head", {"--responses": str(moved)}, []),
        ("a tampered kept file", {}, ["--rounds", str(tampered)]),
        ("a gap in round numbers", {}, ["--rounds", str(gap)]),
        ("an unreadable review verdict", {}, ["--rounds", str(verdict)]),
        ("a file not listed in round.json", {}, ["--rounds", str(stray)]),
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


def main() -> int:
    for tool in ("git", "yq", "bash"):
        if shutil.which(tool) is None:
            print(f"facts-selftest: {tool} is not on PATH", file=sys.stderr)
            return 2
    static_tests()
    with tempfile.TemporaryDirectory(prefix="msw-facts-selftest.") as tmp:
        sb = Sandbox(Path(tmp).resolve())
        keep_tests(sb)
        pages = build_responses(sb.head)
        if fetch_tests(sb, pages):
            assemble_tests(sb)
        else:
            print("facts-selftest: the fetch sandbox is not safe; assemble not run", file=sys.stderr)
            T.failed += 1
        for path in sb.rounds.rglob("*"):
            path.chmod(0o755 if path.is_dir() else 0o644)
    print(f"{T.passed} passed, {T.failed} failed")
    return 0 if T.failed == 0 else 1


if __name__ == "__main__":
    sys.exit(main())
