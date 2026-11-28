#!/usr/bin/env python3
"""Report plan approvals and other commands given on the tracker, with a verdict on each; never write.

    watch-plan-approvals.py ISSUE... [--base origin/master]
    watch-plan-approvals.py --commands CONTEXT

Dogfooding stand-in for the control plane's handler of `/approve-plan`, of
`/help` and of a hand-set `plan approved` msw-state
(`.mintswarm/dogfood/plan-approval/README.md`, "Approval from the tracker").
The session's monitor runs it every polling interval and acts on each line it
prints (Session procedure, step 14, in this directory's README). It only reads;
every GitHub call goes through bot-gh.sh, and the session posts every reaction
and reply.

It reads tracker.state with msw-state.py's settings() at the base ref (--base,
resolved in the current directory). An issue's one trusted approver is its
owner, the person a waiting plan is assigned to: assign-approvers.py's
issue_owner() reads it, the issue's author, or the author of the nearest
ancestor a person filed when a bot filed it, so never a `[bot]` login. The
owner is trusted only when GitHub reports write access or higher for them
(`repos/{owner}/{repo}/collaborators/LOGIN/permission`: permission admin or
write, or role_name admin, maintain or write). When no owner can be found (a
bot filed the issue and no ancestor a person filed), or the owner has no
write access, the issue has no trusted approver, and the reason goes to
stderr. Logins are compared case-insensitively. For each ISSUE it reads the
owner and their permission, the msw-state value (msw-state.py's read_field or
read_labels), the comments (`--paginate --slurp
repos/{owner}/{repo}/issues/N/comments`) and, in labels mode, the events (the
same for `issues/N/events`). Comments and events are ordered by created_at,
then id.

Our comments are the bee's: the runner App's login, `<slug>[bot]`, derived
from its config ($MSW_APP_CONFIG or ~/.config/mintswarm/bee-app.json) by
assign-approvers.py's bee_login(), read at the first comparison and never at
import; a missing or malformed config fails with exit 1 (README.md, Roles).

The current approval package is the latest comment by the bee whose
first line is `<!-- mintswarm: plan approval package -->` and whose second is
`Plan sha256: <64 hex digits>`; PKG is its html_url and SHA that digest. A
comment's first line is taken with CRLF normalised and whitespace trimmed.

A comment's command is its first command line: a line that, with CRLF
normalised and whitespace trimmed, is exactly one token `/[A-Za-z][A-Za-z0-9-]*`,
outside a `>` quote, a ``` or ~~~ fenced block and an indented code block (a
line starting with 4 spaces or a tab). A non-blank line right after a `>` line
with text, up to the next blank line or fence, is the quote's lazy
continuation and is quoted too. Only that first command line counts; the
comment's other lines are a note. A line holding a token and more text, such as
`/approve-plan please` or `/tmp/x`, is not a command line. Comments by
the bee have no command.

A comment whose command is `/approve-plan` is an approval command. It is
answered when a comment by the bee holds a line
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

An unanswered approval command prints `approve #N comment ID by LOGIN package PKG
sha256 SHA` or `refuse #N comment ID by LOGIN: REASON`.

When the value is `plan approved` and no comment by the bee opening
with `<!-- mintswarm: plan approved -->` (the session's record) follows the
current package, or exists at all when there is none, the value was set by
hand. Its actor is the actor of the latest `labeled` event for the
`plan approved` label in labels mode, and unknown in field mode, whose actor
cannot be read. The first reason that applies:

    who set it cannot be read; comment /approve-plan instead
    it was set without a recorded approval   (set by the bee)
    LOGIN is not a trusted approver
    #N has no current approval package
    it predates the current approval package

prints `set-back #N by LOGIN: REASON` (LOGIN `unknown` when it cannot be read);
otherwise it prints `hand-set #N by LOGIN package PKG sha256 SHA`.

Any other command gets a reply, once: a help or hint is answered when a
comment by the bee holds a line that is exactly
`<!-- mintswarm: command-reply ID -->`, and an answered one prints nothing.
Comments by bots get no reply and print nothing. An unanswered `/help` prints
`help #N comment ID by LOGIN commands CONTEXT`; any other token prints
`hint #N comment ID by LOGIN command /X suggest /Y commands CONTEXT`. /Y is the
command valid in CONTEXT (as `--commands CONTEXT` lists them, `/help`
included) whose hyphen-separated words, without a leading `/`, are those of /X
in any order and case, else the closest one by difflib.get_close_matches with
cutoff 0.6 on the lowercased names without `/`, else `none`; so `/approve`
under a decomposition package suggests `approve`. CONTEXT is `plan`,
`decomposition` or `design` when the msw-state is `awaiting approval` and the
latest comment by the bee opening with
`<!-- mintswarm: plan approval package -->`,
`<!-- mintswarm: decomposition approval package -->` or
`<!-- mintswarm: design approval package -->` is of that kind, and `none`
otherwise.

Every comment it acts on, an unanswered approval command (approved or refused)
and an unanswered `/help`, first gets `react #N comment ID`: the session adds
a +1 reaction to it. A hint gets no reaction.

`--commands CONTEXT` prints the Commands section of `plan`, `decomposition`,
`design` or `none` and makes no GitHub call: a `<details><summary>Commands</summary>`
block listing each command valid in that context in a code span, with one line
on where a command counts. The packages and the help and hint replies end with
it. An unknown CONTEXT, or `--commands` with issues, exits 2.

Per issue, in the order given, it prints the react lines, the approve line,
the refuse lines in comment order, the help and hint lines in comment order
and then the hand-set or set-back line. Nothing is printed until
every issue has been read. Exits 0 when everything was read, with or without
findings; 1 when git, yq or a request fails; 2 on bad arguments or
configuration. Standard library only.
"""

