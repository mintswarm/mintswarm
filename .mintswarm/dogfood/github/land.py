#!/usr/bin/env python3
"""Land one mintswarm PR as mintswarm-dev-steward, at the head the maintainer approved.

    land.py PR --sha SHA [--dry-run]

The session runs it when the maintainer approves a PR in the session
(README.md, "Session procedure", step 12). It acts only as
mintswarm-dev-steward: the App config is $MSW_STEWARD_CONFIG, or
~/.config/mintswarm/steward-app.json when that is unset, and it must name the
slug mintswarm-dev-steward and the key service
mintswarm-dev-steward-private-key, or the script exits 1 before anything is
minted. The key is read from the key store recorded in that config, without
per-use confirmation, so the script needs no GUI session and asks nobody
anything: it works on a remote runner.

Token: one installation token is minted with mint-token.sh token (with
MSW_APP_CONFIG set to the steward's config) and the permissions contents and
pull_requests write, checks, statuses and metadata read. It must start with
ghs_. It is held in a variable and passed to every bot-gh.sh call as MSW_TOKEN
in the child's environment only; it is never printed and never in any argv.

The approval steer is a comment on the PR by mintswarm-dev[bot], posted by the
session before this script runs, with a line exactly

    <!-- mintswarm: steer -->

and a line exactly

    Approved head: SHA

besides the maintainer's quoted words and the time.

Checks, all made with the steward's token; every failure is collected and
printed on stderr, and nothing is merged unless all of them hold:

- pulls/PR: open, not a draft, base master, head ref mintswarm/*, head repo
  mintswarm/mintswarm, head sha equal to SHA; no label named hold; mergeable
  true and mergeable_state not dirty. While mergeable is null on an open PR,
  pulls/PR is read again up to 5 times, $MSW_LAND_RETRY_DELAY seconds apart
  (default 3); after that it refuses.
- commits/SHA/check-runs?filter=latest, every page: at least one run named
  gates-matrix, and every run completed with conclusion success.
- commits/SHA/status: every status success, and no more than the 100 one page
  holds.
- GraphQL pullRequest.reviewThreads, paged: no unresolved thread.
- issues/PR/comments, every page: an approval steer naming SHA.

When all hold and --dry-run is not given, it calls
PUT repos/{owner}/{repo}/pulls/PR/merge with {"merge_method": "squash",
"sha": SHA, "commit_title": "TITLE (#PR)"}, so GitHub refuses the merge if the
head moved after the checks, then reads pulls/PR again and prints
merge_commit_sha and merged_by.

Exit codes: 0 when merged, or when --dry-run passes every check; 1 on any
refusal or GitHub failure, including a refused merge call, which is what
removing the steward from the master ruleset's bypass list (the kill switch)
causes; 2 on bad arguments. Standard library only.
"""

import argparse
import json
import os
import re
import subprocess
import sys
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent
BOT_GH = HERE / "bot-gh.sh"
MINT_TOKEN = HERE / "mint-token.sh"

REPO = "mintswarm/mintswarm"
OWNER, NAME = REPO.split("/")
BASE = "master"
BRANCH_PREFIX = "mintswarm/"
STEWARD_SLUG = "mintswarm-dev-steward"
STEWARD_SERVICE = "mintswarm-dev-steward-private-key"
BOT_LOGIN = "mintswarm-dev[bot]"
HOLD_LABEL = "hold"
REQUIRED_CHECK = "gates-matrix"
STEER_MARKER = "<!-- mintswarm: steer -->"
PERMISSIONS = {
    "contents": "write",
    "pull_requests": "write",
    "checks": "read",
    "statuses": "read",
    "metadata": "read",
}
MERGEABLE_RETRIES = 5
SHA_RE = re.compile(r"^[0-9a-f]{40}$")

THREADS_QUERY = """
query($owner: String!, $name: String!, $number: Int!, $after: String) {
  repository(owner: $owner, name: $name) {
    pullRequest(number: $number) {
      reviewThreads(first: 100, after: $after) {
        nodes { isResolved }
        pageInfo { hasNextPage endCursor }
      }
    }
  }
}
"""


class UsageError(Exception):
    pass


class Refused(Exception):
    pass


def steward_config() -> Path:
    path = Path(os.environ.get("MSW_STEWARD_CONFIG") or Path.home() / ".config" / "mintswarm" / "steward-app.json")
    if not path.is_file():
        raise Refused(f"no steward config at {path} (see README.md, Steward)")
    try:
        config = json.loads(path.read_text())
    except (OSError, json.JSONDecodeError) as err:
        raise Refused(f"cannot read {path}: {err}")
    if not isinstance(config, dict):
        raise Refused(f"{path} is not a JSON object")
    if config.get("slug") != STEWARD_SLUG:
        raise Refused(
            f"{path} names the App {config.get('slug')!r}, not {STEWARD_SLUG}: land.py acts only as the steward"
        )
    if config.get("keychain_service") != STEWARD_SERVICE:
        raise Refused(
            f"{path} gives the key service {config.get('keychain_service')!r}, not {STEWARD_SERVICE}: "
            "land.py uses only the steward's key"
        )
    return path


