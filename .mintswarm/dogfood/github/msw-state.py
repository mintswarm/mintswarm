#!/usr/bin/env python3
"""Set an issue's msw-state value, and the GitHub issue status it maps to.

    msw-state.py set ISSUE VALUE [--base origin/master] [--dry-run]
    msw-state.py get ISSUE [--base origin/master]

Dogfooding stand-in for the runner's tracker effect that shows where an issue
is in the pipeline. Every GitHub call goes through bot-gh.sh, so as
mintswarm-dev[bot]. VALUE is one of the coarse phases in VALUES; the README's
`## msw-state` section maps every runner state and every Pipeline state
wording to one of them.

Both subcommands read .mintswarm/config.yaml at the base ref (--base, resolved
with `git rev-parse --verify` in the current directory), never the worktree's
copy: `git show COMMIT:.mintswarm/config.yaml` piped to
`yq -o=json . /dev/stdin`. The settings are tracker.state:

    tracker:
      state:
        source: field                # field | labels
        field: msw-state
        field_id: 50325307
        label: "msw-state:{value}"
        status: issue_status         # issue_status | off

A source other than field or labels, an empty field, a field_id that is not a
positive integer, a label without {value} or a status other than issue_status
or off exits 2.

With source field, the value is the organisation issue field: set GETs
repos/{owner}/{repo}/issues/N/issue-field-values and takes the entry whose
issue_field_id is field_id, its single_select_option.name or else its value;
with no such entry the value is none. An entry named as the field but with
another id exits 1 and writes nothing (the field was recreated: fix field_id).
When the value is already VALUE it prints `#N msw-state is already VALUE`;
otherwise it POSTs the same path with

    {"issue_field_values": [{"field_id": ID, "value": "VALUE"}]}

on stdin, checks that the response holds VALUE for that id (exit 1 if not) and
prints `set #N msw-state to VALUE (was OLD)`. A failed request exits 1 and
says that tracker.state.source: labels is the fallback where issue fields are
unavailable.

With source labels, the value is the one label made from the label template:
set reads the issue's labels (all pages); the labels that match the template
give the current value, none when there are none. When exactly one matches and
it is the target, it writes nothing. Otherwise it reads the repository's
labels (all pages), creates a missing target label (colour ededed,
description `mintswarm state`) and PUTs the issue's other labels plus the
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

With --dry-run set reads, prints the lines it would print and writes nothing.
get prints `msw-state: VALUE` and, with status issue_status, `status: NAME`.

Exits 0 on success or a no-op; 1 when git, yq or a request fails or GitHub does
not do what was asked; 2 on bad arguments or configuration. Standard library
only.
"""

import argparse
import json
import subprocess
import sys
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
    "in review": "In review",
    "awaiting merge": "In review",
    "escalated": "In review",
    "done": "Done",
}
SOURCES = ("field", "labels")
STATUS_MODES = ("issue_status", "off")
LABEL_COLOUR = "ededed"
LABEL_DESCRIPTION = "mintswarm state"
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


def settings(ref: str) -> dict:
    """tracker.state from the config at REF, checked."""
    try:
        commit = run(["git", "rev-parse", "--verify", "--quiet", "--end-of-options", f"{ref}^{{commit}}"]).strip()
    except RuntimeError as exc:
        raise RuntimeError(f"base ref {ref!r} does not resolve to a commit") from exc
    text = run(["git", "show", f"{commit}:{CONFIG}"])
    config = json.loads(run(["yq", "-o=json", ".", "/dev/stdin"], text) or "null")
    tracker = config.get("tracker") if isinstance(config, dict) else None
    state = tracker.get("state") if isinstance(tracker, dict) else None
    if not isinstance(state, dict):
        raise UsageError(f"tracker.state at {ref} must be a mapping")
    if state.get("source") not in SOURCES:
        raise UsageError(f"tracker.state.source at {ref} must be field or labels, not {state.get('source')!r}")
    field, field_id = state.get("field"), state.get("field_id")
    if not isinstance(field, str) or not field.strip():
        raise UsageError(f"tracker.state.field at {ref} must be a field name")
    if not isinstance(field_id, int) or isinstance(field_id, bool) or field_id <= 0:
        raise UsageError(f"tracker.state.field_id at {ref} must be a positive integer, not {field_id!r}")
    label = state.get("label")
    if not isinstance(label, str) or label.count("{value}") != 1:
        raise UsageError(f"tracker.state.label at {ref} must hold {{value}} once, not {label!r}")
    if state.get("status") not in STATUS_MODES:
        raise UsageError(f"tracker.state.status at {ref} must be issue_status or off, not {state.get('status')!r}")
    return {"source": state["source"], "field": field, "field_id": field_id, "label": label, "status": state["status"]}


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


def field_value(entries, config: dict, number: int) -> str:
    """The field's value among ENTRIES, none when unset; a same-named field with another id is an error."""
    if not isinstance(entries, list):
        raise RuntimeError(f"GitHub returned no issue field values for #{number}")
    for entry in entries:
        if not isinstance(entry, dict):
            continue
        if entry.get("issue_field_id") == config["field_id"]:
            option = entry.get("single_select_option")
            name = option.get("name") if isinstance(option, dict) else None
            value = name if isinstance(name, str) else entry.get("value")
            return value if isinstance(value, str) and value else "none"
    for entry in entries:
        if isinstance(entry, dict) and entry.get("issue_field_name") == config["field"]:
            raise RuntimeError(
                f"#{number} has a {config['field']} field with id {entry.get('issue_field_id')}, not"
                f" {config['field_id']}: fix tracker.state.field_id; nothing was written"
            )
    return "none"


