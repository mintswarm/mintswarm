#!/usr/bin/env python3
"""Collect a PR's landing facts: its GitHub state and the job's kept rounds.

    collect-facts.py fetch --pr N --issue N --head SHA [--out DIR]
    collect-facts.py assemble --responses DIR --pr N --issue N --head REF --base REF
                              --approver LOGIN --approved-at TIME
                              [--approval-url URL] [--approval-id ID]
                              [--kickoff-floor PATH] [--assessment PATH] [--rounds DIR]

Dogfooding stand-in for the control plane's landing facts, read by the
landing floor and the landing judge. Judging the facts is theirs; this
script only records them. Paths are relative to the working directory, which
is the worktree root. Every option is given once, as `--name value`;
anything else is a bad argument. N is a positive integer.

fetch
-----

Reads GitHub through .mintswarm/dogfood/github/bot-gh.sh (resolved from this
script's directory), and only with GET: every call is exactly
`bot-gh.sh api --method GET PATH`, with the query in PATH, and never
-f, -F, --field, --raw-field, --input or --paginate. The script reads and
prints no token. PATH is one of, under repos/{owner}/{repo}/ (gh fills the
placeholders):

  pull             pulls/PR                                   one page
  check-runs       commits/SHA/check-runs?filter=latest&...   {"check_runs": [...]}
  status           commits/SHA/status?...                     {"statuses": [...]}
  pr-comments      issues/PR/comments?...                     [...]
  reviews          pulls/PR/reviews?...                       [...]
  review-comments  pulls/PR/comments?...                      [...]
  issue-comments   issues/ISSUE/comments?...                  [...]

Listings are paged by hand with `per_page=100&page=K`, K from 1, stopping
at the first page with fewer than 100 items; more than 30 pages is a
failure. Each response is saved byte for byte as DIR/<name>.<K>.json, and
DIR/request.json records `pr`, `issue`, `head` and `fetched_at`. Everything
is written to DIR.tmp-<random> and renamed onto DIR only when every page was
fetched; an earlier DIR is replaced then. DIR defaults to
.mintswarm/job/landing/responses. SHA is a full commit hash. Exits 0 when
saved, 1 when bot-gh.sh fails, a response is not JSON or not of the shape
above, there are too many pages, or the pages cannot be saved, and 2 on bad
arguments; DIR is unchanged unless it exits 0.

assemble
--------

Builds .mintswarm/job/inputs/landing/facts.json from the saved responses in
DIR, the kept rounds (--rounds, default .mintswarm/job/rounds, see
keep-round.py) and the session's arguments. It reads no network. Its first
action is to delete any stale facts file; the new one is written to a
temporary file and renamed onto it. Exits 0 when written and 2 on any
unusable input, leaving no facts file.

Inputs are checked before anything is written:

- --head and --base are resolved with
  `git rev-parse --verify --end-of-options REF^{commit}`; a REF starting
  with `-` is refused. The saved `pull` must have `number` PR and `head.sha`
  equal to the head, and request.json must match --pr, --issue and the head,
  so the checks belong to the head commit. The PR's `base.sha` is not used:
  the base is the job's.
- --approver matches ^[A-Za-z0-9-]+$; --approved-at is a UTC time,
  YYYY-MM-DDTHH:MM:SSZ; --approval-url starts with https://; --approval-id
  is a positive integer.
- Every listing has pages 1 to k without gaps, every page but the last has
  100 items and the last fewer, so a truncated fetch is refused.
- Under --rounds, every entry not starting with `.` is a directory named
  gates-<n> or review-<n>; each kind is numbered 1 to k without gaps; each
  round.json is keep-round.py's (tool, version, kind and round matching the
  directory) and lists exactly the files present apart from itself, each
  with a matching sha256.

facts.json holds, with keys in this order (written with indent 2,
ensure_ascii off):

  tool           "mintswarm-landing-facts"
  version        1
  pr, issue      the numbers given
  head, base     full commit hashes
  checks         one {name, conclusion, source} per check on the head:
                 source "check-run" for check runs (the latest run per
                 name; conclusion null while the run is not completed) and
                 "status" for commit statuses (name is the context,
                 conclusion the state); sorted by source, then name; []
                 when there are none
  gate_rounds    in round order, {round, verdict, failed_tests, reason,
                 files}: verdict "passed" only when the kept
                 gates.ctrf.json meets every condition of
                 run-gates.py --verdict that needs no inputs/gates.json
                 (CTRF, no results.errors, integer summary counts, at least
                 one test, summary.tests equal to the tests held, all
                 passed with none failed or other, every test passed),
                 otherwise "failed" with reason the first unmet condition
                 (null when passed); failed_tests the names of tests with
                 status failed or other; files the {name, sha256} list of
                 round.json
  review_rounds  in round order, {round, verdict, findings, files}:
                 verdict the first non-blank line after the one `## Verdict`
                 line outside code fences, "pass" or "changes_required";
                 findings the counts {blocker, major, minor, nit}, always
                 all four, from the one fenced yaml block whose first line
                 starts with `findings:`, parsed with yq -p=yaml -o=json;
                 the title must be `# Review round <n>:`
  steers         sorted by time, kind and url, {kind, author, time, url,
                 via}; see below
  approval       {approver, approved_at, approval_ref (the URL, else the
                 id, else null)}, then `kickoff_floor` {path, sha256,
                 record} only when --kickoff-floor is given (the record is
                 floor.py's verdict, a JSON object with tool
                 "mintswarm-plan-floor"), then `assessment` {path, sha256,
                 risk} only when --assessment is given (risk is low, medium
                 or high from the document's single `Risk:` line, or null
                 when the document breaks the assess stage's rules for it)

Steers. Items come from issue-comments (kind issue-comment), pr-comments
(pr-comment), reviews (review) and review-comments (review-comment). The
time is `created_at`, or `submitted_at` for a review; reviews in state
PENDING or without submitted_at are skipped. Only items strictly after
--approved-at count, other than the approval itself: the item whose
html_url equals --approval-url or whose id equals --approval-id. Such an
item is a steer when:

- its author is not a bot (user.type is not Bot and the login does not end
  in [bot]; a deleted user is login `ghost`): via "account";
- or its author is mintswarm-dev[bot] and some line of its body, split at
  LF and trimmed of whitespace, is exactly `<!-- mintswarm: steer -->`:
  via "marker". The session puts this marker on every comment it posts on
  the maintainer's behalf. Lines inside code fences count; a marker in the
  middle of a line does not.

Other bot items are not steers. Steers given only in the session chat, or
relayed to GitHub without the marker, are not seen.

Standard library only, apart from running bot-gh.sh (fetch), git and yq
(assemble).
"""

