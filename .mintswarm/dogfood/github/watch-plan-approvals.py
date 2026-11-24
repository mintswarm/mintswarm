#!/usr/bin/env python3
"""Report plan approvals given on the tracker, with a verdict on each; never write.

    watch-plan-approvals.py ISSUE... [--base origin/master]

Dogfooding stand-in for the control plane's handler of `/approve-plan` and of a
hand-set `plan approved` msw-state (`.mintswarm/dogfood/plan-approval/README.md`,
"Approval from the tracker"). The session's monitor runs it every polling
interval and acts on each line it prints (Session procedure, step 14, in this
directory's README). It only reads; every GitHub call goes through bot-gh.sh.

It reads tracker.state with msw-state.py's settings() and the trusted approvers
with assign-approvers.py's load_config() and approvers(), all at the base ref
(--base, resolved in the current directory), so the trusted approvers are
exactly pipeline.review.reviewers, the people a waiting plan is assigned to,
never a `[bot]` login. Logins are compared case-insensitively. For each ISSUE it
reads the msw-state value (msw-state.py's read_field or read_labels), the
comments (`--paginate --slurp repos/{owner}/{repo}/issues/N/comments`) and, in
labels mode, the events (the same for `issues/N/events`). Comments and events
are ordered by created_at, then id.

The current approval package is the latest mintswarm-dev[bot] comment whose
first line is `<!-- mintswarm: plan approval package -->` and whose second is
`Plan sha256: <64 hex digits>`; PKG is its html_url and SHA that digest. A
comment's first line is taken with CRLF normalised and whitespace trimmed.

A command is a comment, not by mintswarm-dev[bot], whose first line is exactly
`/approve-plan`. It is answered when a mintswarm-dev[bot] comment holds a line
that is exactly `<!-- mintswarm: approve-plan ID -->`, and answered with an
approval when that comment also opens with `<!-- mintswarm: plan approved -->`
(the session's record). An answered command is not checked again: it prints
nothing, and it counts as approved only when it was answered with an approval
and was posted after the current package, so a command refused earlier never
counts, whatever the state is now. Each unanswered command is checked in this
order, and the first check that fails is its reason:

    LOGIN is a bot                     (ends in [bot], or user type Bot)
    LOGIN is not a trusted approver
    #N is not awaiting plan approval (msw-state VALUE)
    #N has no current approval package
    it predates the current approval package
    it was edited after it was posted  (updated_at differs from created_at)
    the plan is already approved       (a valid command came before it)

An unanswered command prints `approve #N comment ID by LOGIN package PKG
sha256 SHA` or `refuse #N comment ID by LOGIN: REASON`.

When the value is `plan approved` and no mintswarm-dev[bot] comment opening
with `<!-- mintswarm: plan approved -->` (the session's record) follows the
current package, or exists at all when there is none, the value was set by
hand. Its actor is the actor of the latest `labeled` event for the
`plan approved` label in labels mode, and unknown in field mode, whose actor
cannot be read. The first reason that applies:

    who set it cannot be read; comment /approve-plan instead
    it was set without a recorded approval   (set by mintswarm-dev[bot])
    LOGIN is not a trusted approver
    #N has no current approval package
    it predates the current approval package

prints `set-back #N by LOGIN: REASON` (LOGIN `unknown` when it cannot be read);
otherwise it prints `hand-set #N by LOGIN package PKG sha256 SHA`.

Per issue, in the order given, it prints the approve line, the refuse lines in
comment order and then the hand-set or set-back line. Nothing is printed until
every issue has been read. Exits 0 when everything was read, with or without
findings; 1 when git, yq or a request fails; 2 on bad arguments or
configuration. Standard library only.
"""

import argparse
import importlib.util
import json
import re
import sys
from pathlib import Path

sys.dont_write_bytecode = True  # no __pycache__ next to the scripts it loads

HERE = Path(__file__).resolve().parent
BOT_LOGIN = "mintswarm-dev[bot]"
COMMAND = "/approve-plan"
PACKAGE = "<!-- mintswarm: plan approval package -->"
RECORD = "<!-- mintswarm: plan approved -->"
ANSWER = re.compile(r"<!-- mintswarm: approve-plan (\d+) -->")
SHA_LINE = re.compile(r"Plan sha256: ([0-9a-f]{64})")
AWAITING = "awaiting approval"
APPROVED = "plan approved"


def load(name: str, file: str):
    spec = importlib.util.spec_from_file_location(name, HERE / file)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


ms = load("msw_state", "msw-state.py")
aa = load("assign_approvers", "assign-approvers.py")


def lines(body) -> list:
    return [line.strip() for line in body.replace("\r\n", "\n").split("\n")] if isinstance(body, str) else []


def first_line(item: dict) -> str:
    got = lines(item.get("body"))
    return got[0] if got else ""


def login(item: dict) -> str:
    user = item.get("user") or item.get("actor")
    name = user.get("login") if isinstance(user, dict) else None
    return name if isinstance(name, str) and name else "unknown"


def is_bot(item: dict) -> bool:
    user = item.get("user") or item.get("actor")
    kind = user.get("type") if isinstance(user, dict) else None
    return login(item).lower().endswith("[bot]") or kind == "Bot"


