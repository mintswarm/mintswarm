#!/usr/bin/env python3
"""Set an issue's msw-state value and the GitHub issue status it maps to, the verdicts and the short title.

    msw-state.py set ISSUE VALUE [--base origin/master] [--dry-run]
    msw-state.py get ISSUE [--base origin/master]
    msw-state.py verdict set NUMBER plan|decomp|pr BAND [--base origin/master] [--dry-run]
    msw-state.py verdict get NUMBER plan|decomp|pr [--base origin/master]
    msw-state.py fields resolve [ISSUE ...] [--base origin/master]
    msw-state.py short-title read ANALYSIS
    msw-state.py short-title set ISSUE --analysis PATH [--base origin/master] [--dry-run]
    msw-state.py short-title get ISSUE [--base origin/master]
    msw-state.py short-title name --title TITLE [--value VALUE]

Dogfooding stand-in for the runner's tracker effects that show where an issue
is in the pipeline and what the approval stages judged. Every GitHub call goes
through bot-gh.sh, so as mintswarm-dev[bot]. VALUE is one of the coarse phases
in VALUES; the README's `## msw-state` section maps every runner state and
every Pipeline state wording to one of them.

Every subcommand reads .mintswarm/config.yaml at the base ref (--base, resolved
with `git rev-parse --verify` in the current directory), never the worktree's
copy: `git show COMMIT:.mintswarm/config.yaml` piped to
`yq -o=json . /dev/stdin`. set and get read tracker.state:

    tracker:
      state:
        source: field                # field | labels
        field: msw-state             # field mode only
        label: "msw-state:{option}"
        status: issue_status         # issue_status | off

A source other than field or labels, a label without {option} or a status
other than issue_status or off exits 2; with source field, so does an empty
field. Source labels needs no field. A leftover field_id is ignored: fields are
named, and their ids are resolved (below).

With source field, the value is the organisation issue field: set GETs
repos/{owner}/{repo}/issues/N/issue-field-values and takes the entry whose
issue_field_name is the field, its single_select_option.name or else its
value; with no such entry the value is none. When the value is already VALUE
it prints `#N msw-state is already VALUE`; otherwise it resolves the field's id
(below) and POSTs the same path with

    {"issue_field_values": [{"field_id": ID, "value": "VALUE"}]}

on stdin, checks that the response holds VALUE for that id (exit 1 if not) and
prints `set #N msw-state to VALUE (was OLD)`. A failed request exits 1 and
says that tracker.state.source: labels is the fallback where issue fields are
unavailable.

Field ids are resolved per account, never configured, in this order:

1. the issue's own entry with the field's name, whose issue_field_id is
   authoritative (the cache is updated when it differs);
2. the cache, MSW_FIELD_CACHE or else ~/.config/mintswarm/issue-fields.json,
   shaped {"ACCOUNT": {"FIELD NAME": ID}}, where ACCOUNT is owner.login from
   one GET of repos/{owner}/{repo} per run that uses the cache; only this
   script writes it, atomically (a temporary file in the same directory, then a
   rename); a missing file is empty, a malformed one exits 1 and names it;
3. the organisation's fields, `--paginate --slurp orgs/{owner}/issue-fields`
   (id and name per field), which the bot normally cannot read (it lacks
   read:org); a refused request falls through.

When no step yields an id, set exits 1 and writes nothing, naming the operator
step: set the field to any option on one issue in GitHub, then run
`fields resolve ISSUE`. A recreated field gets a new id, so the id is resolved
again when GitHub refuses a POST with a cached id (HTTP 422 or 404): the cache
entry is dropped, the organisation's fields are read, and the POST is retried
once with a different id found there (exit 1 when none is). Any other failure
exits 1 and keeps the cache. When GitHub's answer lists the
POSTed id under another name, the field was renamed: the entry is dropped and
set exits 1. Notes about learnt and dropped ids go to stderr.

`fields resolve [ISSUE ...]` learns the ids of every field-mode block at the
base (tracker.state, tracker.plan_verdict, tracker.decomp_verdict,
tracker.short_title) from the
given issues' field values, and from the organisation's fields when they are
readable, writes those it found to the cache and prints one
`NAME: ID (from #N)`, `NAME: ID (from the organisation)` or `NAME: unresolved`
line per field. It exits 1 if any field stays unresolved.

With source labels, the value is the one label made from the label template:
set reads the issue's labels (all pages); the labels that match the template
give the current value, none when there are none. Label names are compared
case-insensitively, as GitHub compares them. When exactly one matches and it
is the target, it writes nothing. Otherwise it reads the repository's labels
(all pages), creates the target label when no label has its name in any case
(colour ededed, description `mintswarm state`) and PUTs the issue's other labels plus the
target in one request, so the issue never has zero or two; the response must
hold exactly one matching label, the target (exit 1 if not).

Then, when status is issue_status and VALUE is not none, set syncs the issue
status, whatever the source and even when the value was already current, so
a re-run repairs a status that GitHub's own automations moved (assigning sets
Todo, linking a PR sets In review). It GETs the issue for node_id and
status.name (unset when null); when that is already STATUS[VALUE] it prints
`#N status is already NAME`. Otherwise it reads the issue's repository's
planningStatuses through GraphQL, picks the status of that name (exit 1 when
there is none, saying tracker.state.status: off turns this off), runs
setIssueStatus, GETs the issue again, checks that the status is now NAME
(exit 1 if not) and prints `set #N status to NAME (was OLD)`.

With --dry-run set reads, prints the lines it would print and writes nothing;
it resolves no id. get prints `msw-state: VALUE` and, with status
issue_status, `status: NAME`.

`verdict set` records an approval stage's band, one of BANDS, the same way:
plan sets tracker.plan_verdict (the msw-plan field, or one `msw-plan:BAND`
label), decomp sets tracker.decomp_verdict (msw-decomp), each a block with
source, field and label only (a status key exits 2), and pr sets exactly one
`msw-verdict:BAND` label on the pull request, from a fixed template that reads
no config (PRs have no issue fields; labels are created with the description
`mintswarm verdict`). Its lines name msw-plan, msw-decomp or msw-verdict where
set names msw-state, it never touches the issue status, and `verdict get`
prints `NAME: BAND`.

`short-title` keeps the item's short name, for displays with little room, in
tracker.short_title, a block like the verdicts' (source, field and label; a
status key exits 2) whose field is a free-text organisation issue field.

`short-title read ANALYSIS` reads the name the analyse stage proposed: the
analysis must hold exactly one `## Short title` heading outside fences (found
with reclassify.py's scan, loaded without writing bytecode), the heading before
it must be `## Size`, and its body, ignoring blank lines and trailing spaces and
tabs, must be exactly the one line `short title: NAME`, NAME two or three words
of ASCII letters and digits in any case, single-spaced, at most WIDTH (24)
characters. It prints `short title: NAME` and exits 0; a missing or malformed
section exits 1 with one reason line, an unreadable file exits 2.

`short-title set ISSUE --analysis PATH` reads the name the same way (a missing
or malformed section exits 2) and writes it only when the issue holds none, so
a name a person set is never overwritten. In field mode the field's raw text is
read: absent or blank is empty, any other text (the literal none included) is
set. When set it prints `#N msw-short-title is already set to SANITISED; left
unchanged` and writes nothing; otherwise it resolves the id and POSTs as set
does, with the same retry, rename and answer checks, and prints `set #N
msw-short-title to NAME (was none)`. A value set by a person between the read
and the POST is overwritten; the window is one request. In labels mode any
label matching the template counts as set; otherwise the one label is added as
set adds it. --dry-run reads, prints and writes nothing.

`short-title get ISSUE` GETs the issue for its title, reads the field or label
and prints `msw-short-title: NAME`, the display name; `short-title name --title
TITLE [--value VALUE]` prints the display name alone and reads nothing. This
script is the only home of that rule, which keeps the case of the words it is
given; every display calls get or name rather than reimplementing it:

- sanitising: each character other than ASCII letters, digits, space and
  `- . / &` becomes a space, runs of spaces collapse, and the result is
  stripped, cut to WIDTH and stripped again;
- the display name is the sanitised value, or, when that is empty, the title's
  fallback: a `type(scope)!:` prefix is dropped, the rest is split on anything
  but ASCII letters and digits, FILLER words are dropped, the first three are
  kept, words are dropped from the end while the name is longer than WIDTH, a
  single word longer than WIDTH is cut, and no word at all gives `Untitled`.

Exits 0 on success or a no-op; 1 when git, yq or a request fails, an id cannot
be resolved or GitHub does not do what was asked; 2 on bad arguments or
configuration. Standard library only.
"""