import argparse
import difflib
import importlib.util
import json
import re
import sys
from pathlib import Path

sys.dont_write_bytecode = True  # no __pycache__ next to the scripts it loads

HERE = Path(__file__).resolve().parent
COMMAND = "/approve-plan"
HELP = "/help"
PACKAGE = "<!-- mintswarm: plan approval package -->"
DECOMPOSITION = "<!-- mintswarm: decomposition approval package -->"
DESIGN = "<!-- mintswarm: design approval package -->"
# The context each package marker puts an issue in while it awaits approval.
CONTEXTS = {PACKAGE: "plan", DECOMPOSITION: "decomposition", DESIGN: "design"}
RECORD = "<!-- mintswarm: plan approved -->"
ANSWER = re.compile(r"<!-- mintswarm: approve-plan (\d+) -->")
REPLY = re.compile(r"<!-- mintswarm: command-reply (\d+) -->")
TOKEN = re.compile(r"/[A-Za-z][A-Za-z0-9-]*")
FENCE = re.compile(r" {0,3}(`{3,}|~{3,})")
SHA_LINE = re.compile(r"Plan sha256: ([0-9a-f]{64})")
AWAITING = "awaiting approval"
APPROVED = "plan approved"


HELP_TEXT = "reply with the commands valid on this issue now"
FIRST_LINE = "as the comment's first line"
# The commands valid in each context, in the order the Commands section lists them.
COMMANDS = {
    "plan": [
        (COMMAND, "approve the plan in the latest approval package; the comment's other lines are a note"),
        (HELP, HELP_TEXT),
    ],
    "decomposition": [
        ("approve", f"approve the decomposition, {FIRST_LINE}"),
        ("changes", f"ask for changes, {FIRST_LINE}, with what to change on the lines after it"),
        ("reject", f"reject the decomposition, {FIRST_LINE}"),
        (HELP, HELP_TEXT),
    ],
    "design": [
        ("approve", f"approve the design, {FIRST_LINE}, citing the sha256 of the current design"),
        ("changes", f"ask for changes, {FIRST_LINE}, with what to change on the lines after it"),
        ("reject", f"reject the design, {FIRST_LINE}"),
        (HELP, HELP_TEXT),
    ],
    "none": [(HELP, HELP_TEXT)],
}
RULE = (
    "A `/` command counts only when it stands alone at the start of a line, outside quotes and code;"
    " only the first one in a comment counts."
)


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
    return login(item) == aa.bee_login()


def order(item: dict) -> tuple:
    created, ident = item.get("created_at"), item.get("id")
    return (created if isinstance(created, str) else "", ident if isinstance(ident, int) else 0)


def command_of(comment: dict):
    """The first command token alone on a line outside quotes, fences and indented code, or None.

    A non-blank line right after a quote line with text is the quote's lazy
    continuation, so it is quoted too, until a blank line or a fence.
    """
    body = comment.get("body")
    if not isinstance(body, str):
        return None
    fence, quoted = None, False
    for raw in body.replace("\r\n", "\n").split("\n"):
        if fence is not None:
            if re.fullmatch(r" {0,3}" + re.escape(fence[0]) + "{%d,}" % len(fence), raw.rstrip()):
                fence = None
            continue
        if not raw.strip():
            quoted = False
            continue
        if m := FENCE.match(raw):
            fence, quoted = m.group(1), False
            continue
        if raw.lstrip().startswith(">"):
            quoted = bool(raw.lstrip().lstrip(">").strip())
            continue
        if quoted or raw.startswith(("    ", "\t")):
            continue
        if TOKEN.fullmatch(raw.strip()):
            return raw.strip()
    return None


