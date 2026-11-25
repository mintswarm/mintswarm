#!/usr/bin/env python3
"""Capture `plan-approval/floor.py` on a fixture repository, as the plan-floor bundle's parity fixture.

Usage: plan_floor_bundle_capture.py OUT

Builds the repository RECIPE declares in a temporary directory outside the repository, with git plumbing
only (`hash-object`, `update-index --cacheinfo` into a temporary index, `write-tree` and `commit-tree`) under
fixed identities and dates, so its commit ids do not depend on the machine. Then, for each case of CASES, it
writes the case's documents and result records into that repository's worktree and runs
`.mintswarm/dogfood/plan-approval/floor.py` there. OUT gets one JSON object per line:

- `{"kind": "recipe", "branch", "commits"}`: the recipe, each commit with its parents (indices of earlier
  commits), time, encoding (null for UTF-8), message bytes, files (`{"path", "mode", "bytes"}`) and the
  commit id the plumbing gave;
- `{"kind": "case", "name", "base", "component", "documents", "files", "directories", "exit", "floor"}`: one
  run of `floor.py`, `base` the ref as given and the commit it is pinned to, `documents` the path given for
  each of `plan`, `issue_document`, `analysis` and `screen` (or null), `files` every file written under the
  worktree root (`{"path", "bytes"}`), `directories` every directory made where a file is expected, `exit` the
  exit code and `floor` the whole text of the `floor.json` it wrote, or null.

Bytes are `{"utf8": text}`, or `{"hex": digits}` when they are not valid UTF-8. The config and the plans keep
every reason in the floor's own wording, never a parser's, so the texts compare byte for byte. The config is
valid both for `floor.py` and for the repository config schema the Rust floor loads.

It reads `.mintswarm/dogfood/` and never modifies it; it needs git and yq. Run it with
PYTHONDONTWRITEBYTECODE=1. Two runs write the same bytes.

`src/exec/git/tests/bundle.rs` runs it into a temporary file and fails when the committed fixture differs.
Refresh the fixture whenever `floor.py` changes:

    PYTHONDONTWRITEBYTECODE=1 python3 crates/runner/tests/fixtures/plan_floor_bundle_capture.py \\
        crates/runner/tests/fixtures/plan_floor_bundle.jsonl
    cargo test -p mintswarm-runner plan_floor
"""

import json
import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[4]
FLOOR = ROOT / ".mintswarm" / "dogfood" / "plan-approval" / "floor.py"
IDENTITY = {
    "GIT_AUTHOR_NAME": "Ada Lovelace",
    "GIT_AUTHOR_EMAIL": "ada@example.com",
    "GIT_COMMITTER_NAME": "Ada Lovelace",
    "GIT_COMMITTER_EMAIL": "ada@example.com",
}

CONFIG = """\
components:
  widgets:
    paths: ["widgets/**"]
  docs:
    paths: ["docs/**"]
shared:
  - "Cargo.lock"
pipeline:
  plan_approval:
    mode: shadow
    require_human:
      paths: ["widgets/secret/**"]
    limits: { files: 3, lines: 100, new_files: 2 }
  landing:
    mode: never
    footprint: { lines_factor: 1 }
"""

RECIPE = {
    "branch": "master",
    "commits": [
        {
            "parents": [],
            "time": 1700000000,
            "encoding": None,
            "message": b"chore(tooling): start\n",
            "files": [("README", "100644", b"fixture\n"), ("tools/run.sh", "100755", b"#!/bin/sh\n")],
        },
        {
            "parents": [0],
            "time": 1700000100,
            "encoding": None,
            "message": b"feat(widgets): the first widget\n\nWith a body.\n",
            "files": [
                ("README", "100644", b"fixture\n"),
                ("tools/run.sh", "100755", b"#!/bin/sh\n"),
                (".mintswarm/config.yaml", "100644", CONFIG.encode("utf-8")),
                ("widgets/a.rs", "100644", b"fn a() {}\n"),
            ],
        },
        {
            "parents": [1],
            "time": 1700000200,
            "encoding": "ISO-8859-1",
            "message": "docs(docs): café notes\n".encode("iso-8859-1"),
            "files": [
                ("README", "100644", b"fixture\n"),
                ("tools/run.sh", "100755", b"#!/bin/sh\n"),
                (".mintswarm/config.yaml", "100644", CONFIG.encode("utf-8")),
                ("widgets/a.rs", "100644", b"fn a() {}\n"),
                ("docs/café.md", "100644", "café\n".encode("utf-8")),
            ],
        },
    ],
}

ISSUE = ".mintswarm/job/inputs/issue.md"
ANALYSIS = ".mintswarm/job/inputs/artefacts/analysis.md"
PLAN = ".mintswarm/job/outputs/plan.md"
SCREEN = ".mintswarm/job/outputs/screen.json"
CRITERIA = ["A widget renders.", "A widget can be `a | b`."]


