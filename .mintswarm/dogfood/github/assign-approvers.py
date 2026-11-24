#!/usr/bin/env python3
"""Assign an issue or PR to the people it waits on, release it, or name a PR's code owners.

    assign-approvers.py assign NUMBER pr [--base origin/master] (--paths P... | --branch B) [--dry-run]
    assign-approvers.py assign NUMBER plan|needs_input|external_steps [--dry-run]
    assign-approvers.py release NUMBER REASON [--dry-run]
    assign-approvers.py owners [--base origin/master] (--paths P... | --branch B)

Dogfooding stand-in for the control plane's "waiting on human" assignee. NUMBER
is an issue or a PR: the issues endpoints serve both. REASON is one of plan,
needs_input, pr and external_steps. Every GitHub call goes through bot-gh.sh,
so as mintswarm-dev[bot].

Code owners decide a PR only. For `pr` and `owners` the approvers are a
minimal covering set of the user code owners of the touched paths, read from
CODEOWNERS at the base: --base is resolved once with `git rev-parse --verify`
in the current directory, and CODEOWNERS is the first of .github/CODEOWNERS,
CODEOWNERS and docs/CODEOWNERS present in that commit, read with `git show`,
never from the worktree or a branch. The paths are --paths, or those of
`git diff --name-only --no-renames BASE...B` for --branch; one of the two is
required, and an empty diff exits 2.

CODEOWNERS follows GitHub's rules: each line is a pattern and its owners; a
path takes the owners of the last rule that matches it, and a rule with no
owners leaves its paths unowned. A pattern with a leading `/`, or with a `/`
anywhere but at its end, is anchored to the root; otherwise it matches at any
depth. `*` matches within one path segment, `?` one character other than `/`,
`**/` zero or more directories and a trailing `/**` everything below; a
pattern matches a file or anything under a directory, and one ending in `/`
only what is under a directory. `\\#` is a literal `#`, and an owner starting
with `#` starts a comment. Negation (`!`), character classes (`[...]`), other
`\\` escapes and owners that are not `@login`, `@org/team` or an e-mail
address exit 2, naming the line. A path ending in `/` is not a file path and
exits 2.

User owners have their `@` stripped and must match ^[A-Za-z0-9-]+$; logins are
compared without regard to case, as GitHub does, and keep the spelling of their
first appearance in CODEOWNERS. A `[bot]` login is skipped, and team and e-mail
owners are not assigned; each gets a note on stderr. Every path whose rule
names users belongs to the approval group of those users, and paths with the
same users share a group. The approvers are chosen by a greedy set cover:
repeatedly the login in the most groups not yet covered, ties going to the
lowest login in lower case, until every group has one. The result depends on
nothing but the paths and CODEOWNERS at the base, so `owners`, `assign ... pr`
and request-review.py choose the same people. A path with no code owner exits
2, naming the paths; a path whose owners are only teams, e-mails or bots is
noted and left out of the cover, and when no path has a user owner the script
exits 2. `owners` prints the approvers, one per line, and makes no GitHub call.

plan, needs_input and external_steps wait on the tracker item's owner: the
issue's author, unless a bot filed it (user.type Bot, or a login ending in
`[bot]`); then the author of the nearest ancestor a person filed, followed
through GitHub's sub-issue `repos/{owner}/{repo}/issues/N/parent`. An issue
with no parent (HTTP 404), or a parent already visited, exits 2. --paths and
--branch are refused for these reasons and for release; --base is accepted and
not used.

assign reads the item first, and only the approvers not already assigned are
posted with
`bot-gh.sh api --method POST repos/{owner}/{repo}/issues/N/assignees --input -`,
so a person's own assignment gets no event of the bot's. GitHub drops logins
it cannot assign (and any over its 10-assignee cap) without an error, so a
posted login missing from the response is named and the script fails.

release takes the item's current assignees and its event history
(`--paginate --slurp`), sorted by created_at and then id (events without them
keep their order). For each assignee, the latest `assigned` event for that
login decides: it is the bot's when actor.login or assigner.login is
mintswarm-dev[bot], or performed_via_github_app.slug is mintswarm-dev. Only
those assignees are removed, with
`bot-gh.sh api --method DELETE repos/{owner}/{repo}/issues/N/assignees --input -`;
an assignment a person made, or one with no assigned event, is left alone.
It keeps no state.

With --dry-run, assign prints the approvers and release the logins it would
remove, one per line, and nothing is changed on GitHub.

Exits 0 on success or a no-op; 1 when git or a request fails or GitHub does
not do what was asked; 2 on bad arguments, CODEOWNERS, paths or logins, or
when nobody is left to assign. Standard library only.
"""