import hashlib
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
from datetime import datetime, timezone
from pathlib import Path

HERE = Path(__file__).resolve().parent
BOT_GH = HERE.parent / "github" / "bot-gh.sh"
FACTS = Path(".mintswarm/job/inputs/landing/facts.json")
RESPONSES = Path(".mintswarm/job/landing/responses")
ROUNDS = Path(".mintswarm/job/rounds")
TOOL = "mintswarm-landing-facts"
VERSION = 1
KEEP_TOOL = "mintswarm-keep-round"
FLOOR_TOOL = "mintswarm-plan-floor"
REPO = "repos/{owner}/{repo}"
PER_PAGE = 100
MAX_PAGES = 30
BOT = "mintswarm-dev[bot]"
MARKER = "<!-- mintswarm: steer -->"
SEVERITIES = ("blocker", "major", "minor", "nit")
REVIEW_VERDICTS = ("pass", "changes_required")
RISKS = ("low", "medium", "high")
TIME_FORMAT = "%Y-%m-%dT%H:%M:%SZ"
TIME = re.compile(r"[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}Z")
LOGIN = re.compile(r"[A-Za-z0-9-]+")
NUMBER = re.compile(r"[1-9][0-9]*")
FULL_HASH = re.compile(r"[0-9a-f]{40}|[0-9a-f]{64}")
ROUND_DIR = re.compile(r"(gates|review)-([1-9][0-9]*)")
ROUND_FILES = {"gates": ("gates.ctrf.json", "gates.md"), "review": ("address.md", "review.md")}
ROUND_KEYS = ["tool", "version", "kind", "round", "kept_at", "files"]
FENCE_OPEN = re.compile(r" {0,3}(`{3,}|~{3,})[ \t]*([^`\s]*)")