def mint(config: Path) -> str:
    env = dict(os.environ)
    env.pop("MSW_TOKEN", None)
    env["MSW_APP_CONFIG"] = str(config)
    proc = subprocess.run(
        [str(MINT_TOKEN), "token", json.dumps(PERMISSIONS, separators=(",", ":"))],
        env=env,
        capture_output=True,
        text=True,
    )
    if proc.returncode != 0:
        raise RuntimeError(f"minting the steward's token failed: {proc.stderr.strip()}")
    token = proc.stdout.strip()
    if not token.startswith("ghs_") or len(token) <= 4:
        raise RuntimeError("mint-token.sh gave no installation token (expected ghs_...)")
    return token


class Steward:
    def __init__(self, config: Path, token: str):
        self.env = dict(os.environ)
        self.env["MSW_APP_CONFIG"] = str(config)
        self.env["MSW_TOKEN"] = token

    def gh(self, args: list, body=None):
        if body is not None:
            args = args + ["--input", "-"]
        proc = subprocess.run(
            [str(BOT_GH), "api"] + args,
            env=self.env,
            input=json.dumps(body) if body is not None else None,
            capture_output=True,
            text=True,
        )
        if proc.returncode != 0:
            raise RuntimeError(f"gh api {' '.join(args)} failed: {proc.stderr.strip()}")
        return json.loads(proc.stdout or "null")

    def pages(self, path: str) -> list:
        pages = self.gh(["--paginate", "--slurp", path])
        if not isinstance(pages, list):
            raise RuntimeError(f"gh api {path} gave no pages")
        return pages


def pull(steward: Steward, pr: int) -> dict:
    data = steward.gh([f"repos/{{owner}}/{{repo}}/pulls/{pr}"])
    if not isinstance(data, dict):
        raise RuntimeError(f"pulls/{pr} is not an object")
    return data


def pull_settled(steward: Steward, pr: int, delay: float) -> dict:
    data = pull(steward, pr)
    for _ in range(MERGEABLE_RETRIES):
        if data.get("mergeable") is not None or data.get("state") != "open":
            break
        time.sleep(delay)
        data = pull(steward, pr)
    return data


def check_pull(data: dict, sha: str) -> list:
    reasons = []
    head = data.get("head") or {}
    base = data.get("base") or {}
    head_repo = (head.get("repo") or {}).get("full_name")
    if data.get("state") != "open":
        reasons.append(f"the PR is {data.get('state')}, not open")
    if data.get("draft") is not False:
        reasons.append("the PR is a draft")
    if base.get("ref") != BASE:
        reasons.append(f"the PR targets {base.get('ref')!r}, not {BASE}")
    if not str(head.get("ref") or "").startswith(BRANCH_PREFIX):
        reasons.append(f"the head branch {head.get('ref')!r} is not a {BRANCH_PREFIX}* branch")
    if head_repo != REPO:
        reasons.append(f"the head branch is in {head_repo!r}, not {REPO}")
    if head.get("sha") != sha:
        reasons.append(f"the head is {head.get('sha')}, not the approved {sha}: it moved since the approval")
    labels = [label.get("name") for label in data.get("labels") or [] if isinstance(label, dict)]
    if HOLD_LABEL in labels:
        reasons.append(f"the PR carries the {HOLD_LABEL} label")
    if data.get("mergeable") is None:
        reasons.append("GitHub has not worked out whether the PR is mergeable")
    elif data.get("mergeable") is not True:
        reasons.append("GitHub reports the PR not mergeable")
    if data.get("mergeable_state") == "dirty":
        reasons.append("the PR conflicts with master")
    return reasons


def check_runs(steward: Steward, sha: str) -> list:
    reasons = []
    runs = []
    for page in steward.pages(f"repos/{{owner}}/{{repo}}/commits/{sha}/check-runs?filter=latest&per_page=100"):
        runs.extend((page or {}).get("check_runs") or [])
    if not any(run.get("name") == REQUIRED_CHECK for run in runs):
        reasons.append(f"no {REQUIRED_CHECK} check run on {sha}")
    for run in runs:
        name = run.get("name")
        if run.get("status") != "completed":
            reasons.append(f"the check run {name} is {run.get('status')}, not completed")
        elif run.get("conclusion") != "success":
            reasons.append(f"the check run {name} concluded {run.get('conclusion')}, not success")
    return reasons