def field_request(args: list, number: int, body: dict = None):
    try:
        return gh(args, body)
    except RuntimeError as exc:
        raise RuntimeError(
            f"{exc}; where issue fields are unavailable, set tracker.state.source: labels (#{number})"
        ) from exc


def read_field(number: int, config: dict) -> str:
    path = f"repos/{{owner}}/{{repo}}/issues/{number}/issue-field-values"
    return field_value(field_request([path], number), config, number)


def set_field(number: int, value: str, config: dict, dry_run: bool) -> None:
    old = read_field(number, config)
    if old == value:
        print(f"#{number} msw-state is already {value}")
        return
    if not dry_run:
        path = f"repos/{{owner}}/{{repo}}/issues/{number}/issue-field-values"
        body = {"issue_field_values": [{"field_id": config["field_id"], "value": value}]}
        got = field_value(field_request(["--method", "POST", path], number, body), config, number)
        if got != value:
            raise RuntimeError(f"GitHub did not set #{number}'s {config['field']} to {value} (it reports {got})")
    print(f"set #{number} msw-state to {value} (was {old})")


# --- labels ------------------------------------------------------------------


def label_of(value: str, config: dict) -> str:
    return config["label"].replace("{value}", value)


def label_value(name, config: dict):
    """The value NAME carries under the label template, or None when it does not match."""
    if not isinstance(name, str):
        return None
    prefix, suffix = config["label"].split("{value}")
    if len(name) < len(prefix) + len(suffix) or not name.startswith(prefix) or not name.endswith(suffix):
        return None
    return name[len(prefix) : len(name) - len(suffix)]


def label_names(labels, number: int) -> list:
    if not isinstance(labels, list):
        raise RuntimeError(f"GitHub returned no label list for #{number}")
    return [label.get("name") for label in labels if isinstance(label, dict) and isinstance(label.get("name"), str)]


def matching(names: list, config: dict) -> list:
    return [name for name in names if label_value(name, config) is not None]


def read_labels(number: int, config: dict) -> tuple:
    """The issue's label names and its current value (none, or the matching values joined)."""
    names = label_names(pages(f"repos/{{owner}}/{{repo}}/issues/{number}/labels", number), number)
    found = [label_value(name, config) for name in matching(names, config)]
    return names, ", ".join(found) if found else "none"


def set_labels(number: int, value: str, config: dict, dry_run: bool) -> None:
    target = label_of(value, config)
    names, old = read_labels(number, config)
    if matching(names, config) == [target]:
        print(f"#{number} msw-state is already {value}")
        return
    if not dry_run:
        known = label_names(pages("repos/{owner}/{repo}/labels", number), number)
        if target not in known:
            gh(
                ["--method", "POST", "repos/{owner}/{repo}/labels"],
                {"name": target, "color": LABEL_COLOUR, "description": LABEL_DESCRIPTION},
            )
        keep = [name for name in names if label_value(name, config) is None]
        got = gh(
            ["--method", "PUT", f"repos/{{owner}}/{{repo}}/issues/{number}/labels"],
            {"labels": keep + [target]},
        )
        left = matching(label_names(got, number), config)
        if left != [target]:
            raise RuntimeError(
                f"GitHub did not leave #{number} with exactly the label {target} (it reports"
                f" {', '.join(left) or 'none'})"
            )
    print(f"set #{number} msw-state to {value} (was {old})")


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


def set_state(number: int, value: str, base: str, dry_run: bool) -> int:
    if value not in VALUES:
        raise UsageError(f"the value must be one of {', '.join(VALUES)}, not {value!r}")
    config = settings(base)
    if config["source"] == "field":
        set_field(number, value, config, dry_run)
    else:
        set_labels(number, value, config, dry_run)
    if config["status"] == "issue_status" and value != "none":
        set_status(number, value, dry_run)
    return 0


def get_state(number: int, base: str) -> int:
    config = settings(base)
    value = read_field(number, config) if config["source"] == "field" else read_labels(number, config)[1]
    print(f"msw-state: {value}")
    if config["status"] == "issue_status":
        print(f"status: {issue_status(number)[1]}")
    return 0


def main(argv: list) -> int:
    parser = argparse.ArgumentParser(description="Set or show an issue's msw-state value and its issue status.")
    sub = parser.add_subparsers(dest="action", required=True)
    s = sub.add_parser("set", help="set the issue's msw-state value, then its issue status")
    s.add_argument("number", type=int, help="issue number")
    s.add_argument("value", help="one of: " + ", ".join(VALUES))
    s.add_argument("--base", default="origin/master", help="ref to read the config at (default origin/master)")
    s.add_argument("--dry-run", action="store_true", help="print the lines, change nothing")
    g = sub.add_parser("get", help="print the issue's msw-state value and its issue status")
    g.add_argument("number", type=int, help="issue number")
    g.add_argument("--base", default="origin/master", help="ref to read the config at (default origin/master)")
    args = parser.parse_args(argv)

    try:
        if args.number <= 0:
            raise UsageError("the issue number must be positive")
        if args.action == "set":
            return set_state(args.number, args.value, args.base, args.dry_run)
        return get_state(args.number, args.base)
    except UsageError as err:
        print(f"msw-state.py: {err}", file=sys.stderr)
        return 2
    except (RuntimeError, json.JSONDecodeError, OSError) as err:
        print(f"msw-state.py: {err}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
