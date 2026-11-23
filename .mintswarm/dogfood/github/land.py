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
pull_requests write, checks (check runs and suites), statuses and metadata
read. It must start with
ghs_. It is held in a variable and passed to every bot-gh.sh call as MSW_TOKEN
in the child's environment only; it is never printed and never in any argv.

The approval steer is a comment on the PR by mintswarm-dev[bot], posted by the
session right before this script runs, that opens with exactly these three
lines, the maintainer's quoted words and the time following them:

    <!-- mintswarm: steer -->
    <!-- mintswarm: steer approval -->
    Approved head: SHA

A steer is counted as .mintswarm/dogfood/landing/collect-facts.py counts one:
any comment or submitted review by an account that is not a bot (user.type
is not Bot and the login does not end in [bot]), and any comment or review
by mintswarm-dev[bot] with a line that, trimmed of whitespace, is exactly the
steer marker. The latest approval steer naming SHA (by created_at, then id)
is the approval, and any other steer withdraws it: a comment created after
it (by created_at, then id), a comment whose updated_at is after its
created_at (an earlier comment may have been edited into a steer), or a
review whose submitted_at is not before its created_at (reviews in state
PENDING or without submitted_at are skipped; reviews have no updated_at, so
an edited review body is not seen). So any later steer, relayed or the
maintainer's own, a "Request changes" or comment review included, withdraws
an approval. The approval itself is strict: the three lines count only at the
top of a comment by mintswarm-dev[bot].

Checks, all made with the steward's token; every failure is collected and
printed on stderr, and nothing is merged unless all of them hold:

