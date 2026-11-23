#!/usr/bin/env python3
"""Re-classify an implementation issue as a plan when its analysis splits it.

    reclassify.py split ISSUE --analysis PATH --analysis-url URL [--base origin/master] [--dry-run]
    reclassify.py check-parent PARENT [--base origin/master]

Dogfooding stand-in for the runner's re-classification before decomposition.
Every GitHub call goes through bot-gh.sh, so as mintswarm-dev[bot].

Both subcommands read .mintswarm/config.yaml at the base ref (--base, resolved
with `git rev-parse --verify` in the current directory), never the worktree's
copy: `git show COMMIT:.mintswarm/config.yaml` piped to
`yq -o=json . /dev/stdin`. The issue types are tracker.kinds:

    tracker:
      kinds:
        design:         {issue_type: Design}
        plan:           {issue_type: Plan}
        implementation: {issue_type: Task}

split reads the analysis at PATH. It must hold exactly one `## Split` section
with exactly one json fence:

    {"split": {"proposed": true, "reason": "components",
               "components": ["agents", "docs"], "finding": "one line"}}

reason is components, other or none; with proposed true it is not none and
finding is not empty. URL is the analysis artifact's html_url and must start
with https://github.com/. With proposed false split prints
`kind: implementation` and writes nothing. Otherwise it reads the issue:

- typed as the plan type already: it prints `kind: plan` and changes nothing,
  so a re-run is a no-op;
- typed as anything but the implementation type (or not typed): exit 2;
- typed as the implementation type: it PATCHes the issue with
  {"type": "<plan type>"} on stdin and checks that the response's type.name
  is the plan type (exit 1 otherwise, with no comment posted), then posts one
  comment with the marker `<!-- mintswarm: reclassified -->`, the old and new
  types, the reason, the components, the finding and the analysis URL.

After a re-classification it prints, one per line:

    kind: plan
    state: Decomposing (re-classified as plan)
    comment: <the comment's html_url>
    package: Re-classified from <old> to <new>: <finding> (<the comment's html_url>)

With --dry-run split reads the issue, prints the lines without the comment's
URL and writes nothing.

check-parent reads the parent and exits 0 when its type is the plan or the
design type; otherwise it exits 1 with
`refusing to file children under #N: type <name or none> is neither Plan nor Design`.

Exits 0 on success or a no-op; 1 on a refusal, or when git, yq or a request
fails or GitHub does not do what was asked; 2 on bad arguments, analysis or
configuration. Standard library only.
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
MARKER = "<!-- mintswarm: reclassified -->"
STATE = "Decomposing (re-classified as plan)"
REASONS = ("components", "other", "none")
KINDS = ("implementation", "plan", "design")
FENCE = re.compile(r"^```json[ \t]*\n(.*?)^```[ \t]*$", re.MULTILINE | re.DOTALL)


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


def issue_types(ref: str) -> dict:
    try:
        commit = run(["git", "rev-parse", "--verify", "--quiet", "--end-of-options", f"{ref}^{{commit}}"]).strip()
    except RuntimeError as exc:
        raise RuntimeError(f"base ref {ref!r} does not resolve to a commit") from exc
    text = run(["git", "show", f"{commit}:{CONFIG}"])
    config = json.loads(run(["yq", "-o=json", ".", "/dev/stdin"], text) or "null")
    if not isinstance(config, dict):
        raise UsageError(f"{CONFIG} at {ref} is not a mapping")
    kinds = (config.get("tracker") or {}).get("kinds") if isinstance(config.get("tracker"), dict) else None
    if not isinstance(kinds, dict):
        raise UsageError(f"tracker.kinds at {ref} must be a mapping")
    types = {}
    for kind in KINDS:
        entry = kinds.get(kind)
        name = entry.get("issue_type") if isinstance(entry, dict) else None
        if not isinstance(name, str) or not name.strip():
            raise UsageError(f"tracker.kinds.{kind}.issue_type at {ref} must be a type name")
        types[kind] = name
    return types


def read_split(path: str) -> dict:
    try:
        text = Path(path).read_text(encoding="utf-8")
    except OSError as exc:
        raise UsageError(f"cannot read the analysis: {exc}") from exc
    lines = text.splitlines(keepends=True)
    starts = [i for i, line in enumerate(lines) if line.rstrip() == "## Split"]
    if len(starts) != 1:
        raise UsageError(f"the analysis must have exactly one '## Split' section, not {len(starts)}")
    body = []
    fenced = False
    for line in lines[starts[0] + 1 :]:
        if line.startswith("```"):
            fenced = not fenced
        elif not fenced and line.startswith("## "):
            break
        body.append(line)
    fences = FENCE.findall("".join(body))
    if len(fences) != 1:
        raise UsageError(f"'## Split' must hold exactly one json fence, not {len(fences)}")
    try:
        doc = json.loads(fences[0])
    except json.JSONDecodeError as exc:
        raise UsageError(f"'## Split' is not valid json: {exc}") from exc
    split = doc.get("split") if isinstance(doc, dict) else None
    if not isinstance(split, dict):
        raise UsageError("'## Split' must hold an object with a 'split' object")
    proposed, reason = split.get("proposed"), split.get("reason")
    components, finding = split.get("components"), split.get("finding")
    if not isinstance(proposed, bool):
        raise UsageError("split.proposed must be true or false")
    if reason not in REASONS:
        raise UsageError(f"split.reason must be one of {', '.join(REASONS)}, not {reason!r}")
    if not isinstance(components, list) or not all(isinstance(c, str) for c in components):
        raise UsageError("split.components must be a list of component names")
    if not isinstance(finding, str) or "\n" in finding or "\r" in finding:
        raise UsageError("split.finding must be one line")
    if proposed and (reason == "none" or not finding.strip()):
        raise UsageError("a proposed split needs a reason other than none and a finding")
    return split


def gh(args: list, body: dict = None):
    argv = [str(BOT_GH), "api"] + args
    if body is not None:
        argv += ["--input", "-"]
    return json.loads(run(argv, json.dumps(body) if body is not None else None) or "null")


def type_of(issue) -> str:
    if not isinstance(issue, dict):
        raise RuntimeError("GitHub returned no issue")
    kind = issue.get("type")
    name = kind.get("name") if isinstance(kind, dict) else None
    return name if isinstance(name, str) else None


def comment_body(old: str, new: str, split: dict, url: str) -> str:
    components = ", ".join(f"`{c}`" for c in split["components"]) or "(none named)"
    return "\n".join(
        [
            MARKER,
            f"Re-classified from `{old}` to `{new}`: the analysis proposes splitting this issue,"
            " so it goes to decomposition as a plan issue.",
            "",
            f"- **Reason:** {split['reason']}",
            f"- **Components:** {components}",
            f"- **Finding:** {split['finding']}",
            f"- **Analysis:** {url}",
        ]
    )


def split_issue(number: int, analysis: str, url: str, base: str, dry_run: bool) -> int:
    if not url.startswith("https://github.com/"):
        raise UsageError(f"--analysis-url must be a https://github.com/ URL, not {url!r}")
    split = read_split(analysis)
    if not split["proposed"]:
        print("kind: implementation")
        return 0
    types = issue_types(base)
    current = type_of(gh([f"repos/{{owner}}/{{repo}}/issues/{number}"]))
    if current == types["plan"]:
        print(f"reclassify.py: #{number} is already a {types['plan']} issue", file=sys.stderr)
        print("kind: plan")
        return 0
    if current != types["implementation"]:
        raise UsageError(
            f"#{number} has type {current or 'none'}, not {types['implementation']}; only an"
            " implementation issue is re-classified"
        )
    old, new = current, types["plan"]
    if dry_run:
        print("kind: plan")
        print(f"state: {STATE}")
        print(f"package: Re-classified from {old} to {new}: {split['finding']}")
        return 0
    patched = gh(["--method", "PATCH", f"repos/{{owner}}/{{repo}}/issues/{number}"], {"type": new})
    if type_of(patched) != new:
        print(
            f"reclassify.py: GitHub did not set #{number}'s type to {new} (it reports"
            f" {type_of(patched) or 'none'}); no comment was posted",
            file=sys.stderr,
        )
        return 1
    posted = gh(
        ["--method", "POST", f"repos/{{owner}}/{{repo}}/issues/{number}/comments"],
        {"body": comment_body(old, new, split, url)},
    )
    link = posted.get("html_url") if isinstance(posted, dict) else None
    if not isinstance(link, str) or not link.startswith("https://github.com/"):
        raise RuntimeError(f"GitHub returned no comment URL for #{number}")
    print("kind: plan")
    print(f"state: {STATE}")
    print(f"comment: {link}")
    print(f"package: Re-classified from {old} to {new}: {split['finding']} ({link})")
    return 0


def check_parent(number: int, base: str) -> int:
    types = issue_types(base)
    current = type_of(gh([f"repos/{{owner}}/{{repo}}/issues/{number}"]))
    if current in (types["plan"], types["design"]):
        print(f"#{number} is a {current} issue: children may be filed under it")
        return 0
    print(
        f"refusing to file children under #{number}: type {current or 'none'} is neither"
        f" {types['plan']} nor {types['design']}",
        file=sys.stderr,
    )
    return 1


def main(argv: list) -> int:
    parser = argparse.ArgumentParser(description="Re-classify a split issue as a plan, or check a parent's type.")
    sub = parser.add_subparsers(dest="action", required=True)
    s = sub.add_parser("split", help="re-classify an implementation issue whose analysis proposes a split")
    s.add_argument("number", type=int, help="issue number")
    s.add_argument("--analysis", required=True, help="path to the published analysis.md")
    s.add_argument("--analysis-url", required=True, help="the analysis artifact's html_url")
    s.add_argument("--base", default="origin/master", help="ref to read the config at (default origin/master)")
    s.add_argument("--dry-run", action="store_true", help="print the lines, change nothing")
    c = sub.add_parser("check-parent", help="refuse a parent that is neither a plan nor a design issue")
    c.add_argument("number", type=int, help="parent issue number")
    c.add_argument("--base", default="origin/master", help="ref to read the config at (default origin/master)")
    args = parser.parse_args(argv)

    try:
        if args.number <= 0:
            raise UsageError("the issue number must be positive")
        if args.action == "split":
            return split_issue(args.number, args.analysis, args.analysis_url, args.base, args.dry_run)
        return check_parent(args.number, args.base)
    except UsageError as err:
        print(f"reclassify.py: {err}", file=sys.stderr)
        return 2
    except (RuntimeError, json.JSONDecodeError, OSError) as err:
        print(f"reclassify.py: {err}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
