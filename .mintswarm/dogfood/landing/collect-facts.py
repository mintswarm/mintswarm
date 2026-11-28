#!/usr/bin/env python3
"""Collect a PR's landing facts: its GitHub state and the job's kept rounds.

    collect-facts.py fetch --pr N --issue N --head SHA [--out DIR]
    collect-facts.py assemble --responses DIR --pr N --issue N --head REF --base REF
                              --approver LOGIN --approved-at TIME
                              [--approval-url URL] [--approval-id ID] [--decision PATH]
                              [--kickoff-floor PATH] [--assessment PATH] --rounds DIR

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
DIR, the kept rounds (--rounds DIR, required, see keep-round.py) and the
session's arguments. It reads no network. Its first action is to delete any
stale facts file; the new one is written to a temporary file and renamed
onto it. Exits 0 when written and 2 on any unusable input, leaving no facts
file.

The kept rounds are read from the job's runner state, never from the
worktree: --rounds DIR must be an existing directory, absolute and not a
symbolic link, and neither DIR resolved nor any of its existing ancestors
may be, by identity (st_dev, st_ino), the working directory, the worktree
root or the main checkout, as keep-round.py requires (the same check, word
for word). A copy of the rounds under .mintswarm/job/, such as the landing
judge's inputs/rounds/, is refused. On one account this is a convention,
not an OS barrier: any process of the account can write DIR. Each round
records the head it judged, and a round whose `current` is false is not
passed, whatever its verdict; that catches a stale round kept honestly, at
an earlier commit. It does not catch an edit of DIR: `head` is a plain
field of round.json, which no sha256 covers, so whoever can edit DIR can
set it to the current head. Against that the only protection is keeping
DIR out of the stages' reach. A review round's head is the session's word,
which keep-round.py checks only for ancestry.

Inputs are checked before anything is written:

- --head and --base are resolved with
  `git rev-parse --verify --end-of-options REF^{commit}`; a REF starting
  with `-` is refused. The saved `pull` must have `number` PR and `head.sha`
  equal to the head, and request.json must match --pr, --issue and the head,
  so the checks belong to the head commit. The PR's `base.sha` is not used:
  the base is the job's.
- --approver is a GitHub login, ^[A-Za-z0-9-]+$, or an App's bot login,
  the same followed by `[bot]` (mintswarm-dev-bee[bot] for a plan approved
  without a human); --approved-at is a UTC time, YYYY-MM-DDTHH:MM:SSZ;
  --approval-url starts with https://; --approval-id is a positive integer;
  request.json's fetched_at is a UTC time of the same form.
- Every listing has pages 1 to k without gaps, every page but the last has
  100 items and the last fewer, so a truncated fetch is refused.
- Under --rounds, every entry not starting with `.` is a directory named
  gates-<n> or review-<n>; each kind is numbered 1 to k without gaps; each
  round.json is keep-round.py's version 2 (tool, version, kind and round
  matching the directory, `head` a full commit hash) and lists exactly the
  files present apart from itself, each with a matching sha256; every gates
  round holds gates.input.json and every review round review.md.

facts.json holds, with keys in this order (written with indent 2,
ensure_ascii off):

  tool           "mintswarm-landing-facts"
  version        3
  pr, issue      the numbers given
  head, base     full commit hashes
  fetched_at     when GitHub was read, from request.json: checks, steers
                 and comments after it are not in these facts
  checks         one {name, conclusion, source, app} per check on the head:
                 source "check-run" for check runs (the latest run per app
                 and name, app the run's app slug, or null when it has
                 none; conclusion null while the run is not completed) and
                 "status" for commit statuses (name is the context,
                 conclusion the state, app null); sorted by source, name
                 and app; [] when there are none
  gate_rounds    in round order, {round, commit, current, verdict,
                 failed_tests, reason, files}: commit the round.json
                 `head`, the commit the round judged, and current whether
                 it is the facts' head; a round whose current is false is
                 not passed, whatever its verdict. verdict is the round's
                 own, unchanged by current: "passed" only when the kept
                 gates.ctrf.json meets every condition of
                 run-gates.py --verdict against the kept gates.input.json,
                 in run-gates.py's order and with its wording, otherwise
                 "failed" with reason the first unmet condition (null when
                 passed). The conditions: the report parses and is CTRF,
                 has results without results.errors and with a summary and
                 a tests list, integer summary counts; the input is usable
                 (a non-empty `gates` list, each with a name and command);
                 summary.tests equals the input's gate count (at least one)
                 and the tests held; the tests name the input's gates in
                 order; all passed with none failed or other; every test
                 passed. Staleness, run-gates.py's remaining condition, is
                 checked by keep-round.py when the round is kept.
                 failed_tests are the names of tests with status failed or
                 other; files the {name, sha256} list of round.json
  review_rounds  in round order, {round, commit, current, verdict,
                 findings, files}: commit and current as for gate rounds,
                 so a review round whose current is false is not passed
                 either; verdict the first non-blank line after the one `## Verdict`
                 line outside code fences, "pass" or "changes_required";
                 findings the counts {blocker, major, minor, nit}, always
                 all four, from the one fenced yaml block whose first line
                 starts with `findings:`, parsed with yq -p=yaml -o=json;
                 the title must be `# Review round <n>:`
  steers         sorted by time, kind and url, {kind, author, time,
                 edited_at, url, via}; see below
  review_requests
                 the logins in the saved pull's `requested_reviewers`
                 that also have an APPROVED or CHANGES_REQUESTED review
                 in `reviews`, deduplicated and sorted, [] when there
                 are none: review re-requested and not yet given, so
                 their earlier review is not current (GitHub drops a
                 user from the list when they submit a review); a first
                 request is not listed. Team requests are not read. A
                 missing field, one that is not a list, or an entry
                 without a string `login` is unusable input
  approval       {approver, approved_at, approval_ref (the URL, else the
                 id, else null)}, then `decision` {path, sha256, record}
                 only when --decision is given (the record is decision.py's
                 decision.json, a JSON object with tool
                 "mintswarm-approval-decision"), then `kickoff_floor`
                 {path, sha256, record} only when --kickoff-floor is given
                 (the record is floor.py's verdict, a JSON object with tool
                 "mintswarm-plan-floor"), then `assessment` {path, sha256,
                 confidence, band} only when --assessment is given
                 (confidence, 0 to 100, and band, the band's identifier,
                 from the document's envelope as plan-approval/decision.py's
                 envelope_problems reads it, both null when it reports any
                 problem; decision.py is loaded by path, with its floor.py,
                 and a load failure is unusable input)

Steers. Items come from issue-comments (kind issue-comment), pr-comments
(pr-comment), reviews (review) and review-comments (review-comment). The
time is `created_at`, or `submitted_at` for a review; reviews in state
PENDING or without submitted_at are skipped. An item counts when its time
is strictly after --approved-at, or when it is a comment whose
`updated_at` is: a comment edited after the approval may have been turned
into a steer. `edited_at` is that `updated_at` when it is after the
approval and differs from the time, otherwise null; reviews have no
`updated_at` in the API, so an edited review body is not seen. The
approval itself never counts: the item whose html_url equals
--approval-url, or the issue-comment or pr-comment whose id equals
--approval-id (the other kinds have their own id sequences). Such an item
is a steer when:

- its author is not a bot (user.type is not Bot and the login does not end
  in [bot]; a deleted user is login `ghost`): via "account";
- or its author is mintswarm-dev-bee[bot] and some line of its body, split at
  LF and trimmed of whitespace, is exactly `<!-- mintswarm: steer -->`:
  via "marker". The session puts this marker on every decision it posts on
  the maintainer's behalf, as .mintswarm/dogfood/plan-approval/README.md,
  section "Decisions while a job is in flight", says. Lines inside code
  fences count; a marker in the middle of a line does not.

Other bot items are not steers. Steers given only in the session chat, or
relayed to GitHub without the marker, are not seen.

Standard library only, apart from running bot-gh.sh (fetch), git and yq
(assemble).
"""