- pulls/PR: open, not a draft, base master, head ref mintswarm/*, head repo
  mintswarm/mintswarm, head sha equal to SHA; no label named hold; mergeable
  true and mergeable_state not dirty. While mergeable is null on an open PR,
  pulls/PR is read again up to 5 times, $MSW_LAND_RETRY_DELAY seconds apart
  (default 3); after that it refuses.
- commits/SHA/check-runs?filter=latest and commits/SHA/check-suites, every
  page: at least one run named gates-matrix created by the github-actions App;
  the check suite of each such run is among the commit's suites and holds at
  least one other run (a gate) by github-actions, since the gates workflow
  creates the gate runs only once gates-matrix has completed; every suite with
  check runs is completed with conclusion success; every run is completed with
  conclusion success. Only success passes: skipped, neutral and every other
  conclusion refuse.
- commits/SHA/status: every status success, and no more than the 100 one page
  holds.
- GraphQL pullRequest.reviewThreads, paged: no unresolved thread.
- issues/PR/comments and pulls/PR/reviews, every page: an approval steer
  naming SHA, with no steer that withdraws it.

When all hold and --dry-run is not given, it reads pulls/PR, the review
threads, the comments and the reviews again, and refuses if any of the fields
checked above (or the title) changed, a thread is unresolved or a steer now
withdraws the approval, since GitHub does not re-check a ruleset
bypass actor's merge for them; then it calls
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
ACTIONS_APP = "github-actions"
STEER_MARKER = "<!-- mintswarm: steer -->"
APPROVAL_MARKER = "<!-- mintswarm: steer approval -->"
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


def app_slug(item: dict):
    return (item.get("app") or {}).get("slug")


def suite_id(run: dict):
    return (run.get("check_suite") or {}).get("id")


def check_runs(steward: Steward, sha: str) -> list:
    reasons = []
    runs = []
    for page in steward.pages(f"repos/{{owner}}/{{repo}}/commits/{sha}/check-runs?filter=latest&per_page=100"):
        runs.extend(run for run in (page or {}).get("check_runs") or [] if isinstance(run, dict))
    suites = []
    for page in steward.pages(f"repos/{{owner}}/{{repo}}/commits/{sha}/check-suites?per_page=100"):
        suites.extend(suite for suite in (page or {}).get("check_suites") or [] if isinstance(suite, dict))
    suites_by_id = {suite.get("id"): suite for suite in suites}

    matrices = [run for run in runs if run.get("name") == REQUIRED_CHECK and app_slug(run) == ACTIONS_APP]
    if not matrices:
        reasons.append(f"no {REQUIRED_CHECK} check run by {ACTIONS_APP} on {sha}")
    for matrix in matrices:
        sid = suite_id(matrix)
        if sid is None or sid not in suites_by_id:
            reasons.append(f"the check suite {sid} of {REQUIRED_CHECK} is not among the check suites of {sha}")
            continue
        gates = [run for run in runs if run is not matrix and suite_id(run) == sid]
        if not gates:
            reasons.append(f"the gates workflow (check suite {sid}) has no gate check run yet: the gates have not run")
        for gate in gates:
            if app_slug(gate) != ACTIONS_APP:
                reasons.append(
                    f"the check run {gate.get('name')} in check suite {sid} is by {app_slug(gate)!r}, not {ACTIONS_APP}"
                )
    for suite in suites:
        if not suite.get("latest_check_runs_count"):
            continue
        label = f"the check suite {suite.get('id')} of {app_slug(suite)}"
        if suite.get("status") != "completed":
            reasons.append(f"{label} is {suite.get('status')}, not completed")
        elif suite.get("conclusion") != "success":
            reasons.append(f"{label} concluded {suite.get('conclusion')}, not success")
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


def body_lines(comment: dict) -> list:
    return [line.rstrip("\r") for line in str(comment.get("body") or "").split("\n")]


def author(comment: dict) -> str:
    return str((comment.get("user") or {}).get("login") or "")


def is_bot(comment: dict) -> bool:
    return (comment.get("user") or {}).get("type") == "Bot" or author(comment).endswith("[bot]")


def is_steer(comment: dict) -> bool:
    """A steer as collect-facts.py counts one: any comment by a person, or a bot comment with the marker on a line."""
    if not is_bot(comment):
        return True
    return author(comment) == BOT_LOGIN and any(line.strip() == STEER_MARKER for line in body_lines(comment))


def is_approval(comment: dict, sha: str) -> bool:
    return author(comment) == BOT_LOGIN and body_lines(comment)[:3] == [
        STEER_MARKER,
        APPROVAL_MARKER,
        f"Approved head: {sha}",
    ]


def steer_order(comment: dict):
    cid = comment.get("id")
    return (str(comment.get("created_at") or ""), cid if isinstance(cid, int) else 0)


def check_steer(steward: Steward, pr: int, sha: str) -> list:
    comments, reviews = [], []
    for page in steward.pages(f"repos/{{owner}}/{{repo}}/issues/{pr}/comments"):
        comments.extend(page or [])
    for page in steward.pages(f"repos/{{owner}}/{{repo}}/pulls/{pr}/reviews"):
        reviews.extend(page or [])
    steers = [comment for comment in comments if isinstance(comment, dict) and is_steer(comment)]
    approvals = [steer for steer in steers if is_approval(steer, sha)]
    if not approvals:
        return [f"no approval steer by {BOT_LOGIN} names the head {sha}"]
    approval = max(approvals, key=steer_order)
    approved_at = str(approval.get("created_at") or "")
    withdraw = f"withdraws the approval of {sha}: post the approval steer again"
    reasons = []
    for steer in steers:
        if is_approval(steer, sha):
            continue
        who = author(steer) or "a deleted user"
        if steer_order(steer) > steer_order(approval):
            reasons.append(f"a later steer by {who} {withdraw}")
        elif str(steer.get("updated_at") or "") > approved_at:
            reasons.append(f"a steer by {who} edited after the approval {withdraw}")
    for review in reviews:
        if not isinstance(review, dict) or review.get("state") == "PENDING" or not review.get("submitted_at"):
            continue
        if is_steer(review) and str(review.get("submitted_at")) >= approved_at:
            who = author(review) or "a deleted user"
            reasons.append(f"a later review by {who} ({review.get('state')}) {withdraw}")
    return reasons


def snapshot(data: dict) -> dict:
    head = data.get("head") or {}
    return {
        "state": data.get("state"),
        "draft": data.get("draft"),
        "title": data.get("title"),
        "base": (data.get("base") or {}).get("ref"),
        "head_ref": head.get("ref"),
        "head_sha": head.get("sha"),
        "head_repo": (head.get("repo") or {}).get("full_name"),
        "labels": sorted(str(label.get("name")) for label in data.get("labels") or [] if isinstance(label, dict)),
        "mergeable": data.get("mergeable"),
        "mergeable_state": data.get("mergeable_state"),
    }


def recheck(steward: Steward, pr: int, sha: str, before: dict) -> list:
    """Read the PR, its threads and its steers again right before the merge: a bypass actor's merge is not re-checked."""
    after = pull(steward, pr)
    reasons = check_pull(after, sha)
    old, new = snapshot(before), snapshot(after)
    for field in old:
        if old[field] != new[field]:
            reasons.append(f"the PR's {field} changed during the checks, from {old[field]!r} to {new[field]!r}")
    reasons += check_threads(steward, pr)
    reasons += check_steer(steward, pr, sha)
    return reasons


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


def refuse(pr: int, reasons: list) -> int:
    for reason in reasons:
        print(f"land.py: refused: {reason}", file=sys.stderr)
    print(f"land.py: #{pr} was not merged", file=sys.stderr)
    return 1


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
            return refuse(args.pr, reasons)
        if args.dry_run:
            print(f"#{args.pr} at {args.sha} passes every check; nothing merged (--dry-run)")
            return 0
        reasons = recheck(steward, args.pr, args.sha, data)
        if reasons:
            return refuse(args.pr, reasons)
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