import argparse
import json
import re
import subprocess
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
BOT_GH = HERE / "bot-gh.sh"
LOCATIONS = (".github/CODEOWNERS", "CODEOWNERS", "docs/CODEOWNERS")
LOGIN = re.compile(r"^[A-Za-z0-9-]+$")
BOT_OWNER = re.compile(r"^@[A-Za-z0-9-]+\[bot\]$", re.IGNORECASE)
TEAM = re.compile(r"^@[A-Za-z0-9-]+/[A-Za-z0-9._-]+$")
EMAIL = re.compile(r"^[^@\s]+@[^@\s]+$")
REASONS = ("plan", "needs_input", "pr", "external_steps")
BOT_LOGIN = "mintswarm-dev[bot]"
BOT_SLUG = "mintswarm-dev"


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


def note(text: str) -> None:
    print(f"assign-approvers.py: {text}", file=sys.stderr)


# --- CODEOWNERS ----------------------------------------------------------------


def resolve_base(ref: str) -> str:
    try:
        return run(["git", "rev-parse", "--verify", "--quiet", "--end-of-options", f"{ref}^{{commit}}"]).strip()
    except RuntimeError as exc:
        raise UsageError(f"base ref {ref!r} does not resolve to a commit") from exc


def read_codeowners(commit: str):
    """The location and text of the first CODEOWNERS present at COMMIT, or (None, "")."""
    for location in LOCATIONS:
        probe = subprocess.run(["git", "cat-file", "-e", f"{commit}:{location}"], capture_output=True)
        if probe.returncode == 0:
            return location, run(["git", "show", f"{commit}:{location}"])
    return None, ""


def pattern_regex(pattern: str) -> re.Pattern:
    anchored = pattern.startswith("/")
    body = pattern[1:] if anchored else pattern
    directory = body.endswith("/")
    core = body[:-1] if directory else body
    if not core:
        raise ValueError("an empty pattern")
    anchored = anchored or "/" in core
    out = [] if anchored else ["(?:.*/)?"]
    i = 0
    while i < len(core):
        if core.startswith("**/", i):
            out.append("(?:.*/)?")
            i += 3
        elif core[i:] == "/**":
            out.append("/.*")
            i += 3
        elif core[i] == "*":
            out.append("[^/]*")
            i += 1
        elif core[i] == "?":
            out.append("[^/]")
            i += 1
        else:
            out.append(re.escape(core[i]))
            i += 1
    out.append("/.*" if directory else "(?:/.*)?")
    return re.compile("".join(out), re.DOTALL)