# name, path after repos/{owner}/{repo}/ (without per_page and page), items key
LISTINGS = (
    ("check-runs", "commits/{head}/check-runs?filter=latest&", "check_runs"),
    ("status", "commits/{head}/status?", "statuses"),
    ("pr-comments", "issues/{pr}/comments?", None),
    ("reviews", "pulls/{pr}/reviews?", None),
    ("review-comments", "pulls/{pr}/comments?", None),
    ("issue-comments", "issues/{issue}/comments?", None),
)
STEER_SOURCES = (
    ("issue-comments", "issue-comment"),
    ("pr-comments", "pr-comment"),
    ("reviews", "review"),
    ("review-comments", "review-comment"),
)
FETCH_OPTIONS = {"--pr": True, "--issue": True, "--head": True, "--out": False}
ASSEMBLE_OPTIONS = {
    "--responses": True, "--pr": True, "--issue": True, "--head": True, "--base": True,
    "--approver": True, "--approved-at": True, "--approval-url": False, "--approval-id": False,
    "--kickoff-floor": False, "--assessment": False, "--rounds": False,
}
USAGE = (
    "usage: collect-facts.py fetch --pr N --issue N --head SHA [--out DIR]\n"
    "       collect-facts.py assemble --responses DIR --pr N --issue N --head REF --base REF "
    "--approver LOGIN --approved-at TIME [--approval-url URL] [--approval-id ID] "
    "[--kickoff-floor PATH] [--assessment PATH] [--rounds DIR]"
)


class InputError(Exception):
    """Bad arguments or unusable input: exit 2."""


class FetchError(Exception):
    """A GitHub read failed or could not be saved: exit 1."""


# --- common ---------------------------------------------------------------

def parse_options(argv: list, spec: dict) -> dict:
    values = {}
    i = 0
    while i < len(argv):
        name = argv[i]
        if name not in spec:
            raise InputError(f"unknown argument {name!r}\n{USAGE}")
        if name in values:
            raise InputError(f"{name} is given twice")
        if i + 1 >= len(argv):
            raise InputError(f"{name} needs a value")
        values[name] = argv[i + 1]
        i += 2
    missing = [name for name, required in spec.items() if required and name not in values]
    if missing:
        raise InputError(f"missing {', '.join(missing)}\n{USAGE}")
    return values


def positive(value: str, name: str) -> int:
    if not NUMBER.fullmatch(value):
        raise InputError(f"{name} must be a positive integer, got {value!r}")
    return int(value)


def parse_time(value, where: str) -> datetime:
    if not isinstance(value, str) or not TIME.fullmatch(value):
        raise InputError(f"{where} is not a UTC time YYYY-MM-DDTHH:MM:SSZ: {value!r}")
    try:
        return datetime.strptime(value, TIME_FORMAT).replace(tzinfo=timezone.utc)
    except ValueError as exc:
        raise InputError(f"{where} is not a valid time: {value!r}") from exc


def strict_json(data: bytes, where: str):
    def no_duplicates(pairs):
        result = {}
        for key, value in pairs:
            if key in result:
                raise ValueError(f"duplicate key {key!r}")
            result[key] = value
        return result

    def no_constant(name):
        raise ValueError(f"{name} is not JSON")

    try:
        return json.loads(data.decode("utf-8"), object_pairs_hook=no_duplicates, parse_constant=no_constant)
    except (UnicodeDecodeError, ValueError) as exc:
        raise InputError(f"{where} is not JSON: {exc}") from exc


def read_bytes(path: Path) -> bytes:
    if path.is_symlink():
        raise InputError(f"{path} is a symbolic link")
    try:
        return path.read_bytes()
    except OSError as exc:
        raise InputError(f"cannot read {path}: {exc}") from exc


def sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def remove(path) -> None:
    try:
        os.unlink(path)
    except FileNotFoundError:
        pass


def page_items(data, key):
    """Return a page's items, or None when the page is not of the expected shape."""
    if key is None:
        return data if isinstance(data, list) else None
    if isinstance(data, dict) and isinstance(data.get(key), list):
        return data[key]
    return None


# --- fetch ----------------------------------------------------------------

def gh_get(path: str) -> tuple:
    argv = [str(BOT_GH), "api", "--method", "GET", path]
    try:
        proc = subprocess.run(argv, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE)
    except OSError as exc:
        raise FetchError(f"cannot run {BOT_GH}: {exc}") from exc
    if proc.returncode != 0:
        raise FetchError(f"bot-gh.sh api --method GET {path} exited {proc.returncode}")
    try:
        data = json.loads(proc.stdout.decode("utf-8"))
    except (UnicodeDecodeError, ValueError) as exc:
        raise FetchError(f"GET {path}: the response is not JSON: {exc}") from exc
    return proc.stdout, data


