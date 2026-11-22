#!/usr/bin/env python3
"""Request review on a PR from the reviewers configured for its changed paths.

    request-review.py PR --branch B [--base origin/master] [--dry-run]
    request-review.py PR --files F... [--dry-run]

Dogfooding stand-in for the runner's review requests. Run it after the PR is
opened and again after it is marked ready for review.

Changed files come from `git diff --name-only --no-renames BASE...B` in the
current directory, or from --files. The configuration is
.mintswarm/config.yaml at the root of the checkout holding this script (or
--config), read with `yq -o=json`:

    components:
      core: {paths: ["crates/core/**"], reviewers: [alice]}   # reviewers optional
    pipeline:
      review: {reviewers: [zmij]}                            # the default

A path glob matches the whole path: `**` matches anything including `/`,
`**/` matches zero or more directories, `*` matches within one path segment
and `?` one character other than `/`. For each file the reviewers are the union
of those of the components whose paths match it; when none of those components
has reviewers, or no component matches, the default applies. The result is the
union over all files, with a leading `@` stripped, deduplicated and sorted.
Every login must match ^[A-Za-z0-9-]+$.

Unless --dry-run, the reviewers are requested with
`bot-gh.sh api --method POST repos/{owner}/{repo}/pulls/PR/requested_reviewers --input -`,
so as mintswarm-dev[bot], and the logins GitHub reports as requested are printed.
With --dry-run the resolved logins are printed, one per line, and nothing is sent.
Code owners are not handled here: GitHub requests them itself.

Exits 0 on success, including when no reviewer is resolved (nothing is sent);
1 when git, yq or the request fails; 2 on bad arguments, configuration or logins.
Standard library only.
"""

import argparse
import json
import re
import subprocess
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[2]
DEFAULT_CONFIG = ROOT / ".mintswarm" / "config.yaml"
BOT_GH = HERE / "bot-gh.sh"
LOGIN = re.compile(r"^[A-Za-z0-9-]+$")


class UsageError(Exception):
    pass


def glob_to_regex(pattern: str) -> re.Pattern:
    out = []
    i = 0
    while i < len(pattern):
        if pattern.startswith("**/", i):
            out.append("(?:.*/)?")
            i += 3
        elif pattern.startswith("**", i):
            out.append(".*")
            i += 2
        elif pattern[i] == "*":
            out.append("[^/]*")
            i += 1
        elif pattern[i] == "?":
            out.append("[^/]")
            i += 1
        else:
            out.append(re.escape(pattern[i]))
            i += 1
    return re.compile("".join(out), re.DOTALL)


def logins(value, where: str) -> list:
    if value is None:
        return []
    if not isinstance(value, list):
        raise UsageError(f"{where} must be a list of logins")
    result = []
    for item in value:
        if not isinstance(item, str):
            raise UsageError(f"{where}: not a login: {item!r}")
        login = item[1:] if item.startswith("@") else item
        if not LOGIN.match(login):
            raise UsageError(f"{where}: not a valid login: {item!r}")
        result.append(login)
    return result


def load_config(path: Path) -> dict:
    proc = subprocess.run(["yq", "-o=json", ".", str(path)], capture_output=True, text=True)
    if proc.returncode != 0:
        raise RuntimeError(f"yq failed on {path}: {proc.stderr.strip()}")
    config = json.loads(proc.stdout or "null")
    if not isinstance(config, dict):
        raise UsageError(f"{path} is not a mapping")
    return config


def resolve(config: dict, files: list) -> list:
    components = []
    for name, component in (config.get("components") or {}).items():
        if not isinstance(component, dict):
            raise UsageError(f"components.{name} must be a mapping")
        paths = component.get("paths") or []
        if not isinstance(paths, list) or not all(isinstance(p, str) for p in paths):
            raise UsageError(f"components.{name}.paths must be a list of globs")
        regexes = [glob_to_regex(p) for p in paths]
        components.append((regexes, logins(component.get("reviewers"), f"components.{name}.reviewers")))
    review = (config.get("pipeline") or {}).get("review") or {}
    default = logins(review.get("reviewers"), "pipeline.review.reviewers")

    resolved = set()
    for path in files:
        per_file = set()
        for regexes, reviewers in components:
            if any(r.fullmatch(path) for r in regexes):
                per_file.update(reviewers)
        resolved.update(per_file or default)
    return sorted(resolved)


def changed_files(base: str, branch: str) -> list:
    proc = subprocess.run(
        ["git", "diff", "--name-only", "--no-renames", f"{base}...{branch}"],
        capture_output=True,
        text=True,
    )
    if proc.returncode != 0:
        raise RuntimeError(f"git diff {base}...{branch} failed: {proc.stderr.strip()}")
    return [line for line in proc.stdout.splitlines() if line]


def request(pr: int, reviewers: list) -> list:
    body = json.dumps({"reviewers": reviewers})
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
    parser = argparse.ArgumentParser(description="Request review from the configured reviewers.")
    parser.add_argument("pr", type=int, help="pull request number")
    parser.add_argument("--branch", help="the PR's head branch, for git diff")
    parser.add_argument("--base", default="origin/master", help="base for git diff (default origin/master)")
    parser.add_argument("--files", nargs="+", help="changed files, instead of git diff")
    parser.add_argument("--config", type=Path, default=DEFAULT_CONFIG, help=argparse.SUPPRESS)
    parser.add_argument("--dry-run", action="store_true", help="print the reviewers, send nothing")
    args = parser.parse_args(argv)

    try:
        if args.pr <= 0:
            raise UsageError("PR must be a positive number")
        if args.files is None and not args.branch:
            raise UsageError("give --branch or --files")
        files = args.files if args.files is not None else changed_files(args.base, args.branch)
        reviewers = resolve(load_config(args.config), files)
        if not reviewers:
            print("no reviewers resolved; nothing requested", file=sys.stderr)
            return 0
        if args.dry_run:
            print("\n".join(reviewers))
            return 0
        requested = request(args.pr, reviewers)
        print("\n".join(requested))
        return 0
    except UsageError as err:
        print(f"request-review.py: {err}", file=sys.stderr)
        return 2
    except (RuntimeError, json.JSONDecodeError, OSError) as err:
        print(f"request-review.py: {err}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
