#!/usr/bin/env python3
"""Gather the maintainer decisions recorded on an issue's ancestors, for the stages that judge its criteria.

    ancestor-decisions.py fetch --issue N [--out DIR]
    ancestor-decisions.py assemble --responses DIR --issue N --base REF

Dogfooding stand-in for the control plane's ancestor decisions. A decision
recorded on a parent issue rereads its children's acceptance criteria; it is
never copied into a child. The stages that judge criteria (analyse, plan,
assess, review, implement and landing) read it from
`.mintswarm/job/inputs/ancestors.md`, which this script writes. The session
runs `fetch` then `assemble` in the job's worktree right before it
dispatches each of them (Session procedure, step 17, in this directory's
README). Paths are relative to the working directory, the worktree root.
Every option is given once, as `--name value`; anything else is a bad
argument. N is a positive integer.

fetch
-----

Reads GitHub through bot-gh.sh beside this script, and only with GET: every
call is exactly `bot-gh.sh api --method GET PATH`, with the query in PATH,
and never -f, -F, --field, --raw-field, --input or --paginate. PATH is one
of, under repos/{owner}/{repo}/ (gh fills the placeholders):

  issues/<n>/parent                      the sub-issue parent of <n>
  issues/<a>                             each ancestor
  issues/<a>/comments?per_page=100&page=K  its comments, page by page
  collaborators/<login>/permission       each person who filed an ancestor

It follows the parent chain from N: a 404 on the parent endpoint (`HTTP 404`
on gh's stderr, as assign-approvers.py's gh() detects it) ends the chain
normally. A revisited issue, or more than 10 ancestors, exits 1. Comment
pages stop at the first page with fewer than 100 items; more than 30 pages
exit 1. A 404 on a permission read is saved as `null`: no permission, as
watch-plan-approvals.py treats it. Every other response is saved byte for
byte, as parent-of-<n>.json, issue-<a>.json, comments-<a>.<K>.json and
permission-<login>.json (login in lower case), with request.json holding
`issue`, `ancestors` (the chain, nearest first) and `fetched_at`.
Everything goes to a temporary directory beside DIR, renamed onto DIR only
when every call succeeded, so a failure leaves DIR as it was. DIR defaults
to .mintswarm/job/ancestors/responses, outside inputs/.

assemble
--------

Reads no network. It first deletes any stale .mintswarm/job/inputs/ancestors.md,
then reads the saved responses and `tracker.ingest.trusted.authors` from
`git show REF:.mintswarm/config.yaml`, converted with `yq -p=yaml -o=json`;
a missing list counts as empty, and a config yq cannot read, or a list that
is not of strings, exits 2.

An ancestor's owner is its author when that author is a person (not user
type Bot and no login ending in `[bot]`); for a bot-filed ancestor it is the
owner of the next ancestor up, and none when the chain ends without a
person. The owner is trusted when the login matches an entry of
`tracker.ingest.trusted.authors` case-insensitively, or when the saved
permission is `admin` or `write`, or its role_name is `admin`, `maintain` or
`write`, as watch-plan-approvals.py's can_write() judges it. A 404 (`null`)
means no permission.

A decision on an ancestor is one of:

- a comment by a login that matches an entry of
  `tracker.ingest.trusted.authors` case-insensitively, or by that
  ancestor's owner when the owner is trusted (`via: account`);
- a mintswarm-dev[bot] comment holding the line `<!-- mintswarm: steer -->`
  (`via: marker`), tested as collect-facts.py tests it: CRLF becomes LF,
  each line is trimmed of whitespace, then compared;
- a section of the ancestor's body, when its owner is trusted, under an ATX
  heading whose text starts with `Decision`, found outside ``` and ~~~
  fenced blocks, up to the next heading of the same or a higher level
  (`via: body`).

Every other comment and section is left out; so is every comment and body
section of an untrusted owner, unless it is a trusted author's or a marker
steer.

The file has a `# Ancestor decisions: #N` title and a one-line summary
naming the chain, then one `## #<a> <title>` section per ancestor, nearest
first, with its URL, its owner and whether that owner is trusted. Each
decision follows, body sections first in body order, then comments by
created_at and id, with its kind, author, `via`, `time`, `edited` (the
comment's updated_at when it differs from created_at, else `none`; body
sections carry no time) and URL, and its text quoted with every line
prefixed by `> ` (CR and CRLF read as LF), so no quoted fence and no
`Risk:` line can reach column 0. An ancestor without decisions says
`No decisions.`; a root issue gives the line `#N has no ancestors.`. The
file is written to a temporary file and renamed.

Exits 0 when saved or written; 1 when fetch fails (a bot-gh.sh failure
other than the 404s above, a response that is not JSON or of the wrong
shape, a loop, too many ancestors or pages); 2 on bad arguments and, for
assemble, on any unusable input, leaving no file. Standard library only,
apart from running bot-gh.sh (fetch), git and yq (assemble).
"""

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
BOT_GH = HERE / "bot-gh.sh"
OUTPUT = Path(".mintswarm/job/inputs/ancestors.md")
RESPONSES = Path(".mintswarm/job/ancestors/responses")
REPO = "repos/{owner}/{repo}"
PER_PAGE = 100
MAX_PAGES = 30
MAX_ANCESTORS = 10
BOT = "mintswarm-dev[bot]"
MARKER = "<!-- mintswarm: steer -->"
WRITE_PERMISSIONS = {"admin", "write"}
WRITE_ROLES = {"admin", "maintain", "write"}
TIME_FORMAT = "%Y-%m-%dT%H:%M:%SZ"
NUMBER = re.compile(r"[1-9][0-9]*")
LOGIN = re.compile(r"[A-Za-z0-9-]+")
HEADING = re.compile(r" {0,3}(#{1,6})(?:[ \t]+(.*?))?[ \t]*$")
FENCE = re.compile(r" {0,3}(`{3,}|~{3,})")
FETCH_OPTIONS = {"--issue": True, "--out": False}
ASSEMBLE_OPTIONS = {"--responses": True, "--issue": True, "--base": True}
USAGE = (
    "usage: ancestor-decisions.py fetch --issue N [--out DIR]\n"
    "       ancestor-decisions.py assemble --responses DIR --issue N --base REF"
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


def person(user):
    """USER's login when a person, else None."""
    if not isinstance(user, dict):
        return None
    login = user.get("login")
    if not isinstance(login, str) or user.get("type") == "Bot" or login.lower().endswith("[bot]"):
        return None
    return login


def is_number(value) -> bool:
    return isinstance(value, int) and not isinstance(value, bool) and value > 0


def remove(path) -> None:
    try:
        os.unlink(path)
    except FileNotFoundError:
        pass


# --- fetch ----------------------------------------------------------------


def gh_get(path: str, missing_ok: bool = False):
    """GET PATH; (raw bytes, data), or None for a 404 when MISSING_OK."""
    argv = [str(BOT_GH), "api", "--method", "GET", path]
    try:
        proc = subprocess.run(argv, stdin=subprocess.DEVNULL, capture_output=True)
    except OSError as exc:
        raise FetchError(f"cannot run {BOT_GH}: {exc}") from exc
    if proc.returncode != 0:
        if missing_ok and b"HTTP 404" in proc.stderr:
            return None
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


def fetch_into(tmp: Path, issue: int) -> None:
    chain = []
    seen = {issue}
    current = issue
    while True:
        got = gh_get(f"{REPO}/issues/{current}/parent", missing_ok=True)
        if got is None:
            break
        raw, parent = got
        number = parent.get("number") if isinstance(parent, dict) else None
        if not is_number(number):
            raise FetchError(f"the parent of #{current} has no issue number")
        if number in seen:
            raise FetchError(f"the parent chain of #{issue} loops at #{number}")
        if len(chain) >= MAX_ANCESTORS:
            raise FetchError(f"#{issue} has more than {MAX_ANCESTORS} ancestors")
        save(tmp, f"parent-of-{current}.json", raw)
        seen.add(number)
        chain.append(number)
        current = number
    authors = {}
    for number in chain:
        raw, data = gh_get(f"{REPO}/issues/{number}")
        if not isinstance(data, dict) or data.get("number") != number:
            raise FetchError(f"GET issues/{number}: the response is not issue #{number}")
        save(tmp, f"issue-{number}.json", raw)
        login = person(data.get("user"))
        if login is not None:
            if not LOGIN.fullmatch(login):
                raise FetchError(f"#{number}'s author is not a valid login: {login!r}")
            authors.setdefault(login.lower(), login)
        for page in range(1, MAX_PAGES + 1):
            raw, data = gh_get(f"{REPO}/issues/{number}/comments?per_page={PER_PAGE}&page={page}")
            if not isinstance(data, list):
                raise FetchError(f"comments of #{number} page {page}: the response is not a list")
            save(tmp, f"comments-{number}.{page}.json", raw)
            if len(data) < PER_PAGE:
                break
        else:
            raise FetchError(f"comments of #{number}: more than {MAX_PAGES} pages")
    for lower, login in sorted(authors.items()):
        got = gh_get(f"{REPO}/collaborators/{login}/permission", missing_ok=True)
        if got is None:
            save(tmp, f"permission-{lower}.json", b"null\n")
            continue
        raw, data = got
        if not isinstance(data, dict):
            raise FetchError(f"the permission of {login} is not an object")
        save(tmp, f"permission-{lower}.json", raw)
    request = {
        "issue": issue,
        "ancestors": chain,
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
    issue = positive(opts["--issue"], "--issue")
    out = Path(opts.get("--out", str(RESPONSES)))
    if out.name in ("", ".", ".."):
        raise InputError(f"--out must name a directory: {out}")
    try:
        out.parent.mkdir(parents=True, exist_ok=True)
        tmp = Path(tempfile.mkdtemp(prefix=f"{out.name}.tmp-", dir=out.parent))
    except OSError as exc:
        raise FetchError(f"cannot create a temporary directory next to {out}: {exc}") from exc
    try:
        fetch_into(tmp, issue)
        os.chmod(tmp, 0o755)
        replace_dir(tmp, out)
    except BaseException:
        shutil.rmtree(tmp, ignore_errors=True)
        raise
    print(f"fetched the ancestors of #{issue} into {out}")
    return 0


# --- assemble -------------------------------------------------------------


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


class Responses:
    def __init__(self, directory: Path):
        self.dir = directory
        if directory.is_symlink() or not directory.is_dir():
            raise InputError(f"the responses {directory} are not a directory")
        try:
            self.entries = set(os.listdir(directory))
        except OSError as exc:
            raise InputError(f"cannot list the responses {directory}: {exc}") from exc

    def load(self, name: str):
        path = self.dir / name
        if name not in self.entries or path.is_symlink():
            raise InputError(f"{path} is missing or a symbolic link")
        try:
            return strict_json(path.read_bytes(), str(path))
        except OSError as exc:
            raise InputError(f"cannot read {path}: {exc}") from exc

    def comments(self, number: int) -> list:
        pattern = re.compile(rf"comments-{number}\.([1-9][0-9]*)\.json")
        pages = sorted(int(m.group(1)) for m in map(pattern.fullmatch, self.entries) if m)
        if not pages or pages != list(range(1, len(pages) + 1)):
            raise InputError(f"the comment pages of #{number} are not numbered 1 to k: {pages}")
        items = []
        for page in pages:
            data = self.load(f"comments-{number}.{page}.json")
            if not isinstance(data, list):
                raise InputError(f"comments of #{number} page {page} is not a list")
            last = page == len(pages)
            if (not last and len(data) != PER_PAGE) or (last and len(data) >= PER_PAGE):
                raise InputError(f"comments of #{number} page {page} has {len(data)} items: the listing is cut")
            items.extend(data)
        return items


def trusted_authors(base: str) -> set:
    try:
        shown = subprocess.run(["git", "show", f"{base}:.mintswarm/config.yaml"], capture_output=True)
    except OSError as exc:
        raise InputError(f"cannot run git: {exc}") from exc
    if shown.returncode != 0:
        raise InputError(f"cannot read .mintswarm/config.yaml at {base!r}")
    try:
        proc = subprocess.run(["yq", "-p=yaml", "-o=json", "."], input=shown.stdout, capture_output=True)
    except OSError as exc:
        raise InputError(f"cannot run yq: {exc}") from exc
    if proc.returncode != 0:
        raise InputError(f"yq cannot read .mintswarm/config.yaml at {base}")
    config = strict_json(proc.stdout, "yq's output")
    node = config
    for key in ("tracker", "ingest", "trusted", "authors"):
        if node is None:
            return set()
        if not isinstance(node, dict):
            raise InputError(f"the config's {key} parent is not a mapping")
        node = node.get(key)
    if node is None:
        return set()
    if not isinstance(node, list) or not all(isinstance(a, str) for a in node):
        raise InputError("tracker.ingest.trusted.authors is not a list of logins")
    return {a.lower() for a in node}


def text_of(value, where: str) -> str:
    if value is None:
        return ""
    if not isinstance(value, str):
        raise InputError(f"{where} is not a string")
    return value.replace("\r\n", "\n").replace("\r", "\n")


def one_line(value: str) -> str:
    return " ".join(value.split())


def has_marker(body: str) -> bool:
    return any(line.strip() == MARKER for line in body.split("\n"))


def decision_sections(body: str) -> list:
    """The sections of BODY under a heading starting with `Decision`, outside fences, in order."""
    sections = []
    current = None
    level = 0
    fence = None
    for line in body.split("\n"):
        opened = FENCE.match(line)
        if fence is not None:
            if opened and opened.group(1)[0] == fence[0] and len(opened.group(1)) >= len(fence):
                if not line.strip().lstrip(fence[0]):
                    fence = None
            if current is not None:
                current.append(line)
            continue
        if opened:
            fence = opened.group(1)
            if current is not None:
                current.append(line)
            continue
        heading = HEADING.fullmatch(line)
        if heading:
            depth = len(heading.group(1))
            if current is not None and depth <= level:
                sections.append(current)
                current = None
            if current is None and (heading.group(2) or "").startswith("Decision"):
                current = [line]
                level = depth
                continue
        if current is not None:
            current.append(line)
    if current is not None:
        sections.append(current)
    return ["\n".join(s).strip("\n") for s in sections]


def quoted(text: str) -> list:
    return [f"> {line.rstrip()}" if line.strip() else ">" for line in text.split("\n")]


def owner_of(chain: list, issues: dict, index: int):
    for number in chain[index:]:
        login = person(issues[number].get("user"))
        if login is not None:
            return login
    return None


def owner_trust(login, authors: set, responses: Responses):
    """(trusted, why) for the owner LOGIN."""
    if login is None:
        return False, "no person filed this issue or an ancestor above it"
    if login.lower() in authors:
        return True, "in tracker.ingest.trusted.authors"
    permission = responses.load(f"permission-{login.lower()}.json")
    if permission is None:
        return False, "no permission on the repository"
    if not isinstance(permission, dict):
        raise InputError(f"the saved permission of {login} is not an object")
    if permission.get("permission") in WRITE_PERMISSIONS or permission.get("role_name") in WRITE_ROLES:
        return True, "write access or higher"
    return False, "no write access"


def comment_decisions(number: int, comments: list, owner, trusted: bool, authors: set) -> list:
    found = []
    for item in comments:
        if not isinstance(item, dict):
            raise InputError(f"a comment of #{number} is not an object")
        url, created, updated, ident = (item.get(k) for k in ("html_url", "created_at", "updated_at", "id"))
        if not all(isinstance(v, str) for v in (url, created)) or not is_number(ident):
            raise InputError(f"a comment of #{number} has no html_url, created_at or id")
        if updated is not None and not isinstance(updated, str):
            raise InputError(f"the updated_at of {url} is not a string")
        user = item.get("user")
        login = user.get("login") if isinstance(user, dict) else None
        body = text_of(item.get("body"), f"the body of {url}")
        if login is None:
            continue
        if not isinstance(login, str):
            raise InputError(f"{url} has an unreadable author")
        if login == BOT and has_marker(body):
            via = "marker"
        elif person(user) is None:
            continue
        elif login.lower() in authors or (trusted and owner is not None and login.lower() == owner.lower()):
            via = "account"
        else:
            continue
        edited = updated if updated is not None and updated != created else "none"
        found.append((created, ident, login, via, edited, url, body))
    found.sort(key=lambda d: (d[0], d[1]))
    return found


def render(issue: int, chain: list, responses: Responses, authors: set) -> str:
    issues = {}
    for number in chain:
        data = responses.load(f"issue-{number}.json")
        if not isinstance(data, dict) or data.get("number") != number:
            raise InputError(f"issue-{number}.json is not issue #{number}")
        if not isinstance(data.get("title"), str) or not isinstance(data.get("html_url"), str):
            raise InputError(f"issue #{number} has no title or html_url")
        issues[number] = data
    lines = [f"# Ancestor decisions: #{issue}", ""]
    if not chain:
        lines.append(f"#{issue} has no ancestors.")
        return "\n".join(lines) + "\n"
    names = ", then ".join(f"#{n}" for n in chain)
    lines += [f"The ancestors of #{issue}, nearest first: {names}.", ""]
    for index, number in enumerate(chain):
        data = issues[number]
        owner = owner_of(chain, issues, index)
        trusted, why = owner_trust(owner, authors, responses)
        author = data.get("user").get("login") if isinstance(data.get("user"), dict) else "ghost"
        lines += [
            f"## #{number} {one_line(data['title'])}",
            "",
            f"- URL: {data['html_url']}",
            f"- Owner: {owner or 'none'}, {'trusted' if trusted else 'not trusted'} ({why})",
            "",
        ]
        count = 0
        if trusted:
            body = text_of(data.get("body"), f"the body of #{number}")
            for section in decision_sections(body):
                count += 1
                lines += [
                    f"### Decision {count}",
                    "",
                    f"- Kind: issue body section; author: {author}; via: body",
                    "- Time: none; edited: none",
                    f"- URL: {data['html_url']}",
                    "",
                    *quoted(section),
                    "",
                ]
        for created, _, login, via, edited, url, body in comment_decisions(
            number, responses.comments(number), owner, trusted, authors
        ):
            count += 1
            lines += [
                f"### Decision {count}",
                "",
                f"- Kind: comment; author: {login}; via: {via}",
                f"- Time: {created}; edited: {edited}",
                f"- URL: {url}",
                "",
                *quoted(body.strip("\n")),
                "",
            ]
        if not count:
            lines += ["No decisions.", ""]
    return "\n".join(lines).rstrip("\n") + "\n"


def write_output(text: str) -> None:
    OUTPUT.parent.mkdir(parents=True, exist_ok=True)
    fd, tmp = tempfile.mkstemp(prefix=".ancestors.md.tmp-", dir=OUTPUT.parent)
    try:
        with os.fdopen(fd, "w", encoding="utf-8") as out:
            out.write(text)
        os.chmod(tmp, 0o644)
        os.replace(tmp, OUTPUT)
    except BaseException:
        remove(tmp)
        raise


def assemble(argv: list) -> int:
    remove(OUTPUT)
    opts = parse_options(argv, ASSEMBLE_OPTIONS)
    issue = positive(opts["--issue"], "--issue")
    responses = Responses(Path(opts["--responses"]))
    request = responses.load("request.json")
    if not isinstance(request, dict) or request.get("issue") != issue:
        raise InputError(f"request.json is not for #{issue}")
    chain = request.get("ancestors")
    if not isinstance(chain, list) or not all(is_number(n) for n in chain) or len(set(chain)) != len(chain):
        raise InputError("request.json's ancestors are not a list of distinct issue numbers")
    if issue in chain or len(chain) > MAX_ANCESTORS:
        raise InputError("request.json's ancestors loop or are too many")
    authors = trusted_authors(opts["--base"])
    write_output(render(issue, chain, responses, authors))
    print(f"ancestor decisions of #{issue} from {len(chain)} ancestors in {OUTPUT}")
    return 0


def main(argv: list) -> int:
    command = argv[0] if argv else None
    if command not in ("fetch", "assemble"):
        print(USAGE, file=sys.stderr)
        return 2
    try:
        return fetch(argv[1:]) if command == "fetch" else assemble(argv[1:])
    except FetchError as exc:
        print(f"ancestor-decisions: {exc}", file=sys.stderr)
        return 1
    except InputError as exc:
        print(f"ancestor-decisions: {exc}", file=sys.stderr)
        return 2
    except Exception as exc:  # a crash must never leave a file or read as fetched
        if command == "assemble":
            try:
                remove(OUTPUT)
            except Exception:
                pass
        message = " ".join(str(exc).split())
        print(f"ancestor-decisions: unexpected error: {type(exc).__name__}: {message}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