import argparse
import importlib.util
import json
import os
import re
import subprocess
import sys
import tempfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
BOT_GH = HERE / "bot-gh.sh"
CONFIG = ".mintswarm/config.yaml"
VALUES = (
    "none",
    "queued",
    "blocked",
    "analysing",
    "designing",
    "planning",
    "awaiting approval",
    "plan approved",
    "implementing",
    "in review",
    "awaiting merge",
    "escalated",
    "done",
)
STATUS = {
    "queued": "Todo",
    "blocked": "Todo",
    "analysing": "In progress",
    "designing": "In progress",
    "planning": "In progress",
    "implementing": "In progress",
    "awaiting approval": "In review",
    "plan approved": "In progress",
    "in review": "In review",
    "awaiting merge": "In review",
    "escalated": "In review",
    "done": "Done",
}
BANDS = ("blocked", "uncertain", "needs-review", "likely-safe", "safe")
SOURCES = ("field", "labels")
STATUS_MODES = ("issue_status", "off")
LABEL_COLOUR = "ededed"
# tracker.<key>: the name the output uses and the description of the labels it creates.
BLOCKS = {
    "state": ("msw-state", "mintswarm state"),
    "plan_verdict": ("msw-plan", "mintswarm verdict"),
    "decomp_verdict": ("msw-decomp", "mintswarm verdict"),
    "short_title": ("msw-short-title", "mintswarm short title"),
}
VERDICT_KEYS = {"plan": "plan_verdict", "decomp": "decomp_verdict"}
PR_VERDICT = {
    "source": "labels",
    "label": "msw-verdict:{option}",
    "status": "off",
    "path": "msw-verdict",
    "name": "msw-verdict",
    "description": "mintswarm verdict",
}
# The short title: its width, the words the fallback drops and the characters a display shows.
WIDTH = 24
FILLER = frozenset(
    "a an the and or nor but so of in on at to for from by with without into onto via as per vs is are be"
    " it its this that these those".split()
)
DISALLOWED = re.compile(r"[^A-Za-z0-9 ./&-]")
SHORT_NAME = re.compile(r"[A-Za-z0-9]+(?: [A-Za-z0-9]+){1,2}")
PREFIX = re.compile(r"^\s*[A-Za-z]+(?:\([^)]*\))?!?:\s*")
SHORT_HEADING = "## Short title"
FIELD_CACHE = Path.home() / ".config" / "mintswarm" / "issue-fields.json"
STATUSES_QUERY = (
    "query($id: ID!) { node(id: $id) { ... on Issue { repository {"
    " planningStatuses(first: 50) { nodes { id name } } } } } }"
)
SET_STATUS = (
    "mutation($issue: ID!, $status: ID!) {"
    " setIssueStatus(input: {issueId: $issue, statusId: $status}) { clientMutationId } }"
)


