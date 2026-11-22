#!/usr/bin/env python3
"""Assign an issue or PR to its approvers while a human is waited on, and release it.

    assign-approvers.py assign NUMBER REASON [--base origin/master] [--dry-run]
    assign-approvers.py release NUMBER REASON [--dry-run]

Dogfooding stand-in for the control plane's "waiting on human" assignee. NUMBER
is an issue or a PR: the issues endpoints serve both. REASON is one of plan,
needs_input, pr and external_steps; it is checked and printed, and selects
nothing. Every GitHub call goes through bot-gh.sh, so as mintswarm-dev[bot].

assign reads .mintswarm/config.yaml at the base ref (--base, resolved with
`git rev-parse --verify` in the current directory), never the worktree's copy:
`git show COMMIT:.mintswarm/config.yaml` piped to `yq -o=json . /dev/stdin`.
The approvers are pipeline.review.reviewers:

    pipeline:
      review: {reviewers: [octocat]}

A leading `@` is stripped. A login ending in `[bot]` is skipped with a note,
so no bot is ever assigned; every other login must match ^[A-Za-z0-9-]+$. The
logins are deduplicated and sorted, then posted with
`bot-gh.sh api --method POST repos/{owner}/{repo}/issues/N/assignees --input -`.
GitHub drops logins it cannot assign (and any over its 10-assignee cap)
without an error, so a requested login missing from the response is named
and the script fails.

release reads no config. It takes the item's current assignees and its event
history (`--paginate --slurp`, oldest first). For each assignee, the last
`assigned` event for that login decides: it is the bot's when actor.login or
assigner.login is mintswarm-dev[bot], or performed_via_github_app.slug is
mintswarm-dev. Only those assignees are removed, with
`bot-gh.sh api --method DELETE repos/{owner}/{repo}/issues/N/assignees --input -`;
an assignment a person made, or one with no assigned event, is left alone.
It keeps no state.

With --dry-run, assign prints the approvers and release the logins it would
remove, one per line, and nothing is changed on GitHub.

Exits 0 on success or a no-op; 1 when git, yq or a request fails or GitHub
does not do what was asked; 2 on bad arguments, configuration or logins.
Standard library only.
"""

import argparse
import json
import re
import subprocess
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
BOT_GH = HERE / "bot-gh.sh"
CONFIG = ".mintswarm/config.yaml"
LOGIN = re.compile(r"^[A-Za-z0-9-]+$")
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


def load_config(ref: str) -> dict:
    try:
        commit = run(["git", "rev-parse", "--verify", "--quiet", "--end-of-options", f"{ref}^{{commit}}"]).strip()
    except RuntimeError as exc:
        raise RuntimeError(f"base ref {ref!r} does not resolve to a commit") from exc
    text = run(["git", "show", f"{commit}:{CONFIG}"])
    config = json.loads(run(["yq", "-o=json", ".", "/dev/stdin"], text) or "null")
    if not isinstance(config, dict):
        raise UsageError(f"{CONFIG} at {ref} is not a mapping")
    return config


def approvers(config: dict) -> list:
    where = "pipeline.review.reviewers"
    pipeline = config.get("pipeline") or {}
    review = pipeline.get("review") if isinstance(pipeline, dict) else None
    value = review.get("reviewers") if isinstance(review, dict) else None
    if value is None:
        return []
    if not isinstance(value, list):
        raise UsageError(f"{where} must be a list of logins")
    result = set()
    for item in value:
        if not isinstance(item, str):
            raise UsageError(f"{where}: not a login: {item!r}")
        login = item[1:] if item.startswith("@") else item
        if login.lower().endswith("[bot]"):
            print(f"assign-approvers.py: skipping {login}: a bot is never assigned", file=sys.stderr)
            continue
        if not LOGIN.match(login):
            raise UsageError(f"{where}: not a valid login: {item!r}")
        result.add(login)
    return sorted(result)