def by_us(item: dict) -> bool:
    return login(item) == BOT_LOGIN


def order(item: dict) -> tuple:
    created, ident = item.get("created_at"), item.get("id")
    return (created if isinstance(created, str) else "", ident if isinstance(ident, int) else 0)


def package_of(comments: list):
    """The latest marked approval package, or None."""
    found = None
    for comment in comments:
        body = lines(comment.get("body"))
        if by_us(comment) and len(body) > 1 and body[0] == PACKAGE and SHA_LINE.fullmatch(body[1]):
            found = comment
    return found


def describe(package: dict) -> str:
    url = package.get("html_url") or package.get("id")
    return f"package {url} sha256 {SHA_LINE.fullmatch(lines(package['body'])[1]).group(1)}"


def answered(comments: list) -> dict:
    """Answered command ids, each mapped to whether an answer was the approval record."""
    ids = {}
    for comment in comments:
        if by_us(comment):
            record = first_line(comment) == RECORD
            for line in lines(comment.get("body")):
                if m := ANSWER.fullmatch(line):
                    ident = int(m.group(1))
                    ids[ident] = ids.get(ident, False) or record
    return ids


def commands(number: int, value: str, comments: list, package, trusted: set) -> list:
    done, approves, refusals, approved = answered(comments), [], [], False
    for comment in comments:
        if by_us(comment) or first_line(comment) != COMMAND:
            continue
        if comment.get("id") in done:
            if done[comment.get("id")] and package is not None and order(comment) > order(package):
                approved = True
            continue
        who = login(comment)
        if is_bot(comment):
            reason = f"{who} is a bot"
        elif who.lower() not in trusted:
            reason = f"{who} is not a trusted approver"
        elif value != AWAITING:
            reason = f"#{number} is not awaiting plan approval (msw-state {value})"
        elif package is None:
            reason = f"#{number} has no current approval package"
        elif order(comment) <= order(package):
            reason = "it predates the current approval package"
        elif comment.get("updated_at") != comment.get("created_at"):
            reason = "it was edited after it was posted"
        elif approved:
            reason = "the plan is already approved"
        else:
            reason, approved = None, True
        head = f"#{number} comment {comment.get('id')} by {who}"
        if reason is None:
            approves.append(f"approve {head} {describe(package)}")
        else:
            refusals.append(f"refuse {head}: {reason}")
    return approves + refusals


def hand_set(number: int, config: dict, comments: list, package, trusted: set) -> list:
    after = order(package) if package is not None else ("", -1)
    if any(by_us(c) and first_line(c) == RECORD and order(c) > after for c in comments):
        return []
    event = None
    if config["source"] == "labels":
        target = ms.label_of(APPROVED, config)
        events = ms.pages(f"repos/{{owner}}/{{repo}}/issues/{number}/events", number)
        for item in sorted((e for e in events if isinstance(e, dict)), key=order):
            label = (item.get("label") or {}).get("name")
            if item.get("event") == "labeled" and isinstance(label, str) and ms.same_label(label, target):
                event = item
    who = login(event) if event is not None else "unknown"
    if event is None or who == "unknown":
        reason = "who set it cannot be read; comment /approve-plan instead"
    elif by_us(event):
        reason = "it was set without a recorded approval"
    elif is_bot(event) or who.lower() not in trusted:
        reason = f"{who} is not a trusted approver"
    elif package is None:
        reason = f"#{number} has no current approval package"
    elif order(event) <= order(package):
        reason = "it predates the current approval package"
    else:
        return [f"hand-set #{number} by {who} {describe(package)}"]
    return [f"set-back #{number} by {who}: {reason}"]


def watch(number: int, config: dict, trusted: set) -> list:
    if config["source"] == "field":
        value = ms.read_field(number, config)
    else:
        value = ms.read_labels(number, config)[1]
    got = ms.pages(f"repos/{{owner}}/{{repo}}/issues/{number}/comments", number)
    comments = sorted((c for c in got if isinstance(c, dict)), key=order)
    package = package_of(comments)
    found = commands(number, value, comments, package, trusted)
    if value == APPROVED:
        found += hand_set(number, config, comments, package, trusted)
    return found


def main(argv: list) -> int:
    parser = argparse.ArgumentParser(description="Report /approve-plan commands and hand-set plan approved values.")
    parser.add_argument("issues", type=int, nargs="+", metavar="ISSUE", help="issue numbers")
    parser.add_argument("--base", default="origin/master", help="ref to read the config at (default origin/master)")
    args = parser.parse_args(argv)

    try:
        if any(number <= 0 for number in args.issues):
            raise ms.UsageError("issue numbers must be positive")
        config = ms.settings(args.base)
        trusted = {name.lower() for name in aa.approvers(aa.load_config(args.base))}
        found = [line for number in args.issues for line in watch(number, config, trusted)]
    except (ms.UsageError, aa.UsageError) as err:
        print(f"watch-plan-approvals.py: {err}", file=sys.stderr)
        return 2
    except (RuntimeError, json.JSONDecodeError, OSError) as err:
        print(f"watch-plan-approvals.py: {err}", file=sys.stderr)
        return 1
    if found:
        print("\n".join(found))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