class UsageError(Exception):
    pass


def run(argv: list, data: str = None) -> str:
    try:
        proc = subprocess.run(argv, input=data, capture_output=True, text=True)
    except OSError as exc:
        raise RuntimeError(f"cannot run {argv[0]}: {exc}") from exc
    if proc.returncode != 0:
        raise RuntimeError(f"{' '.join(argv[:2])} failed: {proc.stderr.strip()}")
    return proc.stdout


def config_at(ref: str):
    """The config at REF, as JSON."""
    try:
        commit = run(["git", "rev-parse", "--verify", "--quiet", "--end-of-options", f"{ref}^{{commit}}"]).strip()
    except RuntimeError as exc:
        raise RuntimeError(f"base ref {ref!r} does not resolve to a commit") from exc
    text = run(["git", "show", f"{commit}:{CONFIG}"])
    return json.loads(run(["yq", "-o=json", ".", "/dev/stdin"], text) or "null")


def settings(ref: str, key: str = "state", config=None) -> dict:
    """tracker.KEY from the config at REF (or CONFIG, already read), checked."""
    if config is None:
        config = config_at(ref)
    path = f"tracker.{key}"
    tracker = config.get("tracker") if isinstance(config, dict) else None
    block = tracker.get(key) if isinstance(tracker, dict) else None
    if not isinstance(block, dict):
        raise UsageError(f"{path} at {ref} must be a mapping")
    if block.get("source") not in SOURCES:
        raise UsageError(f"{path}.source at {ref} must be field or labels, not {block.get('source')!r}")
    field = block.get("field")
    if block["source"] == "field" and (not isinstance(field, str) or not field.strip()):
        raise UsageError(f"{path}.field at {ref} must be a field name")
    label = block.get("label")
    if not isinstance(label, str) or label.count("{option}") != 1:
        raise UsageError(f"{path}.label at {ref} must hold {{option}} once, not {label!r}")
    if key == "state":
        if block.get("status") not in STATUS_MODES:
            raise UsageError(f"{path}.status at {ref} must be issue_status or off, not {block.get('status')!r}")
        status = block["status"]
    elif "status" in block:
        raise UsageError(f"{path} at {ref} takes no status")
    else:
        status = "off"
    name, description = BLOCKS[key]
    return {
        "source": block["source"],
        "field": field,
        "label": label,
        "status": status,
        "path": path,
        "name": name,
        "description": description,
    }


def gh(args: list, body: dict = None):
    argv = [str(BOT_GH), "api"] + args
    if body is not None:
        argv += ["--input", "-"]
    return json.loads(run(argv, json.dumps(body) if body is not None else None) or "null")


def pages(path: str, number: int) -> list:
    """Every item of a paginated list, through --paginate --slurp."""
    got = gh(["--paginate", "--slurp", path]) or []
    if not isinstance(got, list) or not all(isinstance(page, list) for page in got):
        raise RuntimeError(f"GitHub returned no list for {path.replace('{owner}/{repo}', '…')} (#{number})")
    return [item for page in got for item in page]


# --- field -------------------------------------------------------------------