def save(directory: Path, name: str, data: bytes) -> None:
    try:
        with open(directory / name, "xb") as out:
            out.write(data)
    except OSError as exc:
        raise FetchError(f"cannot save {name}: {exc}") from exc


def fetch_into(tmp: Path, pr: int, issue: int, head: str) -> None:
    raw, data = gh_get(f"{REPO}/pulls/{pr}")
    if not isinstance(data, dict):
        raise FetchError(f"GET pulls/{pr}: the response is not an object")
    save(tmp, "pull.1.json", raw)
    for name, template, key in LISTINGS:
        prefix = f"{REPO}/" + template.format(pr=pr, issue=issue, head=head)
        for page in range(1, MAX_PAGES + 1):
            raw, data = gh_get(f"{prefix}per_page={PER_PAGE}&page={page}")
            items = page_items(data, key)
            if items is None:
                shape = f"an object with a {key} list" if key else "a list"
                raise FetchError(f"{name} page {page}: the response is not {shape}")
            save(tmp, f"{name}.{page}.json", raw)
            if len(items) < PER_PAGE:
                break
        else:
            raise FetchError(f"{name}: more than {MAX_PAGES} pages")
    request = {
        "pr": pr,
        "issue": issue,
        "head": head,
        "fetched_at": datetime.now(timezone.utc).strftime(TIME_FORMAT),
    }
    save(tmp, "request.json", (json.dumps(request, indent=2) + "\n").encode("utf-8"))


def replace_dir(tmp: Path, out: Path) -> None:
    try:
        if not os.path.lexists(out):
            os.rename(tmp, out)
            return
        if out.is_symlink() or not out.is_dir():
            raise FetchError(f"{out} exists and is not a directory")
        old = Path(tempfile.mkdtemp(prefix=f"{out.name}.old-", dir=out.parent))
        os.rename(out, old / "previous")
        try:
            os.rename(tmp, out)
        except OSError:
            os.rename(old / "previous", out)
            raise
        shutil.rmtree(old, ignore_errors=True)
    except OSError as exc:
        raise FetchError(f"cannot move the responses onto {out}: {exc}") from exc


def fetch(argv: list) -> int:
    opts = parse_options(argv, FETCH_OPTIONS)
    pr = positive(opts["--pr"], "--pr")
    issue = positive(opts["--issue"], "--issue")
    head = opts["--head"]
    if not FULL_HASH.fullmatch(head):
        raise InputError(f"--head must be a full commit hash, got {head!r}")
    out = Path(opts.get("--out", str(RESPONSES)))
    if out.name in ("", ".", ".."):
        raise InputError(f"--out must name a directory: {out}")
    try:
        out.parent.mkdir(parents=True, exist_ok=True)
        tmp = Path(tempfile.mkdtemp(prefix=f"{out.name}.tmp-", dir=out.parent))
    except OSError as exc:
        raise FetchError(f"cannot create a temporary directory next to {out}: {exc}") from exc
    try:
        fetch_into(tmp, pr, issue, head)
        os.chmod(tmp, 0o755)
        replace_dir(tmp, out)
    except BaseException:
        shutil.rmtree(tmp, ignore_errors=True)
        raise
    print(f"fetched {len(os.listdir(out)) - 1} pages into {out}")
    return 0


# --- assemble: responses --------------------------------------------------

class Responses:
    def __init__(self, directory: Path):
        self.dir = directory
        try:
            self.entries = set(os.listdir(directory))
        except OSError as exc:
            raise InputError(f"cannot list the responses {directory}: {exc}") from exc

    def load(self, name: str):
        if name not in self.entries:
            raise InputError(f"{self.dir / name} is missing")
        return strict_json(read_bytes(self.dir / name), str(self.dir / name))

    def listing(self, name: str, key) -> list:
        pattern = re.compile(re.escape(name) + r"\.([1-9][0-9]*)\.json")
        pages = sorted(int(m.group(1)) for m in map(pattern.fullmatch, self.entries) if m)
        if not pages or pages != list(range(1, len(pages) + 1)):
            raise InputError(f"{self.dir}: the pages of {name} are not numbered 1 to k: {pages}")
        items = []
        for page in pages:
            data = page_items(self.load(f"{name}.{page}.json"), key)
            if data is None:
                raise InputError(f"{name} page {page} is not of the expected shape")
            last = page == len(pages)
            if not last and len(data) != PER_PAGE:
                raise InputError(f"{name} page {page} has {len(data)} items but is not the last")
            if last and len(data) >= PER_PAGE:
                raise InputError(f"{name} ends with a full page {page}: the listing is truncated")
            items.extend(data)
        return items