def parse_rules(text: str, location: str) -> list:
    """The rules of a CODEOWNERS file, in order: (regex, owner tokens)."""
    rules = []
    for number, line in enumerate(text.splitlines(), 1):
        stripped = line.strip()
        if not stripped or stripped.startswith("#"):
            continue
        where = f"{location}:{number}: {stripped!r}"
        tokens = stripped.split()
        raw = tokens[0]
        owners = []
        for token in tokens[1:]:
            if token.startswith("#"):
                break
            if not (BOT_OWNER.match(token) or TEAM.match(token) or EMAIL.match(token)):
                if not (token.startswith("@") and LOGIN.match(token[1:])):
                    raise UsageError(f"{where}: not an owner: {token!r}")
            owners.append(token)
        if raw.startswith("!") or "[" in raw or "]" in raw:
            raise UsageError(f"{where}: unsupported pattern {raw!r}")
        if re.search(r"\\(?!#)", raw):
            raise UsageError(f"{where}: unsupported escape in {raw!r}")
        try:
            regex = pattern_regex(raw.replace("\\#", "#"))
        except ValueError as exc:
            raise UsageError(f"{where}: {exc}") from exc
        rules.append((regex, owners))
    return rules


def path_owners(rules: list, path: str) -> list:
    owners = []
    for regex, rule_owners in rules:
        if regex.fullmatch(path):
            owners = rule_owners
    return owners


def changed_paths(commit: str, branch: str) -> list:
    out = run(["git", "diff", "--name-only", "--no-renames", f"{commit}...{branch}"])
    paths = [line for line in out.splitlines() if line]
    if not paths:
        raise UsageError(f"git diff {commit}...{branch} names no changed path")
    return paths


def approval_groups(base: str, paths: list = None, branch: str = None):
    """The approval groups of the touched paths, and the spelling of each login."""
    commit = resolve_base(base)
    if branch is not None:
        paths = changed_paths(commit, branch)
    for path in paths:
        if not path or path.endswith("/"):
            raise UsageError(f"not a file path: {path!r}")
    location, text = read_codeowners(commit)
    if location is None:
        raise UsageError(f"no CODEOWNERS at {base} ({', '.join(LOCATIONS)}); no code owner for: {' '.join(paths)}")
    rules = parse_rules(text, location)
    spelling = {}
    for _, owners in rules:
        for owner in owners:
            login = owner[1:]
            if owner.startswith("@") and LOGIN.match(login):
                spelling.setdefault(login.lower(), login)
    owned = {path: path_owners(rules, path) for path in paths}
    unowned = sorted(path for path, owners in owned.items() if not owners)
    if unowned:
        raise UsageError(f"no code owner in {location} at {base} for: {' '.join(unowned)}")
    groups = set()
    noted = set()
    teamless = []
    for path in sorted(owned):
        users = set()
        for owner in owned[path]:
            login = owner[1:]
            if owner.startswith("@") and LOGIN.match(login):
                users.add(login.lower())
                continue
            if owner.lower() not in noted:
                noted.add(owner.lower())
                if BOT_OWNER.match(owner):
                    note(f"skipping {login}: a bot is never assigned")
                elif TEAM.match(owner):
                    note(f"not assigning team {owner}: teams are not assigned")
                else:
                    note(f"not assigning e-mail owner {owner}: only users are assigned")
        if users:
            groups.add(frozenset(users))
        else:
            teamless.append(path)
    if teamless:
        note(f"no user code owner for {' '.join(teamless)}; left out of the cover")
    if not groups:
        raise UsageError(f"no user code owner left to assign for: {' '.join(sorted(paths))}")
    return groups, spelling


def cover(groups: set) -> dict:
    """A greedy set cover of GROUPS: each chosen login, lower case, with the groups it covers."""
    uncovered = set(groups)
    chosen = {}
    while uncovered:
        counts = {}
        for group in uncovered:
            for login in group:
                counts[login] = counts.get(login, 0) + 1
        login = min(counts, key=lambda name: (-counts[name], name))
        chosen[login] = sorted(sorted(g) for g in uncovered if login in g)
        uncovered = {g for g in uncovered if login not in g}
    return chosen


def code_owners(base: str, paths: list = None, branch: str = None) -> list:
    groups, spelling = approval_groups(base, paths, branch)
    chosen = cover(groups)
    for login in sorted(chosen):
        for group in chosen[login]:
            note(f"{spelling[login]} covers the group {' '.join(spelling[g] for g in group)}")
    return [spelling[login] for login in sorted(chosen)]