def check_statuses(steward: Steward, sha: str) -> list:
    reasons = []
    combined = steward.gh([f"repos/{{owner}}/{{repo}}/commits/{sha}/status?per_page=100"]) or {}
    statuses = combined.get("statuses") or []
    total = combined.get("total_count", len(statuses))
    if total > 100 or total > len(statuses):
        reasons.append(f"{sha} has {total} statuses, more than one page of 100 shows")
    for status in statuses:
        if status.get("state") != "success":
            reasons.append(f"the status {status.get('context')} is {status.get('state')}, not success")
    return reasons


def check_threads(steward: Steward, pr: int) -> list:
    unresolved = 0
    after = None
    while True:
        args = ["graphql", "-f", f"query={THREADS_QUERY}", "-f", f"owner={OWNER}", "-f", f"name={NAME}"]
        args += ["-F", f"number={pr}"]
        if after:
            args += ["-f", f"after={after}"]
        data = steward.gh(args) or {}
        pull_request = ((data.get("data") or {}).get("repository") or {}).get("pullRequest")
        if not pull_request:
            return [f"GraphQL found no pull request #{pr}"]
        threads = pull_request.get("reviewThreads") or {}
        unresolved += sum(1 for node in threads.get("nodes") or [] if node.get("isResolved") is not True)
        info = threads.get("pageInfo") or {}
        if not info.get("hasNextPage"):
            break
        after = info.get("endCursor")
        if not after:
            raise RuntimeError("GraphQL said there are more review threads but gave no cursor")
    return [f"{unresolved} review thread(s) unresolved"] if unresolved else []


def is_approval(comment: dict, sha: str) -> bool:
    if (comment.get("user") or {}).get("login") != BOT_LOGIN:
        return False
    lines = [line.rstrip("\r") for line in str(comment.get("body") or "").split("\n")]
    return STEER_MARKER in lines and f"Approved head: {sha}" in lines


def check_steer(steward: Steward, pr: int, sha: str) -> list:
    comments = []
    for page in steward.pages(f"repos/{{owner}}/{{repo}}/issues/{pr}/comments"):
        comments.extend(page or [])
    if any(is_approval(comment, sha) for comment in comments if isinstance(comment, dict)):
        return []
    return [f"no approval steer by {BOT_LOGIN} names the head {sha}"]


def merge(steward: Steward, pr: int, sha: str, title: str) -> dict:
    body = {"merge_method": "squash", "sha": sha, "commit_title": f"{title} (#{pr})"}
    try:
        steward.gh(["--method", "PUT", f"repos/{{owner}}/{{repo}}/pulls/{pr}/merge"], body=body)
    except RuntimeError as err:
        raise RuntimeError(f"{err}; nothing was merged")
    data = pull(steward, pr)
    if data.get("merged") is not True:
        raise RuntimeError(f"the merge call answered, but #{pr} is not merged")
    return data


def main(argv: list) -> int:
    parser = argparse.ArgumentParser(description="Land a PR as mintswarm-dev-steward at the approved head.")
    parser.add_argument("pr", type=int, help="pull request number")
    parser.add_argument("--sha", required=True, help="the head commit the maintainer approved (40 hex characters)")
    parser.add_argument("--dry-run", action="store_true", help="check everything, merge nothing")
    args = parser.parse_args(argv)

    try:
        if args.pr <= 0:
            raise UsageError("PR must be a positive number")
        if not SHA_RE.match(args.sha):
            raise UsageError("--sha must be 40 lowercase hex characters")
        try:
            delay = float(os.environ.get("MSW_LAND_RETRY_DELAY", "3"))
        except ValueError:
            raise UsageError("MSW_LAND_RETRY_DELAY must be a number of seconds")
        if delay < 0:
            raise UsageError("MSW_LAND_RETRY_DELAY must not be negative")

        config = steward_config()
        steward = Steward(config, mint(config))
        data = pull_settled(steward, args.pr, delay)
        reasons = check_pull(data, args.sha)
        reasons += check_runs(steward, args.sha)
        reasons += check_statuses(steward, args.sha)
        reasons += check_threads(steward, args.pr)
        reasons += check_steer(steward, args.pr, args.sha)
        if reasons:
            for reason in reasons:
                print(f"land.py: refused: {reason}", file=sys.stderr)
            print(f"land.py: #{args.pr} was not merged", file=sys.stderr)
            return 1
        if args.dry_run:
            print(f"#{args.pr} at {args.sha} passes every check; nothing merged (--dry-run)")
            return 0
        merged = merge(steward, args.pr, args.sha, str(data.get("title") or ""))
        print(f"merged #{args.pr} at {args.sha}")
        print(f"merge_commit_sha: {merged.get('merge_commit_sha')}")
        print(f"merged_by: {(merged.get('merged_by') or {}).get('login')}")
        return 0
    except UsageError as err:
        print(f"land.py: {err}", file=sys.stderr)
        return 2
    except Refused as err:
        print(f"land.py: refused: {err}", file=sys.stderr)
        return 1
    except (RuntimeError, json.JSONDecodeError, OSError, AttributeError, TypeError) as err:
        print(f"land.py: {err}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