def check_request(responses: Responses, pr: int, issue: int, head: str) -> None:
    request = responses.load("request.json")
    if not isinstance(request, dict):
        raise InputError("request.json is not an object")
    for key, expected in (("pr", pr), ("issue", issue), ("head", head)):
        if request.get(key) != expected:
            raise InputError(f"request.json has {key} {request.get(key)!r}, expected {expected!r}")
    pull = responses.load("pull.1.json")
    if not isinstance(pull, dict) or pull.get("number") != pr:
        raise InputError(f"the saved pull is not PR {pr}")
    pull_head = pull.get("head")
    if not isinstance(pull_head, dict) or pull_head.get("sha") != head:
        sha = pull_head.get("sha") if isinstance(pull_head, dict) else None
        raise InputError(f"PR {pr}'s head is {sha!r}, not the head {head}")


def collect_checks(responses: Responses) -> list:
    latest = {}
    for run in responses.listing("check-runs", "check_runs"):
        if not isinstance(run, dict) or not isinstance(run.get("name"), str):
            raise InputError(f"a check run without a name: {run!r}")
        ident = run.get("id")
        if not isinstance(ident, int) or isinstance(ident, bool):
            raise InputError(f"check run {run['name']!r} has no integer id")
        conclusion = run.get("conclusion") if run.get("status") == "completed" else None
        if run["name"] not in latest or ident > latest[run["name"]][0]:
            latest[run["name"]] = (ident, conclusion)
    checks = [
        {"name": name, "conclusion": conclusion, "source": "check-run"}
        for name, (_, conclusion) in latest.items()
    ]
    for status in responses.listing("status", "statuses"):
        if (not isinstance(status, dict) or not isinstance(status.get("context"), str)
                or not isinstance(status.get("state"), str)):
            raise InputError(f"a commit status without a context or state: {status!r}")
        checks.append({"name": status["context"], "conclusion": status["state"], "source": "status"})
    checks.sort(key=lambda c: (c["source"], c["name"], str(c["conclusion"])))
    return checks


def author(item: dict) -> tuple:
    """Return (login, is_bot)."""
    user = item.get("user")
    if user is None:
        return "ghost", False
    if not isinstance(user, dict) or not isinstance(user.get("login"), str):
        raise InputError(f"an item with an unreadable user: {item.get('html_url')!r}")
    login = user["login"]
    return login, user.get("type") == "Bot" or login.endswith("[bot]")


def has_marker(body) -> bool:
    if body is None:
        return False
    if not isinstance(body, str):
        raise InputError("an item body is not a string")
    return any(line.strip() == MARKER for line in body.split("\n"))


def collect_steers(responses: Responses, approved_at: datetime, approval_url, approval_id) -> list:
    steers = []
    for name, kind in STEER_SOURCES:
        for item in responses.listing(name, None):
            if not isinstance(item, dict):
                raise InputError(f"an item of {name} is not an object")
            if kind == "review":
                if item.get("state") == "PENDING" or item.get("submitted_at") is None:
                    continue
                stamp = item["submitted_at"]
            else:
                stamp = item.get("created_at")
            url = item.get("html_url")
            if not isinstance(url, str):
                raise InputError(f"an item of {name} has no html_url")
            if parse_time(stamp, f"the time of {url}") <= approved_at:
                continue
            if approval_url is not None and url == approval_url:
                continue
            if approval_id is not None and str(item.get("id")) == approval_id:
                continue
            login, is_bot = author(item)
            if not is_bot:
                via = "account"
            elif login == BOT and has_marker(item.get("body")):
                via = "marker"
            else:
                continue
            steers.append({"kind": kind, "author": login, "time": stamp, "url": url, "via": via})
    steers.sort(key=lambda s: (s["time"], s["kind"], s["url"]))
    return steers


# --- assemble: kept rounds ------------------------------------------------