class Fields:
    """Field ids by name for the repository's account: the cache, and the organisation's fields."""

    def __init__(self):
        self.path = Path(os.environ.get("MSW_FIELD_CACHE") or FIELD_CACHE)
        self._account = None
        self._org = None

    def account(self) -> str:
        if self._account is None:
            repo = gh(["repos/{owner}/{repo}"])
            owner = repo.get("owner") if isinstance(repo, dict) else None
            login = owner.get("login") if isinstance(owner, dict) else None
            if not isinstance(login, str) or not login:
                raise RuntimeError("GitHub returned no owner for the repository")
            self._account = login
        return self._account

    def load(self) -> dict:
        try:
            text = self.path.read_text(encoding="utf-8")
        except FileNotFoundError:
            return {}
        try:
            cache = json.loads(text)
        except json.JSONDecodeError:
            cache = None
        valid = isinstance(cache, dict) and all(
            isinstance(ids, dict) and all(is_id(i) for i in ids.values()) for ids in cache.values()
        )
        if not valid:
            raise RuntimeError(f"the field id cache {self.path} is malformed; fix or remove it")
        return cache

    def save(self, cache: dict) -> None:
        self.path.parent.mkdir(parents=True, exist_ok=True)
        fd, tmp = tempfile.mkstemp(dir=self.path.parent, prefix=".issue-fields.", suffix=".tmp")
        try:
            with os.fdopen(fd, "w", encoding="utf-8") as f:
                json.dump(cache, f, indent=2, sort_keys=True)
                f.write("\n")
            os.replace(tmp, self.path)
        except BaseException:
            Path(tmp).unlink(missing_ok=True)
            raise

    def cached(self, name: str):
        return self.load().get(self.account(), {}).get(name)

    def remember(self, name: str, field_id: int, where: str) -> None:
        cache = self.load()
        ids = cache.setdefault(self.account(), {})
        if ids.get(name) != field_id:
            ids[name] = field_id
            self.save(cache)
            note(f"learnt the {name} field id {field_id} for {self.account()} ({where})")

    def forget(self, name: str) -> None:
        cache = self.load()
        ids = cache.get(self.account(), {})
        if name in ids:
            note(f"dropped the cached {name} field id {ids.pop(name)} for {self.account()}")
            self.save(cache)

    def organisation(self) -> dict:
        """The organisation's field ids by name, empty when GitHub refuses to list them."""
        if self._org is None:
            self._org = {}
            try:
                got = gh(["--paginate", "--slurp", "orgs/{owner}/issue-fields"])
            except RuntimeError:
                return self._org
            for page in got if isinstance(got, list) else []:
                for item in page if isinstance(page, list) else []:
                    if isinstance(item, dict) and isinstance(item.get("name"), str) and is_id(item.get("id")):
                        self._org.setdefault(item["name"], item["id"])
        return self._org


def is_id(value) -> bool:
    return isinstance(value, int) and not isinstance(value, bool) and value > 0


def note(text: str) -> None:
    print(f"msw-state.py: {text}", file=sys.stderr)


def unresolved(config: dict, number: int) -> str:
    return (
        f"cannot resolve the {config['field']} field's id: the bot cannot list the organisation's fields and"
        f" #{number} has no {config['field']} value; set {config['field']} to any option on one issue in GitHub,"
        f" then run msw-state.py fields resolve ISSUE (or set {config['path']}.source: labels); nothing was written"
    )


def field_entry(entries, name: str, number: int) -> tuple:
    """The value of the field named NAME among ENTRIES (none when unset) and its id (None when unset)."""
    if not isinstance(entries, list):
        raise RuntimeError(f"GitHub returned no issue field values for #{number}")
    for entry in entries:
        if isinstance(entry, dict) and entry.get("issue_field_name") == name:
            option = entry.get("single_select_option")
            label = option.get("name") if isinstance(option, dict) else None
            value = label if isinstance(label, str) else entry.get("value")
            field_id = entry.get("issue_field_id")
            return (value if isinstance(value, str) and value else "none"), (field_id if is_id(field_id) else None)
    return "none", None


def field_value(entries, config: dict, number: int) -> str:
    """The field's value among ENTRIES, none when unset."""
    return field_entry(entries, config["field"], number)[0]


def field_request(args: list, number: int, config: dict, body: dict = None):
    try:
        return gh(args, body)
    except RuntimeError as exc:
        raise RuntimeError(
            f"{exc}; where issue fields are unavailable, set {config['path']}.source: labels (#{number})"
        ) from exc


def read_entries(number: int, config: dict):
    path = f"repos/{{owner}}/{{repo}}/issues/{number}/issue-field-values"
    return field_request([path], number, config)


def read_field(number: int, config: dict) -> str:
    return field_value(read_entries(number, config), config, number)


def resolve(name: str, entry_id, number: int, fields: Fields, config: dict) -> tuple:
    """The field's id and where it came from: the issue, the cache or the organisation."""
    if entry_id is not None:
        fields.remember(name, entry_id, f"from #{number}")
        return entry_id, "issue"
    cached = fields.cached(name)
    if cached is not None:
        return cached, "cache"
    found = fields.organisation().get(name)
    if found is not None:
        fields.remember(name, found, "from the organisation")
        return found, "organisation"
    raise RuntimeError(unresolved(config, number))