import hashlib
import importlib.util
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
DECISION = HERE.parent / "plan-approval" / "decision.py"
FACTS = Path(".mintswarm/job/inputs/landing/facts.json")
RESPONSES = Path(".mintswarm/job/landing/responses")
TOOL = "mintswarm-landing-facts"
VERSION = 3
REVIEWED = ("APPROVED", "CHANGES_REQUESTED")  # review states a re-request makes not current
KEEP_TOOL = "mintswarm-keep-round"
KEEP_VERSION = 2
FLOOR_TOOL = "mintswarm-plan-floor"
DECISION_TOOL = "mintswarm-approval-decision"
REPO = "repos/{owner}/{repo}"
PER_PAGE = 100
MAX_PAGES = 30
BOT = "mintswarm-dev-bee[bot]"
MARKER = "<!-- mintswarm: steer -->"
SEVERITIES = ("blocker", "major", "minor", "nit")
REVIEW_VERDICTS = ("pass", "changes_required")
TIME_FORMAT = "%Y-%m-%dT%H:%M:%SZ"
TIME = re.compile(r"[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}Z")
LOGIN = re.compile(r"[A-Za-z0-9-]+(?:\[bot\])?")
NUMBER = re.compile(r"[1-9][0-9]*")
FULL_HASH = re.compile(r"[0-9a-f]{40}|[0-9a-f]{64}")
ROUND_DIR = re.compile(r"(gates|review)-([1-9][0-9]*)")
GATES_INPUT = "gates.input.json"
ROUND_FILES = {"gates": ("gates.ctrf.json", GATES_INPUT, "gates.md"), "review": ("address.md", "review.md")}
REQUIRED_FILES = {"gates": GATES_INPUT, "review": "review.md"}
ROUND_KEYS = ["tool", "version", "kind", "round", "head", "kept_at", "files"]
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
# the kinds an --approval-id can belong to: the approval is a comment
APPROVAL_ID_KINDS = ("issue-comment", "pr-comment")
STEER_SOURCES = (
    ("issue-comments", "issue-comment"),
    ("pr-comments", "pr-comment"),
    ("reviews", "review"),
    ("review-comments", "review-comment"),
)
FETCH_OPTIONS = {"--pr": True, "--issue": True, "--head": True, "--out": False}
ASSEMBLE_OPTIONS = {
    "--responses": True,
    "--pr": True,
    "--issue": True,
    "--head": True,
    "--base": True,
    "--approver": True,
    "--approved-at": True,
    "--approval-url": False,
    "--approval-id": False,
    "--decision": False,
    "--kickoff-floor": False,
    "--assessment": False,
    "--rounds": True,
}
USAGE = (
    "usage: collect-facts.py fetch --pr N --issue N --head SHA [--out DIR]\n"
    "       collect-facts.py assemble --responses DIR --pr N --issue N --head REF --base REF "
    "--approver LOGIN --approved-at TIME [--approval-url URL] [--approval-id ID] "
    "[--decision PATH] [--kickoff-floor PATH] [--assessment PATH] --rounds DIR"
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


def check_request(responses: Responses, pr: int, issue: int, head: str) -> str:
    """Check the saved request and PR against the arguments; return the request's fetched_at."""
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
    fetched_at = request.get("fetched_at")
    parse_time(fetched_at, "request.json's fetched_at")
    return fetched_at


def collect_review_requests(responses: Responses) -> list:
    requested = responses.load("pull.1.json").get("requested_reviewers")
    if not isinstance(requested, list):
        raise InputError(f"the saved pull's requested_reviewers is not a list: {requested!r}")
    found = set()
    for reviewer in requested:
        if not isinstance(reviewer, dict) or not isinstance(reviewer.get("login"), str):
            raise InputError(f"a requested reviewer without a login: {reviewer!r}")
        found.add(reviewer["login"])
    reviewed = {author(r)[0] for r in responses.listing("reviews", None) if r.get("state") in REVIEWED}
    return sorted(found & reviewed)


def collect_checks(responses: Responses) -> list:
    latest = {}
    for run in responses.listing("check-runs", "check_runs"):
        if not isinstance(run, dict) or not isinstance(run.get("name"), str):
            raise InputError(f"a check run without a name: {run!r}")
        ident = run.get("id")
        if not isinstance(ident, int) or isinstance(ident, bool):
            raise InputError(f"check run {run['name']!r} has no integer id")
        conclusion = run.get("conclusion") if run.get("status") == "completed" else None
        app = run.get("app")
        if app is not None and not isinstance(app, dict):
            raise InputError(f"check run {run['name']!r} has an unreadable app")
        app_id = app.get("id") if app else None
        slug = app.get("slug") if app else None
        if slug is not None and not isinstance(slug, str):
            raise InputError(f"check run {run['name']!r} has an app slug that is not a string")
        key = (str(app_id), run["name"])
        if key not in latest or ident > latest[key][0]:
            latest[key] = (ident, run["name"], conclusion, slug)
    checks = [
        {"name": name, "conclusion": conclusion, "source": "check-run", "app": slug}
        for _, name, conclusion, slug in latest.values()
    ]
    for status in responses.listing("status", "statuses"):
        if (
            not isinstance(status, dict)
            or not isinstance(status.get("context"), str)
            or not isinstance(status.get("state"), str)
        ):
            raise InputError(f"a commit status without a context or state: {status!r}")
        checks.append({"name": status["context"], "conclusion": status["state"], "source": "status", "app": None})
    checks.sort(key=lambda c: (c["source"], c["name"], str(c["app"]), str(c["conclusion"])))
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
            url = item.get("html_url")
            if not isinstance(url, str):
                raise InputError(f"an item of {name} has no html_url")
            edited_at = None
            if kind == "review":
                if item.get("state") == "PENDING" or item.get("submitted_at") is None:
                    continue
                stamp = item["submitted_at"]
            else:
                stamp = item.get("created_at")
                updated = item.get("updated_at")
                if (
                    updated is not None
                    and updated != stamp
                    and parse_time(updated, f"the updated_at of {url}") > approved_at
                ):
                    edited_at = updated
            if parse_time(stamp, f"the time of {url}") <= approved_at and edited_at is None:
                continue
            if approval_url is not None and url == approval_url:
                continue
            if approval_id is not None and kind in APPROVAL_ID_KINDS and str(item.get("id")) == approval_id:
                continue
            login, is_bot = author(item)
            if not is_bot:
                via = "account"
            elif login == BOT and has_marker(item.get("body")):
                via = "marker"
            else:
                continue
            steers.append(
                {"kind": kind, "author": login, "time": stamp, "edited_at": edited_at, "url": url, "via": via}
            )
    steers.sort(key=lambda s: (s["time"], s["kind"], s["url"]))
    return steers


# --- assemble: kept rounds ------------------------------------------------


def load_round(path: Path, kind: str, number: int) -> dict:
    record = strict_json(read_bytes(path / "round.json"), str(path / "round.json"))
    if not isinstance(record, dict) or list(record) != ROUND_KEYS:
        raise InputError(f"{path}/round.json does not have the keys {ROUND_KEYS}")
    if (record["tool"], record["version"], record["kind"], record["round"]) != (KEEP_TOOL, KEEP_VERSION, kind, number):
        raise InputError(
            f"{path}/round.json is not keep-round.py's version {KEEP_VERSION} record " f"of {kind} round {number}"
        )
    if not isinstance(record["head"], str) or not FULL_HASH.fullmatch(record["head"]):
        raise InputError(f"{path}/round.json: head is not a full commit hash: {record['head']!r}")
    files = record["files"]
    if not isinstance(files, list):
        raise InputError(f"{path}/round.json: files is not a list")
    names = []
    for entry in files:
        if (
            not isinstance(entry, dict)
            or list(entry) != ["name", "sha256"]
            or entry["name"] not in ROUND_FILES[kind]
            or not isinstance(entry["sha256"], str)
        ):
            raise InputError(f"{path}/round.json: unexpected file entry {entry!r}")
        names.append(entry["name"])
    if names != sorted(set(names)):
        raise InputError(f"{path}/round.json: the files are repeated or not sorted")
    if REQUIRED_FILES[kind] not in names:
        raise InputError(f"{path} has no {REQUIRED_FILES[kind]}")
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
    return {"head": record["head"], "files": [dict(entry) for entry in files], "data": data}


def check_rounds_location(value: str) -> Path:
    """--rounds DIR: absolute, not a symbolic link, and inside neither the worktree nor the main checkout.

    The same text in keep-round.py and collect-facts.py. DIR resolved and each of its existing ancestors
    are compared by identity (st_dev, st_ino) with the working directory, the worktree root and the main
    checkout, so neither a case variant on a case-insensitive file system nor `..` or a symbolic link
    above DIR gets past it.
    """
    path = Path(value)
    if not path.is_absolute():
        raise InputError(f"--rounds must be an absolute path, got {value!r}")
    if path.is_symlink():
        raise InputError(f"--rounds {value} is a symbolic link")
    places = [("the worktree", Path.cwd())]
    for name, args in (
        ("the worktree", ["--show-toplevel"]),
        ("the main checkout", ["--path-format=absolute", "--git-common-dir"]),
    ):
        try:
            proc = subprocess.run(["git", "rev-parse", *args], stdin=subprocess.DEVNULL, capture_output=True)
        except OSError as exc:
            raise InputError(f"cannot run git: {exc}") from exc
        out = os.fsdecode(proc.stdout).strip()  # a path, kept exactly: no replacement characters
        if proc.returncode != 0 or not out:
            raise InputError(f"git rev-parse {args[-1]} failed: the working directory is not a git checkout")
        places.append((name, Path(out).parent if name == "the main checkout" else Path(out)))
    identities = {}
    for name, place in places:
        try:
            info = os.stat(place)
        except OSError as exc:
            raise InputError(f"cannot stat {name} {place}: {exc}") from exc
        identities.setdefault((info.st_dev, info.st_ino), (name, place))
    resolved = path.resolve()
    for candidate in (resolved, *resolved.parents):
        try:
            info = os.stat(candidate)
        except OSError:  # not there yet: an ancestor further up decides
            continue
        found = identities.get((info.st_dev, info.st_ino))
        if found:
            raise InputError(f"--rounds {value} is inside {found[0]} {found[1]}; kept rounds live in runner state")
    return path


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


def kept_gates(data: bytes) -> list:
    """Return the gates of a kept gates.input.json, as run-gates.py's load_gates reads them."""
    try:
        spec = json.loads(data.decode("utf-8"))
        gates = spec["gates"]
        if not isinstance(gates, list) or not gates:
            raise ValueError("no gates defined")
        for gate in gates:
            if not isinstance(gate, dict) or not gate.get("name") or not gate.get("command"):
                raise ValueError(f"gate without name or command: {gate!r}")
    except (UnicodeDecodeError, ValueError, KeyError, TypeError) as exc:
        raise ValueError(f"unusable {GATES_INPUT}: {exc}") from exc
    return gates


def ctrf_unmet(data, input_data: bytes):
    """Return (report or None, first unmet condition or None).

    Mirrors run-gates.py's unmet_condition in order and wording, with the
    kept file names; its staleness check is keep-round.py's.
    """
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
    try:
        gates = kept_gates(input_data)
    except ValueError as exc:
        return report, str(exc)
    if counts["tests"] != len(gates) or counts["tests"] <= 0:
        return report, f"summary.tests is {counts['tests']}, the input has {len(gates)} gates"
    if counts["tests"] != len(tests):
        return report, f"summary.tests is {counts['tests']}, results.tests holds {len(tests)}"
    names = [t.get("name") if isinstance(t, dict) else None for t in tests]
    expected = [g["name"] for g in gates]
    if names != expected:
        return report, f"results.tests names {names}, the input has {expected}"
    if counts["passed"] != counts["tests"]:
        return report, f"summary.passed is {counts['passed']} of {counts['tests']}"
    if counts["failed"] != 0 or counts["other"] != 0:
        return report, f"summary.failed is {counts['failed']}, summary.other is {counts['other']}"
    for test in tests:
        if test.get("status") != "passed":
            return report, f"test {test['name']} has status {test.get('status')!r}"
    return report, None


def gate_round(number: int, kept: dict, head: str) -> dict:
    report, reason = ctrf_unmet(kept["data"].get("gates.ctrf.json"), kept["data"][GATES_INPUT])
    failed = []
    results = report.get("results") if isinstance(report, dict) else None
    tests = results.get("tests") if isinstance(results, dict) else None
    if isinstance(tests, list):
        failed = [t.get("name") for t in tests if isinstance(t, dict) and t.get("status") in ("failed", "other")]
    return {
        "round": number,
        "commit": kept["head"],
        "current": kept["head"] == head,
        "verdict": "passed" if reason is None else "failed",
        "failed_tests": failed,
        "reason": reason,
        "files": kept["files"],
    }


def scan_fences(lines: list) -> tuple:
    """Return (the (index, line) pairs outside code fences, the fenced blocks as (info, lines))."""
    outside, blocks = [], []
    fence, info, content = None, "", []
    for index, line in enumerate(lines):
        if fence is None:
            match = FENCE_OPEN.match(line)
            # a backtick fence's info string has no backticks (CommonMark)
            if match and (match.group(1)[0] == "~" or "`" not in line[match.end(1) :]):
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


def review_round(number: int, kept: dict, where: str, head: str) -> dict:
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
    verdict = next((line.strip() for line in lines[headings[0] + 1 :] if line.strip()), None)
    if verdict not in REVIEW_VERDICTS:
        raise InputError(f"{where}: the verdict {verdict!r} is not pass or changes_required")
    found = [
        content for info, content in blocks if info.lower() == "yaml" and content and content[0].startswith("findings:")
    ]
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
    return {
        "round": number,
        "commit": kept["head"],
        "current": kept["head"] == head,
        "verdict": verdict,
        "findings": counts,
        "files": kept["files"],
    }


# --- assemble: approval ---------------------------------------------------


def load_decision():
    """plan-approval/decision.py, loaded by path, with its floor set."""
    sys.dont_write_bytecode = True  # leave no __pycache__ next to decision.py
    try:
        spec = importlib.util.spec_from_file_location("msw_approval_decision", DECISION)
        if spec is None or spec.loader is None:
            raise InputError(f"cannot load {DECISION}")
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        module.floor = module.load_floor()
    except InputError:
        raise
    except Exception as exc:
        raise InputError(f"cannot load {DECISION}: {type(exc).__name__}") from exc
    return module


def assessment_confidence(data: bytes) -> tuple:
    """(confidence, band) from an assessment's envelope, or (None, None) when it is unusable."""
    decision = load_decision()
    try:
        text = data.decode("utf-8")
    except UnicodeDecodeError:
        return None, None
    problems, envelope = decision.envelope_problems(text)
    if problems:
        return None, None
    return envelope["confidence"], envelope["band"]


def build_approval(opts: dict) -> dict:
    approver, approved_at = opts["--approver"], opts["--approved-at"]
    if not LOGIN.fullmatch(approver):
        raise InputError(f"--approver is not a GitHub login or an App's bot login: {approver!r}")
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
    for option, key, tool, script in (
        ("--decision", "decision", DECISION_TOOL, "decision.py"),
        ("--kickoff-floor", "kickoff_floor", FLOOR_TOOL, "floor.py"),
    ):
        if option not in opts:
            continue
        path = opts[option]
        data = read_bytes(Path(path))
        record = strict_json(data, path)
        if not isinstance(record, dict) or record.get("tool") != tool:
            raise InputError(f"{path} is not a {script} record (tool {tool})")
        approval[key] = {"path": path, "sha256": sha256(data), "record": record}
    if "--assessment" in opts:
        path = opts["--assessment"]
        data = read_bytes(Path(path))
        confidence, band = assessment_confidence(data)
        approval["assessment"] = {"path": path, "sha256": sha256(data), "confidence": confidence, "band": band}
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
    fetched_at = check_request(responses, pr, issue, head)
    checks = collect_checks(responses)
    rounds_dir = check_rounds_location(opts["--rounds"])
    kept = load_rounds(rounds_dir)
    gate_rounds = [gate_round(n, kept["gates"][n], head) for n in sorted(kept["gates"])]
    review_rounds = [
        review_round(n, kept["review"][n], str(rounds_dir / f"review-{n}" / "review.md"), head)
        for n in sorted(kept["review"])
    ]
    steers = collect_steers(responses, approved_at, opts.get("--approval-url"), opts.get("--approval-id"))
    review_requests = collect_review_requests(responses)
    facts = {
        "tool": TOOL,
        "version": VERSION,
        "pr": pr,
        "issue": issue,
        "head": head,
        "base": base,
        "fetched_at": fetched_at,
        "checks": checks,
        "gate_rounds": gate_rounds,
        "review_rounds": review_rounds,
        "steers": steers,
        "review_requests": review_requests,
        "approval": approval,
    }
    write_facts(facts)
    print(
        f"facts: {len(checks)} checks, {len(gate_rounds)} gate rounds, "
        f"{len(review_rounds)} review rounds, {len(steers)} steers, "
        f"{len(review_requests)} pending review requests in {FACTS}"
    )
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