def load_round(path: Path, kind: str, number: int) -> dict:
    record = strict_json(read_bytes(path / "round.json"), str(path / "round.json"))
    if not isinstance(record, dict) or list(record) != ROUND_KEYS:
        raise InputError(f"{path}/round.json does not have the keys {ROUND_KEYS}")
    if (record["tool"], record["version"], record["kind"], record["round"]) != (KEEP_TOOL, 1, kind, number):
        raise InputError(f"{path}/round.json is not keep-round.py's record of {kind} round {number}")
    files = record["files"]
    if not isinstance(files, list):
        raise InputError(f"{path}/round.json: files is not a list")
    names = []
    for entry in files:
        if (not isinstance(entry, dict) or list(entry) != ["name", "sha256"]
                or entry["name"] not in ROUND_FILES[kind] or not isinstance(entry["sha256"], str)):
            raise InputError(f"{path}/round.json: unexpected file entry {entry!r}")
        names.append(entry["name"])
    if names != sorted(set(names)):
        raise InputError(f"{path}/round.json: the files are repeated or not sorted")
    if kind == "review" and "review.md" not in names:
        raise InputError(f"{path} has no review.md")
    try:
        present = set(os.listdir(path))
    except OSError as exc:
        raise InputError(f"cannot list {path}: {exc}") from exc
    if present != {"round.json", *names}:
        raise InputError(f"{path} holds {sorted(present)}, round.json lists {names}")
    data = {}
    for entry in files:
        content = read_bytes(path / entry["name"])
        if sha256(content) != entry["sha256"]:
            raise InputError(f"{path / entry['name']} does not match its sha256 in round.json")
        data[entry["name"]] = content
    return {"files": [dict(entry) for entry in files], "data": data}


def load_rounds(rounds: Path) -> dict:
    if rounds.is_symlink() or not rounds.is_dir():
        raise InputError(f"{rounds} is not a directory of kept rounds")
    found = {"gates": {}, "review": {}}
    try:
        entries = sorted(os.listdir(rounds))
    except OSError as exc:
        raise InputError(f"cannot list {rounds}: {exc}") from exc
    for entry in entries:
        if entry.startswith("."):
            continue
        match = ROUND_DIR.fullmatch(entry)
        path = rounds / entry
        if not match or path.is_symlink() or not path.is_dir():
            raise InputError(f"{path} is not a kept round")
        kind, number = match.group(1), int(match.group(2))
        found[kind][number] = load_round(path, kind, number)
    for kind, kept in found.items():
        if sorted(kept) != list(range(1, len(kept) + 1)):
            raise InputError(f"the kept {kind} rounds are not numbered 1 to k: {sorted(kept)}")
    return found


def ctrf_unmet(data):
    """Return (report or None, first unmet condition or None)."""
    if data is None:
        return None, "no report: gates.ctrf.json was not kept"
    try:
        report = json.loads(data.decode("utf-8"))
    except (UnicodeDecodeError, ValueError) as exc:
        return None, f"gates.ctrf.json does not parse: {exc}"
    if not isinstance(report, dict) or report.get("reportFormat") != "CTRF":
        return None, "reportFormat is not CTRF"
    results = report.get("results")
    if not isinstance(results, dict):
        return report, "the report has no results"
    if "errors" in results:
        return report, "results.errors is present"
    summary, tests = results.get("summary"), results.get("tests")
    if not isinstance(summary, dict) or not isinstance(tests, list):
        return report, "the report has no results.summary or results.tests"
    counts = {}
    for key in ("tests", "passed", "failed", "other"):
        value = summary.get(key)
        if not isinstance(value, int) or isinstance(value, bool):
            return report, f"summary.{key} is not an integer"
        counts[key] = value
    if counts["tests"] <= 0:
        return report, f"summary.tests is {counts['tests']}"
    if counts["tests"] != len(tests):
        return report, f"summary.tests is {counts['tests']}, results.tests holds {len(tests)}"
    if counts["passed"] != counts["tests"]:
        return report, f"summary.passed is {counts['passed']} of {counts['tests']}"
    if counts["failed"] != 0 or counts["other"] != 0:
        return report, f"summary.failed is {counts['failed']}, summary.other is {counts['other']}"
    for test in tests:
        if not isinstance(test, dict) or test.get("status") != "passed":
            name = test.get("name") if isinstance(test, dict) else None
            status = test.get("status") if isinstance(test, dict) else None
            return report, f"test {name} has status {status!r}"
    return report, None


def gate_round(number: int, kept: dict) -> dict:
    report, reason = ctrf_unmet(kept["data"].get("gates.ctrf.json"))
    failed = []
    try:
        tests = report["results"]["tests"]
    except (TypeError, KeyError):
        tests = []
    if isinstance(tests, list):
        failed = [t.get("name") for t in tests if isinstance(t, dict) and t.get("status") in ("failed", "other")]
    return {
        "round": number,
        "verdict": "passed" if reason is None else "failed",
        "failed_tests": failed,
        "reason": reason,
        "files": kept["files"],
    }