def gh(args: list, body: dict = None):
    argv = [str(BOT_GH), "api"] + args
    if body is not None:
        argv += ["--input", "-"]
    return json.loads(run(argv, json.dumps(body) if body is not None else None) or "null")


def assignee_logins(issue) -> list:
    if not isinstance(issue, dict):
        raise RuntimeError("GitHub returned no issue")
    return [a.get("login", "") for a in issue.get("assignees") or [] if isinstance(a, dict)]


def by_bot(event: dict) -> bool:
    actor = (event.get("actor") or {}).get("login")
    assigner = (event.get("assigner") or {}).get("login")
    app = (event.get("performed_via_github_app") or {}).get("slug")
    return BOT_LOGIN in (actor, assigner) or app == BOT_SLUG


def assign(number: int, reason: str, base: str, dry_run: bool) -> int:
    logins = approvers(load_config(base))
    if not logins:
        print("no approvers resolved; nothing assigned", file=sys.stderr)
        return 0
    if dry_run:
        print("\n".join(logins))
        return 0
    issue = gh(["--method", "POST", f"repos/{{owner}}/{{repo}}/issues/{number}/assignees"], {"assignees": logins})
    missing = [login for login in logins if login not in assignee_logins(issue)]
    if missing:
        print(f"assign-approvers.py: GitHub did not assign {' '.join(missing)} to #{number}"
              " (not assignable, or over the 10-assignee cap)", file=sys.stderr)
        return 1
    print(f"assigned {' '.join(logins)} to #{number} ({reason})")
    return 0


def release(number: int, reason: str, dry_run: bool) -> int:
    current = assignee_logins(gh([f"repos/{{owner}}/{{repo}}/issues/{number}"]))
    pages = gh(["--paginate", "--slurp", f"repos/{{owner}}/{{repo}}/issues/{number}/events"]) or []
    events = []
    for page in pages:
        events.extend(page if isinstance(page, list) else [page])
    latest = {}
    for event in events:
        if isinstance(event, dict) and event.get("event") == "assigned":
            latest[(event.get("assignee") or {}).get("login")] = event
    mine = []
    for login in current:
        event = latest.get(login)
        if event is None:
            print(f"assign-approvers.py: leaving {login} alone: no assigned event", file=sys.stderr)
        elif not by_bot(event):
            who = (event.get("actor") or {}).get("login") or "someone else"
            print(f"assign-approvers.py: leaving {login} alone: assigned by {who}", file=sys.stderr)
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
    left = [login for login in mine if login in assignee_logins(issue)]
    if left:
        print(f"assign-approvers.py: GitHub did not remove {' '.join(left)} from #{number}", file=sys.stderr)
        return 1
    print(f"released {' '.join(mine)} from #{number} ({reason})")
    return 0


def main(argv: list) -> int:
    parser = argparse.ArgumentParser(description="Assign an item's approvers, or release them.")
    parser.add_argument("action", choices=("assign", "release"))
    parser.add_argument("number", type=int, help="issue or pull request number")
    parser.add_argument("reason", help="one of " + ", ".join(REASONS))
    parser.add_argument("--base", default="origin/master", help="ref to read the config at (default origin/master)")
    parser.add_argument("--dry-run", action="store_true", help="print the logins, change nothing")
    args = parser.parse_args(argv)

    try:
        if args.number <= 0:
            raise UsageError("NUMBER must be a positive number")
        if args.reason not in REASONS:
            raise UsageError(f"REASON must be one of {', '.join(REASONS)}, not {args.reason!r}")
        if args.action == "assign":
            return assign(args.number, args.reason, args.base, args.dry_run)
        return release(args.number, args.reason, args.dry_run)
    except UsageError as err:
        print(f"assign-approvers.py: {err}", file=sys.stderr)
        return 2
    except (RuntimeError, json.JSONDecodeError, OSError) as err:
        print(f"assign-approvers.py: {err}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