def refuses_id(exc: RuntimeError) -> bool:
    """Whether GitHub refused a field POST over its field id (HTTP 422 or 404), not for another reason."""
    return re.search(r"\bHTTP (?:404|422)\b", str(exc)) is not None


def post_field(number: int, value: str, config: dict, field_id: int) -> object:
    path = f"repos/{{owner}}/{{repo}}/issues/{number}/issue-field-values"
    body = {"issue_field_values": [{"field_id": field_id, "value": value}]}
    return field_request(["--method", "POST", path], number, config, body)


def write_field(number: int, value: str, config: dict, entry_id) -> None:
    """POST VALUE to the field, resolving its id (ENTRY_ID is the issue's own, or None), and check the answer."""
    name = config["field"]
    fields = Fields()
    field_id, how = resolve(name, entry_id, number, fields, config)
    try:
        got = post_field(number, value, config, field_id)
    except RuntimeError as exc:
        if how != "cache" or not refuses_id(exc):
            raise
        fields.forget(name)
        again = fields.organisation().get(name)
        if again is None or again == field_id:
            raise RuntimeError(f"{exc}; the cached id {field_id} was dropped; {unresolved(config, number)}") from exc
        fields.remember(name, again, "from the organisation")
        field_id = again
        got = post_field(number, value, config, field_id)
    if not isinstance(got, list):
        raise RuntimeError(f"GitHub returned no issue field values for #{number}")
    posted = [e for e in got if isinstance(e, dict) and e.get("issue_field_id") == field_id]
    if posted and posted[0].get("issue_field_name") != name:
        fields.forget(name)
        raise RuntimeError(
            f"GitHub names the field with id {field_id} {posted[0].get('issue_field_name')!r}, not {name}: the"
            f" field was renamed; rename it back or change {config['path']}.field"
        )
    got = field_value(posted, config, number)
    if got != value:
        raise RuntimeError(f"GitHub did not set #{number}'s {name} to {value} (it reports {got})")


def set_field(number: int, value: str, config: dict, dry_run: bool) -> None:
    old, entry_id = field_entry(read_entries(number, config), config["field"], number)
    if old == value:
        print(f"#{number} {config['name']} is already {value}")
        return
    if not dry_run:
        write_field(number, value, config, entry_id)
    print(f"set #{number} {config['name']} to {value} (was {old})")


# --- labels ------------------------------------------------------------------


def label_of(value: str, config: dict) -> str:
    return config["label"].replace("{option}", value)


def same_label(a: str, b: str) -> bool:
    """GitHub compares label names case-insensitively."""
    return a.casefold() == b.casefold()


def label_value(name, config: dict):
    """The value NAME carries under the label template, in any case, or None when it does not match."""
    if not isinstance(name, str):
        return None
    prefix, suffix = config["label"].split("{option}")
    if len(name) < len(prefix) + len(suffix):
        return None
    if not same_label(name[: len(prefix)], prefix) or not same_label(name[len(name) - len(suffix) :], suffix):
        return None
    return name[len(prefix) : len(name) - len(suffix)]


def label_names(labels, number: int) -> list:
    if not isinstance(labels, list):
        raise RuntimeError(f"GitHub returned no label list for #{number}")
    return [label.get("name") for label in labels if isinstance(label, dict) and isinstance(label.get("name"), str)]


def matching(names: list, config: dict) -> list:
    return [name for name in names if label_value(name, config) is not None]


def only_target(names: list, target: str, config: dict) -> bool:
    """Whether exactly one of NAMES matches the template and it is TARGET, in any case."""
    found = matching(names, config)
    return len(found) == 1 and same_label(found[0], target)


def read_labels(number: int, config: dict) -> tuple:
    """The issue's label names and its current value (none, or the matching values joined)."""
    names = label_names(pages(f"repos/{{owner}}/{{repo}}/issues/{number}/labels", number), number)
    found = [label_value(name, config) for name in matching(names, config)]
    return names, ", ".join(found) if found else "none"


def set_labels(number: int, value: str, config: dict, dry_run: bool) -> None:
    target = label_of(value, config)
    names, old = read_labels(number, config)
    if only_target(names, target, config):
        print(f"#{number} {config['name']} is already {value}")
        return
    if not dry_run:
        known = label_names(pages("repos/{owner}/{repo}/labels", number), number)
        if not any(same_label(name, target) for name in known):
            gh(
                ["--method", "POST", "repos/{owner}/{repo}/labels"],
                {"name": target, "color": LABEL_COLOUR, "description": config["description"]},
            )
        keep = [name for name in names if label_value(name, config) is None]
        got = gh(
            ["--method", "PUT", f"repos/{{owner}}/{{repo}}/issues/{number}/labels"],
            {"labels": keep + [target]},
        )
        left = matching(label_names(got, number), config)
        if not only_target(left, target, config):
            raise RuntimeError(
                f"GitHub did not leave #{number} with exactly the label {target} (it reports"
                f" {', '.join(left) or 'none'})"
            )
    print(f"set #{number} {config['name']} to {value} (was {old})")