# --- GitHub ------------------------------------------------------------------


def gh(args: list, body: dict = None, missing_ok: bool = False):
    argv = [str(BOT_GH), "api"] + args
    if body is not None:
        argv += ["--input", "-"]
    try:
        proc = subprocess.run(
            argv, input=json.dumps(body) if body is not None else None, capture_output=True, text=True
        )
    except OSError as exc:
        raise RuntimeError(f"cannot run {argv[0]}: {exc}") from exc
    if proc.returncode != 0:
        if missing_ok and "HTTP 404" in proc.stderr:
            return None
        raise RuntimeError(f"bot-gh.sh api failed: {proc.stderr.strip()}")
    return json.loads(proc.stdout or "null")


def assignee_logins(issue) -> list:
    if not isinstance(issue, dict):
        raise RuntimeError("GitHub returned no issue")
    return [a.get("login", "") for a in issue.get("assignees") or [] if isinstance(a, dict)]


def by_bot(event: dict) -> bool:
    actor = (event.get("actor") or {}).get("login")
    assigner = (event.get("assigner") or {}).get("login")
    app = (event.get("performed_via_github_app") or {}).get("slug")
    return BOT_LOGIN in (actor, assigner) or app == BOT_SLUG


def lowered(logins: list) -> set:
    return {login.lower() for login in logins}


def person(user) -> str:
    """USER's login when a person, else None."""
    if not isinstance(user, dict):
        return None
    login = user.get("login")
    if not isinstance(login, str) or user.get("type") == "Bot" or login.lower().endswith("[bot]"):
        return None
    return login


def item_owner(number: int, issue: dict) -> str:
    """The tracker item's owner: its author, or the nearest person-filed ancestor's."""
    login = person(issue.get("user"))
    seen = {number}
    current = number
    while login is None:
        parent = gh([f"repos/{{owner}}/{{repo}}/issues/{current}/parent"], missing_ok=True)
        if not isinstance(parent, dict):
            raise UsageError(f"#{number} was filed by a bot and no person-filed ancestor was found")
        current = parent.get("number")
        if not isinstance(current, int) or current in seen:
            raise UsageError(f"#{number}: the parent chain loops at #{current}; no person-filed ancestor was found")
        seen.add(current)
        login = person(parent.get("user"))
    if not LOGIN.fullmatch(login):
        raise UsageError(f"#{current}'s author is not a valid login: {login!r}")
    if current != number:
        note(f"#{number} was filed by a bot; waiting on #{current}'s author")
    return login


def assign(number: int, reason: str, base: str, paths: list, branch: str, dry_run: bool) -> int:
    issue = None
    if reason == "pr":
        if paths is None and branch is None:
            raise UsageError("pr needs --paths or --branch")
        logins = code_owners(base, paths, branch)
    else:
        if paths is not None or branch is not None:
            raise UsageError(f"{reason} takes no --paths or --branch: it waits on the item's owner")
        issue = gh([f"repos/{{owner}}/{{repo}}/issues/{number}"])
        if not isinstance(issue, dict):
            raise RuntimeError("GitHub returned no issue")
        logins = [item_owner(number, issue)]
    if dry_run:
        print("\n".join(logins))
        return 0
    if issue is None:
        issue = gh([f"repos/{{owner}}/{{repo}}/issues/{number}"])
    current = lowered(assignee_logins(issue))
    already = [login for login in logins if login.lower() in current]
    todo = [login for login in logins if login.lower() not in current]
    if already:
        note(f"already assigned to #{number}: {' '.join(already)}")
    if not todo:
        print(f"nothing to assign: #{number} is assigned to every approver ({reason})")
        return 0
    issue = gh(["--method", "POST", f"repos/{{owner}}/{{repo}}/issues/{number}/assignees"], {"assignees": todo})
    missing = [login for login in todo if login.lower() not in lowered(assignee_logins(issue))]
    if missing:
        note(f"GitHub did not assign {' '.join(missing)} to #{number} (not assignable, or over the 10-assignee cap)")
        return 1
    print(f"assigned {' '.join(todo)} to #{number} ({reason})")
    return 0