def scan_fences(lines: list) -> tuple:
    """Return (the (index, line) pairs outside code fences, the fenced blocks as (info, lines))."""
    outside, blocks = [], []
    fence = None
    for index, line in enumerate(lines):
        if fence is None:
            match = FENCE_OPEN.match(line)
            # a backtick fence's info string has no backticks (CommonMark)
            if match and (match.group(1)[0] == "~" or "`" not in line[match.end(1):]):
                fence, info, content = match.group(1), match.group(2), []
            else:
                outside.append((index, line))
        elif re.fullmatch(r" {0,3}" + re.escape(fence[0]) + "{" + str(len(fence)) + r",}[ \t]*", line):
            blocks.append((info, content))
            fence = None
        else:
            content.append(line)
    return outside, blocks


def yq_json(text: str, where: str):
    try:
        proc = subprocess.run(["yq", "-p=yaml", "-o=json", "."], input=text.encode("utf-8"), capture_output=True)
    except OSError as exc:
        raise InputError(f"cannot run yq: {exc}") from exc
    if proc.returncode != 0:
        detail = proc.stderr.decode("utf-8", errors="replace").strip()
        raise InputError(f"{where}: yq cannot read the findings block: {detail}")
    return strict_json(proc.stdout, f"{where}: yq's output")


def review_round(number: int, kept: dict, where: str) -> dict:
    try:
        text = kept["data"]["review.md"].decode("utf-8")
    except UnicodeDecodeError as exc:
        raise InputError(f"{where} is not UTF-8") from exc
    lines = text.replace("\r\n", "\n").split("\n")
    if not lines[0].startswith(f"# Review round {number}:"):
        raise InputError(f"{where} is not titled '# Review round {number}:'")
    outside, blocks = scan_fences(lines)
    headings = [index for index, line in outside if line.strip() == "## Verdict"]
    if len(headings) != 1:
        raise InputError(f"{where} has {len(headings)} '## Verdict' lines outside code fences, not one")
    verdict = next((line.strip() for line in lines[headings[0] + 1:] if line.strip()), None)
    if verdict not in REVIEW_VERDICTS:
        raise InputError(f"{where}: the verdict {verdict!r} is not pass or changes_required")
    found = [content for info, content in blocks
             if info.lower() == "yaml" and content and content[0].startswith("findings:")]
    if len(found) != 1:
        raise InputError(f"{where} has {len(found)} yaml blocks starting with 'findings:', not one")
    parsed = yq_json("\n".join(found[0]) + "\n", where)
    findings = parsed.get("findings") if isinstance(parsed, dict) else None
    if findings is None and isinstance(parsed, dict) and "findings" in parsed:
        findings = []
    if not isinstance(findings, list):
        raise InputError(f"{where}: findings is not a list")
    counts = {severity: 0 for severity in SEVERITIES}
    for finding in findings:
        severity = finding.get("severity") if isinstance(finding, dict) else None
        if severity not in counts:
            raise InputError(f"{where}: a finding with severity {severity!r}")
        counts[severity] += 1
    return {"round": number, "verdict": verdict, "findings": counts, "files": kept["files"]}


# --- assemble: approval ---------------------------------------------------

def assessment_risk(data: bytes):
    try:
        text = data.decode("utf-8")
    except UnicodeDecodeError:
        return None
    lines = [line for line in text.split("\n") if line.startswith("Risk:")]
    if len(lines) != 1:
        return None
    level = lines[0][len("Risk: "):] if lines[0].startswith("Risk: ") else None
    return level if level in RISKS else None


def build_approval(opts: dict) -> dict:
    approver, approved_at = opts["--approver"], opts["--approved-at"]
    if not LOGIN.fullmatch(approver):
        raise InputError(f"--approver is not a GitHub login: {approver!r}")
    url, ident = opts.get("--approval-url"), opts.get("--approval-id")
    if url is not None and (not url.startswith("https://") or any(c.isspace() for c in url)):
        raise InputError(f"--approval-url is not an https URL: {url!r}")
    if ident is not None:
        positive(ident, "--approval-id")
    approval = {
        "approver": approver,
        "approved_at": approved_at,
        "approval_ref": url if url is not None else ident,
    }
    if "--kickoff-floor" in opts:
        path = opts["--kickoff-floor"]
        data = read_bytes(Path(path))
        record = strict_json(data, path)
        if not isinstance(record, dict) or record.get("tool") != FLOOR_TOOL:
            raise InputError(f"{path} is not a floor.py verdict (tool {FLOOR_TOOL})")
        approval["kickoff_floor"] = {"path": path, "sha256": sha256(data), "record": record}
    if "--assessment" in opts:
        path = opts["--assessment"]
        data = read_bytes(Path(path))
        approval["assessment"] = {"path": path, "sha256": sha256(data), "risk": assessment_risk(data)}
    return approval


