#!/usr/bin/env python3
"""Request review on a PR from the code owners of its changed paths.

    request-review.py PR --branch B [--base origin/master] [--dry-run]
    request-review.py PR --files F... [--base origin/master] [--dry-run]

Dogfooding stand-in for the runner's review requests. Run it after the PR is
opened and again after it is marked ready for review.

The reviewers are a minimal covering set of the user code owners of the
changed paths, read from CODEOWNERS at --base: one owner for each approval
group, chosen by `assign-approvers.py owners`, which this script runs with the
same --base and with --branch, or with --paths taking --files. Its docstring
gives the CODEOWNERS rules, the cover and the cases that exit 2; its stderr is
passed through. Team code owners are not requested here: GitHub requests them
itself when the PR opens.

Unless --dry-run, the reviewers are requested with
`bot-gh.sh api --method POST repos/{owner}/{repo}/pulls/PR/requested_reviewers --input -`,
so as mintswarm-dev-bee[bot], and the logins GitHub reports as requested are printed.
With --dry-run the chosen logins are printed, one per line, and nothing is sent.
Reviewers are never filtered by earlier reviews or by requests already
pending: a reviewer who has approved or requested changes is requested again,
which GitHub accepts and which puts the PR back in their review queue. That is
how the session re-requests review after an address round (README.md,
"Session procedure", step 10).

Exits 0 on success; 1 when git, assign-approvers.py or the request fails; 2 on
bad arguments, and whenever `assign-approvers.py owners` exits 2: a path with
no code owner, no user left to request, or CODEOWNERS it cannot read.
Standard library only.
"""

import argparse
import json
import subprocess
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
ASSIGN = HERE / "assign-approvers.py"
BOT_GH = HERE / "bot-gh.sh"


class UsageError(Exception):
    pass


def reviewers(base: str, branch: str, files: list) -> list:
    argv = [sys.executable, "-I", str(ASSIGN), "owners", "--base", base]
    argv += ["--branch", branch] if files is None else ["--paths"] + files
    proc = subprocess.run(argv, stdout=subprocess.PIPE, text=True)
    if proc.returncode == 2:
        raise UsageError("no reviewers: assign-approvers.py owners exited 2")
    if proc.returncode != 0:
        raise RuntimeError(f"assign-approvers.py owners failed with exit {proc.returncode}")
    return [line for line in proc.stdout.splitlines() if line]


def request(pr: int, logins: list) -> list:
    body = json.dumps({"reviewers": logins})
    proc = subprocess.run(
        [
            str(BOT_GH),
            "api",
            "--method",
            "POST",
            f"repos/{{owner}}/{{repo}}/pulls/{pr}/requested_reviewers",
            "--input",
            "-",
        ],
        input=body,
        capture_output=True,
        text=True,
    )
    if proc.returncode != 0:
        raise RuntimeError(f"requesting reviewers on #{pr} failed: {proc.stderr.strip()}")
    response = json.loads(proc.stdout or "{}")
    return [r.get("login", "") for r in response.get("requested_reviewers") or []]


def main(argv: list) -> int:
    parser = argparse.ArgumentParser(description="Request review from the code owners of the changed paths.")
    parser.add_argument("pr", type=int, help="pull request number")
    parser.add_argument("--branch", help="the PR's head branch, for git diff")
    parser.add_argument("--base", default="origin/master", help="ref to diff against and read CODEOWNERS at")
    parser.add_argument("--files", nargs="+", help="changed files, instead of git diff")
    parser.add_argument("--dry-run", action="store_true", help="print the reviewers, send nothing")
    args = parser.parse_args(argv)

    try:
        if args.pr <= 0:
            raise UsageError("PR must be a positive number")
        if (args.files is None) == (not args.branch):
            raise UsageError("give exactly one of --branch and --files")
        logins = reviewers(args.base, args.branch, args.files)
        if args.dry_run:
            print("\n".join(logins))
            return 0
        print("\n".join(request(args.pr, logins)))
        return 0
    except UsageError as err:
        print(f"request-review.py: {err}", file=sys.stderr)
        return 2
    except (RuntimeError, json.JSONDecodeError, OSError) as err:
        print(f"request-review.py: {err}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