def event_order(event: dict) -> tuple:
    created = event.get("created_at")
    ident = event.get("id")
    return (created if isinstance(created, str) else "", ident if isinstance(ident, int) else 0)


def release(number: int, reason: str, dry_run: bool) -> int:
    current = assignee_logins(gh([f"repos/{{owner}}/{{repo}}/issues/{number}"]))
    pages = gh(["--paginate", "--slurp", f"repos/{{owner}}/{{repo}}/issues/{number}/events"]) or []
    events = []
    for page in pages:
        events.extend(page if isinstance(page, list) else [page])
    latest = {}
    assigned = [e for e in events if isinstance(e, dict) and e.get("event") == "assigned"]
    for event in sorted(assigned, key=event_order):
        latest[((event.get("assignee") or {}).get("login") or "").lower()] = event
    mine = []
    for login in current:
        event = latest.get(login.lower())
        if event is None:
            note(f"leaving {login} alone: no assigned event")
        elif not by_bot(event):
            who = (event.get("actor") or {}).get("login") or "someone else"
            note(f"leaving {login} alone: assigned by {who}")
        else:
            mine.append(login)
    mine = sorted(set(mine))
    if not mine:
        print("nothing to release")
        return 0
    if dry_run:
        print("\n".join(mine))
        return 0
    issue = gh(["--method", "DELETE", f"repos/{{owner}}/{{repo}}/issues/{number}/assignees"], {"assignees": mine})
    left = [login for login in mine if login.lower() in lowered(assignee_logins(issue))]
    if left:
        note(f"GitHub did not remove {' '.join(left)} from #{number}")
        return 1
    print(f"released {' '.join(mine)} from #{number} ({reason})")
    return 0


def add_paths(parser: argparse.ArgumentParser, required: bool) -> None:
    group = parser.add_mutually_exclusive_group(required=required)
    group.add_argument("--paths", nargs="+", help="the touched paths")
    group.add_argument("--branch", help="take the paths from git diff BASE...BRANCH")


def main(argv: list) -> int:
    parser = argparse.ArgumentParser(description="Assign an item's approvers, release them, or name the code owners.")
    actions = parser.add_subparsers(dest="action", required=True)
    base_help = "ref to read CODEOWNERS at (default origin/master)"
    for name in ("assign", "release"):
        sub = actions.add_parser(name)
        sub.add_argument("number", type=int, help="issue or pull request number")
        sub.add_argument("reason", help="one of " + ", ".join(REASONS))
        sub.add_argument("--base", default="origin/master", help=base_help)
        if name == "assign":
            add_paths(sub, required=False)
        sub.add_argument("--dry-run", action="store_true", help="print the logins, change nothing")
    sub = actions.add_parser("owners")
    sub.add_argument("--base", default="origin/master", help=base_help)
    add_paths(sub, required=True)
    args = parser.parse_args(argv)

    try:
        if args.action == "owners":
            print("\n".join(code_owners(args.base, args.paths, args.branch)))
            return 0
        if args.number <= 0:
            raise UsageError("NUMBER must be a positive number")
        if args.reason not in REASONS:
            raise UsageError(f"REASON must be one of {', '.join(REASONS)}, not {args.reason!r}")
        if args.action == "assign":
            return assign(args.number, args.reason, args.base, args.paths, args.branch, args.dry_run)
        return release(args.number, args.reason, args.dry_run)
    except UsageError as err:
        note(str(err))
        return 2
    except (RuntimeError, json.JSONDecodeError, OSError) as err:
        note(str(err))
        return 1


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