# --- status ------------------------------------------------------------------


def issue_status(number: int) -> tuple:
    """The issue's node id and status name, unset when it has none."""
    issue = gh([f"repos/{{owner}}/{{repo}}/issues/{number}"])
    node = issue.get("node_id") if isinstance(issue, dict) else None
    if not isinstance(node, str) or not node:
        raise RuntimeError(f"GitHub returned no node id for #{number}")
    status = issue.get("status")
    name = status.get("name") if isinstance(status, dict) else None
    return node, name if isinstance(name, str) and name else "unset"


def graphql(query: str, variables: dict, number: int) -> dict:
    got = gh(["graphql", "--method", "POST"], {"query": query, "variables": variables})
    if not isinstance(got, dict) or got.get("errors") or not isinstance(got.get("data"), dict):
        errors = got.get("errors") if isinstance(got, dict) else None
        raise RuntimeError(f"GitHub's GraphQL API refused a request for #{number}: {json.dumps(errors)}")
    return got["data"]


def status_id(node: str, name: str, number: int) -> str:
    data = graphql(STATUSES_QUERY, {"id": node}, number)
    repository = (data.get("node") or {}).get("repository") if isinstance(data.get("node"), dict) else None
    statuses = (repository or {}).get("planningStatuses") if isinstance(repository, dict) else None
    nodes = statuses.get("nodes") if isinstance(statuses, dict) else None
    if not isinstance(nodes, list):
        raise RuntimeError(f"GitHub returned no issue statuses for #{number}'s repository")
    for status in nodes:
        if isinstance(status, dict) and status.get("name") == name and isinstance(status.get("id"), str):
            return status["id"]
    known = ", ".join(s.get("name") for s in nodes if isinstance(s, dict) and isinstance(s.get("name"), str))
    raise RuntimeError(
        f"#{number}'s repository has no issue status {name} (it has {known or 'none'});"
        " set tracker.state.status: off to stop setting statuses"
    )


def set_status(number: int, value: str, dry_run: bool) -> None:
    name = STATUS[value]
    node, old = issue_status(number)
    if old == name:
        print(f"#{number} status is already {name}")
        return
    target = status_id(node, name, number)
    if not dry_run:
        graphql(SET_STATUS, {"issue": node, "status": target}, number)
        _, got = issue_status(number)
        if got != name:
            raise RuntimeError(f"GitHub did not set #{number}'s status to {name} (it reports {got})")
    print(f"set #{number} status to {name} (was {old})")


# --- commands ----------------------------------------------------------------


def set_value(number: int, value: str, config: dict, dry_run: bool) -> None:
    if config["source"] == "field":
        set_field(number, value, config, dry_run)
    else:
        set_labels(number, value, config, dry_run)


def get_value(number: int, config: dict) -> str:
    return read_field(number, config) if config["source"] == "field" else read_labels(number, config)[1]


def set_state(number: int, value: str, base: str, dry_run: bool) -> int:
    if value not in VALUES:
        raise UsageError(f"the value must be one of {', '.join(VALUES)}, not {value!r}")
    config = settings(base)
    set_value(number, value, config, dry_run)
    if config["status"] == "issue_status" and value != "none":
        set_status(number, value, dry_run)
    return 0


def get_state(number: int, base: str) -> int:
    config = settings(base)
    print(f"msw-state: {get_value(number, config)}")
    if config["status"] == "issue_status":
        print(f"status: {issue_status(number)[1]}")
    return 0


def verdict_settings(kind: str, base: str) -> dict:
    if kind == "pr":
        return dict(PR_VERDICT)
    if kind not in VERDICT_KEYS:
        raise UsageError(f"the kind must be plan, decomp or pr, not {kind!r}")
    return settings(base, VERDICT_KEYS[kind])


def set_verdict(number: int, kind: str, band: str, base: str, dry_run: bool) -> int:
    if band not in BANDS:
        raise UsageError(f"the band must be one of {', '.join(BANDS)}, not {band!r}")
    set_value(number, band, verdict_settings(kind, base), dry_run)
    return 0


def get_verdict(number: int, kind: str, base: str) -> int:
    config = verdict_settings(kind, base)
    print(f"{config['name']}: {get_value(number, config)}")
    return 0


def resolve_fields(numbers: list, base: str) -> int:
    config = config_at(base)
    tracker = config.get("tracker") if isinstance(config, dict) else None
    blocks = {}
    for key in BLOCKS:
        if isinstance(tracker, dict) and key in tracker:
            block = settings(base, key, config)
            if block["source"] == "field":
                blocks.setdefault(block["field"], block)
    if not blocks:
        return 0
    found = {}
    for number in numbers:
        entries = read_entries(number, next(iter(blocks.values())))
        for name in blocks:
            field_id = field_entry(entries, name, number)[1]
            if name not in found and field_id is not None:
                found[name] = (field_id, f"from #{number}")
    fields = Fields()
    if any(name not in found for name in blocks):
        for name, field_id in fields.organisation().items():
            if name in blocks and name not in found:
                found[name] = (field_id, "from the organisation")
    if found:
        cache = fields.load()
        cache.setdefault(fields.account(), {}).update({name: found[name][0] for name in found})
        fields.save(cache)
    for name in blocks:
        print(f"{name}: {found[name][0]} ({found[name][1]})" if name in found else f"{name}: unresolved")
    return 0 if all(name in found for name in blocks) else 1