def issue_text() -> str:
    criteria = "".join(f"{i}. {c}\n" for i, c in enumerate(CRITERIA, 1))
    return (
        "# feat(widgets): a second widget\n\n"
        "- **Issue:** #7\n- **Kind:** implementation\n- **Component:** widgets\n- **Labels:** mintswarm\n\n"
        "## Problem\n\nOne widget is not enough.\n\n"
        f"## Acceptance criteria\n\n{criteria}"
    )


def analysis_text() -> str:
    return "# Analysis: a second widget\n\n## What is asked\n\nA second widget.\n\n## Questions\n\n"


def plan_text(files: list, new_files: list, lines: int) -> str:
    footprint = {
        "footprint": {
            "files": files,
            "new_files": new_files,
            "estimated_lines": lines,
            "production_lines": lines,
            "new_dependencies": [],
            "external_steps": [],
            "gate_changes": [],
            "overridden_assumptions": [],
        }
    }
    rows = "".join(f"| {c.replace('|', chr(92) + '|')} | a widget test |\n" for c in CRITERIA)
    return (
        "# Plan: a second widget\n\nAdd a widget.\n\n"
        "## Approach\n\nWrite it.\n\n"
        "## Steps\n\n1. **Widget** — files: as declared.\n\n"
        f"## Footprint\n\n```json\n{json.dumps(footprint, indent=2)}\n```\n\n"
        "## Verification\n\n| Acceptance criterion | Test or check |\n| --- | --- |\n"
        f"{rows}\n## Questions\n\n"
    )


def record_text(artefact: str) -> str:
    record = {
        "status": "complete",
        "artefact": artefact,
        "outputs": [f".mintswarm/job/outputs/{artefact}.md"],
        "questions": [],
        "reason": "",
        "commit": None,
        "pr_title": None,
    }
    return json.dumps(record, indent=2) + "\n"


SCREEN_TEXT = (
    json.dumps(
        {
            "tool": "dependency-screen",
            "version": 1,
            "mode": "plan",
            "date": "2026-01-01",
            "advisory_source": "osv",
            "packages": [],
        },
        indent=2,
    )
    + "\n"
)
CLEAN_PLAN = plan_text(["widgets/b.rs", "widgets/mod.rs"], ["widgets/b.rs"], 40)
ALL_DOCUMENTS = {"plan": PLAN, "issue_document": ISSUE, "analysis": ANALYSIS, "screen": SCREEN}
ALL_FILES = {
    PLAN: CLEAN_PLAN,
    ".mintswarm/job/outputs/plan.result.json": record_text("plan"),
    ISSUE: issue_text(),
    ANALYSIS: analysis_text(),
    ".mintswarm/job/inputs/artefacts/analysis.result.json": record_text("analysis"),
    SCREEN: SCREEN_TEXT,
}


def changed(files: dict, **replace) -> dict:
    out = dict(files)
    for path, text in replace.items():
        out[path] = text
    return out


# Each case: name, base ("head", "tree" or "root"), component, documents, files, directories.
CASES = [
    ("clean", "head", "widgets", ALL_DOCUMENTS, ALL_FILES, []),
    (
        "limits hit",
        "head",
        "widgets",
        ALL_DOCUMENTS,
        changed(ALL_FILES, **{PLAN: plan_text([f"widgets/{n}.rs" for n in "bcde"], ["widgets/b.rs"], 400)}),
        [],
    ),
    (
        "require_human path hit",
        "head",
        "widgets",
        ALL_DOCUMENTS,
        changed(ALL_FILES, **{PLAN: plan_text(["widgets/secret/key.rs"], [], 5)}),
        [],
    ),
    (
        "optional documents absent",
        "head",
        "widgets",
        {"plan": PLAN, "issue_document": None, "analysis": None, "screen": None},
        {PLAN: CLEAN_PLAN},
        [],
    ),
    ("base a tree", "tree", "widgets", ALL_DOCUMENTS, ALL_FILES, []),
    ("base without config", "root", "widgets", ALL_DOCUMENTS, ALL_FILES, []),
    (
        "missing analysis",
        "head",
        "widgets",
        ALL_DOCUMENTS,
        {path: text for path, text in ALL_FILES.items() if path != ANALYSIS},
        [],
    ),
    (
        "plan record a directory",
        "head",
        "widgets",
        ALL_DOCUMENTS,
        {path: text for path, text in ALL_FILES.items() if path != ".mintswarm/job/outputs/plan.result.json"},
        [".mintswarm/job/outputs/plan.result.json"],
    ),
]


def blob(data: bytes) -> dict:
    try:
        return {"utf8": data.decode("utf-8")}
    except UnicodeDecodeError:
        return {"hex": data.hex()}