# --- assemble -------------------------------------------------------------

def resolve(ref: str, name: str) -> str:
    if ref.startswith("-"):
        raise InputError(f"{name} must not start with '-': {ref!r}")
    try:
        proc = subprocess.run(
            ["git", "rev-parse", "--verify", "--quiet", "--end-of-options", f"{ref}^{{commit}}"],
            capture_output=True,
        )
    except OSError as exc:
        raise InputError(f"cannot run git: {exc}") from exc
    commit = proc.stdout.decode("ascii", errors="replace").strip()
    if proc.returncode != 0 or not FULL_HASH.fullmatch(commit):
        raise InputError(f"{name} {ref!r} does not resolve to a commit")
    return commit


def write_facts(facts: dict) -> None:
    try:
        FACTS.parent.mkdir(parents=True, exist_ok=True)
        fd, tmp = tempfile.mkstemp(prefix=".facts.", suffix=".json", dir=FACTS.parent)
    except OSError as exc:
        raise InputError(f"cannot write {FACTS}: {exc}") from exc
    try:
        with os.fdopen(fd, "w", encoding="utf-8") as out:
            out.write(json.dumps(facts, indent=2, ensure_ascii=False) + "\n")
        os.replace(tmp, FACTS)
    except BaseException as exc:
        try:
            remove(tmp)
        except OSError:
            pass
        if isinstance(exc, OSError):
            raise InputError(f"cannot write {FACTS}: {exc}") from exc
        raise


def assemble(argv: list) -> int:
    try:
        remove(FACTS)
    except OSError as exc:
        raise InputError(f"cannot delete the stale {FACTS}: {exc}") from exc
    opts = parse_options(argv, ASSEMBLE_OPTIONS)
    pr = positive(opts["--pr"], "--pr")
    issue = positive(opts["--issue"], "--issue")
    approved_at = parse_time(opts["--approved-at"], "--approved-at")
    approval = build_approval(opts)
    head = resolve(opts["--head"], "--head")
    base = resolve(opts["--base"], "--base")
    responses = Responses(Path(opts["--responses"]))
    check_request(responses, pr, issue, head)
    checks = collect_checks(responses)
    rounds_dir = Path(opts.get("--rounds", str(ROUNDS)))
    kept = load_rounds(rounds_dir)
    gate_rounds = [gate_round(n, kept["gates"][n]) for n in sorted(kept["gates"])]
    review_rounds = [
        review_round(n, kept["review"][n], str(rounds_dir / f"review-{n}" / "review.md"))
        for n in sorted(kept["review"])
    ]
    steers = collect_steers(responses, approved_at, opts.get("--approval-url"), opts.get("--approval-id"))
    facts = {
        "tool": TOOL,
        "version": VERSION,
        "pr": pr,
        "issue": issue,
        "head": head,
        "base": base,
        "checks": checks,
        "gate_rounds": gate_rounds,
        "review_rounds": review_rounds,
        "steers": steers,
        "approval": approval,
    }
    write_facts(facts)
    print(f"facts: {len(checks)} checks, {len(gate_rounds)} gate rounds, "
          f"{len(review_rounds)} review rounds, {len(steers)} steers in {FACTS}")
    return 0


def main(argv: list) -> int:
    command = argv[0] if argv else None
    if command not in ("fetch", "assemble"):
        print(USAGE, file=sys.stderr)
        return 2
    try:
        return fetch(argv[1:]) if command == "fetch" else assemble(argv[1:])
    except FetchError as exc:
        print(f"collect-facts: {exc}", file=sys.stderr)
        return 1
    except InputError as exc:
        print(f"collect-facts: {exc}", file=sys.stderr)
        return 2
    except Exception as exc:  # a crash must never leave facts or read as fetched
        if command == "assemble":
            try:
                remove(FACTS)
            except Exception:
                pass
        message = " ".join(str(exc).split())
        print(f"collect-facts: unexpected error: {type(exc).__name__}: {message}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