# --- short title -------------------------------------------------------------


class SectionError(Exception):
    """A missing or malformed `## Short title` section."""


def sanitise(value: str) -> str:
    """VALUE with only the characters a display shows, single-spaced and cut to WIDTH, in its own case."""
    return " ".join(DISALLOWED.sub(" ", value).split())[:WIDTH].strip()


def fallback_name(title: str) -> str:
    """The short name made from TITLE, in the case of its words."""
    words = [w for w in re.split(r"[^A-Za-z0-9]+", PREFIX.sub("", title, count=1)) if w]
    words = [w for w in words if w.casefold() not in FILLER][:3]
    while len(words) > 1 and len(" ".join(words)) > WIDTH:
        words.pop()
    return " ".join(words)[:WIDTH] if words else "Untitled"


def display_name(value, title: str) -> str:
    """The name a display shows: VALUE sanitised, or TITLE's fallback when that leaves nothing."""
    return (sanitise(value) if value else "") or fallback_name(title)


def heading_scan():
    """reclassify.py's scan, loaded without writing bytecode."""
    spec = importlib.util.spec_from_file_location("msw_reclassify", HERE / "reclassify.py")
    module = importlib.util.module_from_spec(spec)
    sys.dont_write_bytecode = True
    spec.loader.exec_module(module)
    return module.scan


def read_short_title(path: str) -> str:
    """The name in the analysis's `## Short title` section; UsageError when unreadable, SectionError when bad."""
    try:
        text = Path(path).read_text(encoding="utf-8")
    except (OSError, UnicodeDecodeError) as exc:
        raise UsageError(f"cannot read the analysis: {exc}") from exc
    lines = [line[:-1] if line.endswith("\r") else line for line in text.split("\n")]
    marks, _ = heading_scan()(lines)
    starts = [i for i in marks if lines[i].rstrip(" \t") == SHORT_HEADING]
    if len(starts) != 1:
        raise SectionError(f"the analysis must have exactly one '{SHORT_HEADING}' section, not {len(starts)}")
    start = starts[0]
    before = [i for i in marks if i < start]
    if not before or lines[before[-1]].rstrip(" \t") != "## Size":
        raise SectionError(f"'{SHORT_HEADING}' must come right after '## Size'")
    end = next((i for i in marks if i > start), len(lines))
    body = [line.rstrip(" \t") for line in lines[start + 1 : end] if line.strip(" \t")]
    if len(body) != 1 or not body[0].startswith("short title: "):
        raise SectionError(f"'{SHORT_HEADING}' must hold exactly one line 'short title: NAME'")
    name = body[0][len("short title: ") :]
    if not SHORT_NAME.fullmatch(name) or len(name) > WIDTH:
        raise SectionError(
            f"the short title {name!r} must be two or three words of ASCII letters and digits, single-spaced,"
            f" at most {WIDTH} characters"
        )
    return name


def raw_field(number: int, config: dict) -> tuple:
    """The field's text (None when absent or blank) and its id on the issue (None when unset)."""
    entries = read_entries(number, config)
    if not isinstance(entries, list):
        raise RuntimeError(f"GitHub returned no issue field values for #{number}")
    for entry in entries:
        if isinstance(entry, dict) and entry.get("issue_field_name") == config["field"]:
            option = entry.get("single_select_option")
            label = option.get("name") if isinstance(option, dict) else None
            value = label if isinstance(label, str) else entry.get("value")
            field_id = entry.get("issue_field_id")
            text = value if isinstance(value, str) and value.strip() else None
            return text, (field_id if is_id(field_id) else None)
    return None, None


def raw_label(number: int, config: dict):
    """The value of the first label matching the template, None when there is none."""
    names, _ = read_labels(number, config)
    found = matching(names, config)
    return label_value(found[0], config) if found else None


def short_title_read(path: str) -> int:
    try:
        name = read_short_title(path)
    except SectionError as err:
        print(f"msw-state.py: {err}", file=sys.stderr)
        return 1
    print(f"short title: {name}")
    return 0


def short_title_set(number: int, analysis: str, base: str, dry_run: bool) -> int:
    try:
        name = read_short_title(analysis)
    except SectionError as err:
        raise UsageError(str(err)) from err
    config = settings(base, "short_title")
    if config["source"] == "field":
        current, entry_id = raw_field(number, config)
    else:
        current, entry_id = raw_label(number, config), None
    if current is not None:
        print(f"#{number} {config['name']} is already set to {sanitise(current)}; left unchanged")
        return 0
    if config["source"] == "labels":
        set_labels(number, name, config, dry_run)
        return 0
    if not dry_run:
        write_field(number, name, config, entry_id)
    print(f"set #{number} {config['name']} to {name} (was none)")
    return 0