def environment(tmp: Path) -> dict:
    env = {"PATH": os.environ.get("PATH", ""), "HOME": str(tmp / "home"), "TMPDIR": str(tmp / "tmp"), "LC_ALL": "C"}
    env.update({"GIT_CONFIG_NOSYSTEM": "1", "GIT_CONFIG_GLOBAL": os.devnull})
    return env


def git(env: dict, *args: str, data: bytes = b"", extra: dict = None) -> str:
    proc = subprocess.run(["git", *args], input=data, env={**env, **(extra or {})}, capture_output=True)
    if proc.returncode != 0:
        raise RuntimeError(f"git {args[0]} failed: {proc.stderr.decode('utf-8', errors='replace').strip()}")
    return proc.stdout.decode("ascii").strip()


def build(tmp: Path, work: Path) -> list:
    """Build RECIPE into the repository at work; the commit ids, in order."""
    env = {**environment(tmp), "GIT_DIR": str(work / ".git")}
    ids = []
    for n, commit in enumerate(RECIPE["commits"]):
        index = {"GIT_INDEX_FILE": str(tmp / f"index-{n}")}
        for path, mode, data in commit["files"]:
            oid = git(env, "hash-object", "-w", "--stdin", data=data)
            git(env, "update-index", "--add", "--cacheinfo", f"{mode},{oid},{path}", extra=index)
        tree = git(env, "write-tree", extra=index)
        args = []
        if commit["encoding"] is not None:
            args += ["-c", f"i18n.commitEncoding={commit['encoding']}"]
        args += ["commit-tree", tree]
        for parent in commit["parents"]:
            args += ["-p", ids[parent]]
        date = f"{commit['time']} +0000"
        dates = {"GIT_AUTHOR_DATE": date, "GIT_COMMITTER_DATE": date, **IDENTITY}
        ids.append(git(env, *args, data=commit["message"], extra=dates))
    git(env, "update-ref", f"refs/heads/{RECIPE['branch']}", ids[-1])
    return ids


def run_case(tmp: Path, work: Path, ids: list, case: tuple) -> dict:
    name, base, component, documents, files, directories = case
    shutil.rmtree(work / ".mintswarm", ignore_errors=True)
    for path, text in files.items():
        (work / path).parent.mkdir(parents=True, exist_ok=True)
        (work / path).write_bytes(text.encode("utf-8"))
    for path in directories:
        (work / path).mkdir(parents=True)
    env = {**environment(tmp), "GIT_DIR": str(work / ".git")}
    if base == "head":
        ref, commit = RECIPE["branch"], ids[-1]
    elif base == "tree":
        ref = git(env, "rev-parse", f"{ids[-1]}^{{tree}}")
        commit = ref
    else:
        ref = commit = ids[0]
    argv = ["--plan", documents["plan"], "--component", component, "--base", ref]
    for key, flag in (("issue_document", "--issue-document"), ("analysis", "--analysis"), ("screen", "--screen")):
        if documents[key] is not None:
            argv += [flag, documents[key]]
    proc = subprocess.run(
        [sys.executable, "-I", "-B", str(FLOOR), *argv], cwd=work, env=environment(tmp), capture_output=True
    )
    verdict = work / ".mintswarm" / "job" / "outputs" / "floor.json"
    return {
        "kind": "case",
        "name": name,
        "base": {"ref": ref, "commit": commit},
        "component": component,
        "documents": documents,
        "files": [{"path": path, "bytes": blob(text.encode("utf-8"))} for path, text in files.items()],
        "directories": directories,
        "exit": proc.returncode,
        "floor": verdict.read_text(encoding="utf-8") if verdict.exists() else None,
    }


def main(argv: list) -> int:
    if len(argv) != 2:
        print("usage: plan_floor_bundle_capture.py OUT", file=sys.stderr)
        return 2
    with tempfile.TemporaryDirectory(prefix="msw-plan-floor-bundle-capture-") as name:
        tmp = Path(name)
        (tmp / "home").mkdir()
        (tmp / "tmp").mkdir()
        work = tmp / "work"
        git(environment(tmp), "init", "--quiet", "--template=", "-b", RECIPE["branch"], str(work))
        ids = build(tmp, work)
        commits = [
            {
                "parents": commit["parents"],
                "time": commit["time"],
                "encoding": commit["encoding"],
                "message": blob(commit["message"]),
                "files": [{"path": path, "mode": mode, "bytes": blob(data)} for path, mode, data in commit["files"]],
                "id": oid,
            }
            for commit, oid in zip(RECIPE["commits"], ids)
        ]
        lines = [{"kind": "recipe", "branch": RECIPE["branch"], "commits": commits}]
        lines += [run_case(tmp, work, ids, case) for case in CASES]
    text = "".join(json.dumps(line, ensure_ascii=False) + "\n" for line in lines)
    Path(argv[1]).write_bytes(text.encode("utf-8"))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