def suggest(token: str, where: str) -> str:
    """The command valid in context WHERE that TOKEN was probably meant as, or none.

    Names are compared without their leading `/`, so `/approve` finds `approve`.
    """
    known = {name.lstrip("/").lower(): name for name, _ in COMMANDS[where]}
    words = sorted(token.lower()[1:].split("-"))
    for bare, name in known.items():
        if sorted(bare.split("-")) == words:
            return name
    close = difflib.get_close_matches(token.lower()[1:], list(known), n=1, cutoff=0.6)
    return known[close[0]] if close else "none"


def context(value: str, comments: list) -> str:
    """plan, decomposition or design when awaiting approval of the latest marked package of that kind, else none."""
    found = "none"
    if value == AWAITING:
        for comment in comments:
            if by_us(comment) and first_line(comment) in CONTEXTS:
                found = CONTEXTS[first_line(comment)]
    return found


def render(name: str) -> str:
    """The Commands section of context NAME, for the packages and the help and hint replies."""
    rows = [f"- `{command}`: {text}." for command, text in COMMANDS[name]]
    return "\n".join(["<details><summary>Commands</summary>", "", *rows, "", RULE, "", "</details>"])


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


def answered(comments: list) -> tuple:
    """Answered command ids, each mapped to whether an answer was the approval record, and replied ids."""
    ids, replied = {}, set()
    for comment in comments:
        if by_us(comment):
            record = first_line(comment) == RECORD
            for line in lines(comment.get("body")):
                if m := ANSWER.fullmatch(line):
                    ident = int(m.group(1))
                    ids[ident] = ids.get(ident, False) or record
                elif m := REPLY.fullmatch(line):
                    replied.add(int(m.group(1)))
    return ids, replied


def commands(number: int, value: str, comments: list, package, trusted: set) -> list:
    (done, replied), where = answered(comments), context(value, comments)
    reacts, approves, refusals, replies, approved = [], [], [], [], False
    for comment in comments:
        token = command_of(comment) if not by_us(comment) else None
        if token is None:
            continue
        ident, who = comment.get("id"), login(comment)
        head = f"#{number} comment {ident} by {who}"
        if token != COMMAND:
            if ident in replied or is_bot(comment):
                continue
            if token == HELP:
                reacts.append(f"react #{number} comment {ident}")
                replies.append(f"help {head} commands {where}")
            else:
                replies.append(f"hint {head} command {token} suggest {suggest(token, where)} commands {where}")
            continue
        if ident in done:
            if done[comment.get("id")] and package is not None and order(comment) > order(package):
                approved = True
            continue
        reacts.append(f"react #{number} comment {ident}")
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
        if reason is None:
            approves.append(f"approve {head} {describe(package)}")
        else:
            refusals.append(f"refuse {head}: {reason}")
    return reacts + approves + refusals + replies


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


WRITE_PERMISSIONS = {"admin", "write"}
WRITE_ROLES = {"admin", "maintain", "write"}


def can_write(owner: str) -> bool:
    """Whether GitHub reports write, maintain or admin access to the repository for OWNER."""
    got = aa.gh([f"repos/{{owner}}/{{repo}}/collaborators/{owner}/permission"], missing_ok=True)
    if not isinstance(got, dict):
        return False
    return got.get("permission") in WRITE_PERMISSIONS or got.get("role_name") in WRITE_ROLES


def trusted_of(number: int) -> set:
    """The issue's trusted approvers: its owner when they may write, or nobody."""
    try:
        owner = aa.issue_owner(number)
    except aa.UsageError as err:
        print(f"watch-plan-approvals.py: {err}; #{number} has no trusted approver", file=sys.stderr)
        return set()
    if not can_write(owner):
        print(
            f"watch-plan-approvals.py: #{number}'s owner {owner} has no write access to the repository;"
            f" #{number} has no trusted approver",
            file=sys.stderr,
        )
        return set()
    return {owner.lower()}


def watch(number: int, config: dict) -> list:
    trusted = trusted_of(number)
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
    parser = argparse.ArgumentParser(description="Report tracker commands and hand-set plan approved values.")
    parser.add_argument("issues", type=int, nargs="*", metavar="ISSUE", help="issue numbers")
    parser.add_argument("--base", default="origin/master", help="ref to read the config at (default origin/master)")
    parser.add_argument("--commands", metavar="CONTEXT", help="print the Commands section of CONTEXT and exit")
    args = parser.parse_args(argv)

    try:
        if args.commands is not None:
            if args.issues:
                raise ms.UsageError("--commands takes no issues")
            if args.commands not in COMMANDS:
                raise ms.UsageError(f"unknown context {args.commands}; expected one of {', '.join(COMMANDS)}")
            print(render(args.commands))
            return 0
        if not args.issues:
            raise ms.UsageError("give at least one issue")
        if any(number <= 0 for number in args.issues):
            raise ms.UsageError("issue numbers must be positive")
        config = ms.settings(args.base)
        found = [line for number in args.issues for line in watch(number, config)]
    except ms.UsageError as err:
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