def short_title_get(number: int, base: str) -> int:
    config = settings(base, "short_title")
    issue = gh([f"repos/{{owner}}/{{repo}}/issues/{number}"])
    title = issue.get("title") if isinstance(issue, dict) else None
    if not isinstance(title, str):
        raise RuntimeError(f"GitHub returned no title for #{number}")
    value = raw_field(number, config)[0] if config["source"] == "field" else raw_label(number, config)
    print(f"{config['name']}: {display_name(value, title)}")
    return 0


def main(argv: list) -> int:
    parser = argparse.ArgumentParser(
        description="Set or show an issue's msw-state value, its status, verdicts and short title."
    )
    sub = parser.add_subparsers(dest="action", required=True)
    base_help = "ref to read the config at (default origin/master)"
    s = sub.add_parser("set", help="set the issue's msw-state value, then its issue status")
    s.add_argument("number", type=int, help="issue number")
    s.add_argument("value", help="one of: " + ", ".join(VALUES))
    s.add_argument("--base", default="origin/master", help=base_help)
    s.add_argument("--dry-run", action="store_true", help="print the lines, change nothing")
    g = sub.add_parser("get", help="print the issue's msw-state value and its issue status")
    g.add_argument("number", type=int, help="issue number")
    g.add_argument("--base", default="origin/master", help=base_help)
    v = sub.add_parser("verdict", help="set or print an approval stage's verdict")
    vsub = v.add_subparsers(dest="verdict", required=True)
    vs = vsub.add_parser("set", help="set the verdict to a band")
    vs.add_argument("number", type=int, help="issue or pull request number")
    vs.add_argument("kind", help="plan, decomp or pr")
    vs.add_argument("band", help="one of: " + ", ".join(BANDS))
    vs.add_argument("--base", default="origin/master", help=base_help)
    vs.add_argument("--dry-run", action="store_true", help="print the line, change nothing")
    vg = vsub.add_parser("get", help="print the verdict")
    vg.add_argument("number", type=int, help="issue or pull request number")
    vg.add_argument("kind", help="plan, decomp or pr")
    vg.add_argument("--base", default="origin/master", help=base_help)
    f = sub.add_parser("fields", help="resolve the configured fields' ids into the cache")
    fsub = f.add_subparsers(dest="fields", required=True)
    fr = fsub.add_parser("resolve", help="learn the field ids from issues and the organisation")
    fr.add_argument("numbers", type=int, nargs="*", metavar="ISSUE", help="issues that hold the fields")
    fr.add_argument("--base", default="origin/master", help=base_help)
    t = sub.add_parser("short-title", help="read, set or show the item's short title")
    tsub = t.add_subparsers(dest="short_title", required=True)
    tr = tsub.add_parser("read", help="print the short title the analysis proposes")
    tr.add_argument("analysis", help="the analysis to read")
    ts = tsub.add_parser("set", help="write the analysis's short title when the issue holds none")
    ts.add_argument("number", type=int, help="issue number")
    ts.add_argument("--analysis", required=True, help="the analysis to read the name from")
    ts.add_argument("--base", default="origin/master", help=base_help)
    ts.add_argument("--dry-run", action="store_true", help="print the line, change nothing")
    tg = tsub.add_parser("get", help="print the issue's display name")
    tg.add_argument("number", type=int, help="issue number")
    tg.add_argument("--base", default="origin/master", help=base_help)
    tn = tsub.add_parser("name", help="print the display name for a title and value, offline")
    tn.add_argument("--title", required=True, help="the item's title")
    tn.add_argument("--value", default="", help="the field's or label's value, if any")
    args = parser.parse_args(argv)

    try:
        if args.action == "fields":
            numbers = args.numbers
        elif args.action == "short-title" and args.short_title in ("read", "name"):
            numbers = []
        else:
            numbers = [args.number]
        if any(number <= 0 for number in numbers):
            raise UsageError("the issue number must be positive")
        if args.action == "short-title":
            if args.short_title == "read":
                return short_title_read(args.analysis)
            if args.short_title == "set":
                return short_title_set(args.number, args.analysis, args.base, args.dry_run)
            if args.short_title == "get":
                return short_title_get(args.number, args.base)
            print(display_name(args.value, args.title))
            return 0
        if args.action == "set":
            return set_state(args.number, args.value, args.base, args.dry_run)
        if args.action == "get":
            return get_state(args.number, args.base)
        if args.action == "fields":
            return resolve_fields(args.numbers, args.base)
        if args.verdict == "set":
            return set_verdict(args.number, args.kind, args.band, args.base, args.dry_run)
        return get_verdict(args.number, args.kind, args.base)
    except UsageError as err:
        print(f"msw-state.py: {err}", file=sys.stderr)
        return 2
    except (RuntimeError, json.JSONDecodeError, OSError) as err:
        print(f"msw-state.py: {err}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
