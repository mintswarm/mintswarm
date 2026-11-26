#!/usr/bin/env python3
"""Check the structure of a decomposition against the deterministic floor of decomposition approval.

    floor.py --decomposition PATH --issue-document PATH --base REF

Dogfooding stand-in for the control plane's decomposition floor. It checks
structure only: the `## Children` yaml block parses and has the decompose
stage's shape, each child has a known kind and, for an implementation child,
a `components` list of one or more components, each named once, that the
base config knows (a child's `components` names are checked against the base
config whatever its kind), `depends_on` names only
siblings, the dependencies have no cycle, and each numbered item under the
parent issue's `## Acceptance criteria` has a row in `## Coverage`. Paths are
relative to the working directory, the worktree root. All three arguments
are required, each given once as `--name value`; REF must not start with `-`.

The plan floor, .mintswarm/dogfood/plan-approval/floor.py, is loaded by path
and its object store, base-config loading, section, table and verdict
helpers are reused: the config is .mintswarm/config.yaml at the commit REF
resolves to, read from an object store made for this run, never the
worktree's. The children block is converted with `yq -p=yaml -o=json .`.

The verdict is written to .mintswarm/job/outputs/floor.json: tool, version,
the sha256 of both documents, the base ref and commit, the component names
of the base config, the verdict (`clean` or `hit`), each rule's result and
reasons, and the rules not evaluated (the child rules after a
`children-block` hit). Exit 0 when clean, 1 on a hit, 2 on unusable input or
an unexpected error, with no verdict left behind.
"""

import hashlib
import importlib.util
import os
import re
import subprocess
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
PLAN_FLOOR = HERE / ".." / "plan-approval" / "floor.py"
VERDICT = Path(".mintswarm/job/outputs/floor.json")
TOOL = "mintswarm-decomposition-floor"
VERSION = 1
RULES = (
    "children-block",
    "child-kind",
    "child-component",
    "known-components",
    "depends-on-siblings",
    "no-cycles",
    "coverage",
)
CHILD_RULES = RULES[1:6]
KINDS = ("implementation", "plan")
ARGS = ("--decomposition", "--issue-document", "--base")
USAGE = "usage: floor.py --decomposition PATH --issue-document PATH --base REF"
CRITERION = re.compile(r"([0-9]+)[.)](?:[ \t]|$)")

P = None  # the plan floor module, once loaded


class InputError(Exception):
    """Unusable input: exit 2, no verdict."""


class BlockError(Exception):
    """The children-block rule is a hit; the message is its reason."""


def remove_stale_verdict() -> None:
    try:
        os.unlink(VERDICT)
    except FileNotFoundError:
        pass
    except OSError as exc:
        raise InputError(f"cannot delete the stale verdict {VERDICT}: {exc}") from exc


def load_plan_floor():
    path = PLAN_FLOOR.resolve()
    if not path.is_file():
        raise InputError(f"the plan floor {path} is missing")
    sys.dont_write_bytecode = True  # leave no __pycache__ next to the plan floor
    try:
        spec = importlib.util.spec_from_file_location("msw_plan_floor", path)
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
    except Exception as exc:
        message = " ".join(str(exc).split())
        raise InputError(f"cannot load the plan floor {path}: {type(exc).__name__}: {message}") from exc
    return module


def parse_args(argv: list) -> dict:
    args = {}
    i = 0
    while i < len(argv):
        name = argv[i]
        if name not in ARGS:
            raise InputError(f"unknown argument {name!r}; {USAGE}")
        if name in args:
            raise InputError(f"{name} given twice; {USAGE}")
        if i + 1 >= len(argv) or argv[i + 1] == "" or argv[i + 1] in ARGS:
            raise InputError(f"{name} needs a value; {USAGE}")
        args[name] = argv[i + 1]
        i += 2
    missing = [name for name in ARGS if name not in args]
    if missing:
        raise InputError(f"missing {', '.join(missing)}; {USAGE}")
    if args["--base"].startswith("-"):
        raise InputError(f"--base must not start with '-': {args['--base']!r}")
    return {
        "decomposition": args["--decomposition"],
        "issue_document": args["--issue-document"],
        "base": args["--base"],
    }


# --- the children block ----------------------------------------------------


def children_fence(text: str) -> str:
    """The body of the yaml fence that opens `## Children`; raises BlockError."""
    try:
        _, body = P.section(P.document_lines(text), "Children", "the decomposition")
    except P.DocumentError as exc:
        raise BlockError(str(exc)) from exc
    start = next((i for i, line in enumerate(body) if line.strip()), None)
    if start is None or body[start].rstrip() != "```yaml":
        raise BlockError("the decomposition: '## Children' does not start with a '```yaml' fence")
    end = next((i for i in range(start + 1, len(body)) if body[i].rstrip() == "```"), None)
    if end is None:
        raise BlockError("the decomposition: the '## Children' yaml fence is not closed by a '```' line")
    return "".join(line + "\n" for line in body[start + 1 : end])


def parse_children(text: str) -> list:
    """The children of the decomposition, their shape checked; raises BlockError."""
    try:
        proc = subprocess.run(
            ["yq", "-p=yaml", "-o=json", "."], input=children_fence(text).encode(), capture_output=True
        )
    except OSError as exc:
        raise P.InputError(f"cannot run yq: {exc}") from exc
    if proc.returncode != 0:
        detail = proc.stderr.decode("utf-8", errors="replace").strip().splitlines()
        raise BlockError(f"the children block does not parse: {P.flat(detail[-1]) if detail else proc.returncode}")
    try:
        value = P.strict_json(proc.stdout.decode("utf-8"))
    except (UnicodeDecodeError, ValueError) as exc:
        raise BlockError(f"the children block is not a single yaml document: {P.flat(str(exc))}") from exc
    if not isinstance(value, dict) or "children" not in value:
        raise BlockError("the children block is not a mapping with a 'children' key")
    children = value["children"]
    if not isinstance(children, list) or not children:
        raise BlockError("'children' is not a non-empty list")
    keys = set()
    for n, child in enumerate(children, 1):
        if not isinstance(child, dict):
            raise BlockError(f"child {n} is not a mapping")
        key = child.get("key")
        if not isinstance(key, str) or not key:
            raise BlockError(f"child {n} has no 'key' that is a non-empty string")
        if key in keys:
            raise BlockError(f"the key {key!r} is used by more than one child")
        keys.add(key)
        deps = child.get("depends_on")
        if not isinstance(deps, list) or not all(isinstance(d, str) for d in deps):
            raise BlockError(f"child {key!r}: 'depends_on' is not a list of strings")
    return children


def kind_reasons(children: list) -> list:
    return [
        f"child {c['key']!r}: kind {c.get('kind')!r} is not 'implementation' or 'plan'"
        for c in children
        if not (isinstance(c.get("kind"), str) and c["kind"] in KINDS)
    ]


def component_reasons(children: list) -> list:
    reasons = []
    for c in children:
        if c.get("kind") != "implementation":
            continue
        key = c["key"]
        if "component" in c:
            reasons.append(
                f"child {key!r}: has a 'component' key; an implementation child names its components "
                "in a 'components' list"
            )
            continue
        names = c.get("components")
        if not isinstance(names, list) or not names or not all(isinstance(n, str) and n for n in names):
            reasons.append(f"child {key!r}: an implementation child needs a non-empty 'components' list of names")
            continue
        for index, name in enumerate(names):
            if name in names[index + 1 :] and name not in names[:index]:
                reasons.append(f"child {key!r}: components names {name!r} twice")
    return reasons


def known_reasons(children: list, components: list) -> list:
    reasons = []
    for c in children:
        names = c.get("components")
        if not isinstance(names, list):
            continue
        for name in dict.fromkeys(n for n in names if isinstance(n, str)):
            if name not in components:
                reasons.append(f"child {c['key']!r}: component {name!r} is not in the base config")
    return reasons


def sibling_reasons(children: list) -> list:
    keys = [c["key"] for c in children]
    return [
        f"child {c['key']!r}: depends_on names {dep!r}, which is not a sibling"
        for c in children
        for dep in c["depends_on"]
        if dep not in keys
    ]


def cycle_reasons(children: list) -> list:
    """One reason per back edge found by a three-colour DFS over sibling edges."""
    edges = {c["key"]: [d for d in c["depends_on"] if d in {x["key"] for x in children}] for c in children}
    colour = {key: "white" for key in edges}
    path = []
    reasons = []

    def visit(key: str) -> None:
        colour[key] = "grey"
        path.append(key)
        for dep in edges[key]:
            if colour[dep] == "grey":
                cycle = path[path.index(dep) :] + [dep]
                reasons.append(f"the dependencies form a cycle: {', '.join(cycle)}")
            elif colour[dep] == "white":
                visit(dep)
        path.pop()
        colour[key] = "black"

    for key in edges:
        if colour[key] == "white":
            visit(key)
    return reasons


# --- coverage --------------------------------------------------------------


def coverage_reasons(issue: str, decomposition: str) -> list:
    try:
        _, body = P.section(P.issue_body(issue), "Acceptance criteria", "the issue document")
        numbers = [m.group(1) for m in map(CRITERION.match, body) if m]
        if not numbers:
            raise P.DocumentError("the issue document: '## Acceptance criteria' holds no numbered criterion")
        _, coverage = P.section(P.document_lines(decomposition), "Coverage", "the decomposition")
    except P.DocumentError as exc:
        return [str(exc)]
    start = next((i for i, line in enumerate(coverage) if line.strip()), len(coverage))
    table = []
    for line in coverage[start:]:
        if not line.lstrip(" \t").startswith("|"):
            break
        table.append(line)
    if len(table) < 2 or not all(P.SEPARATOR_CELL.fullmatch(cell) for cell in P.table_cells(table[1])):
        return ["the decomposition: '## Coverage' does not start with a table header and a '---' row"]
    rows = [P.table_cells(line) for line in table[2:]]
    reasons = []
    for n in numbers:
        if not any(
            len(cells) >= 2 and cells[0].strip().startswith((f"{n}.", f"{n})")) and cells[1].strip() for cells in rows
        ):
            reasons.append(f"criterion {n} of the issue has no Coverage row")
    return reasons


# --- verdict ---------------------------------------------------------------


def conclude(text: str, issue: str, components: list) -> tuple:
    results = []
    skipped = []

    def record(rule: str, reasons: list) -> None:
        results.append({"rule": rule, "result": "hit" if reasons else "clean", "reasons": reasons})

    try:
        children = parse_children(text)
    except BlockError as exc:
        record("children-block", [str(exc)])
        skipped = list(CHILD_RULES)
    else:
        record("children-block", [])
        record("child-kind", kind_reasons(children))
        record("child-component", component_reasons(children))
        record("known-components", known_reasons(children, components))
        record("depends-on-siblings", sibling_reasons(children))
        record("no-cycles", cycle_reasons(children))
    record("coverage", coverage_reasons(issue, text))
    return results, skipped


def floor(argv: list) -> int:
    global P
    remove_stale_verdict()
    args = parse_args(argv)
    P = load_plan_floor()
    with P.job_object_store({"base": args["base"]}) as (store, commits):
        commit = commits["base"]
        config = P.load_config(commit, git_dir=store)
        if not isinstance(config.get("components"), dict):
            raise InputError(f"config: components at {commit} is not a mapping")
        components = sorted(config["components"])
        data = P.read_document(args["decomposition"], "decomposition")
        issue = P.read_document(args["issue_document"], "issue document")

    results, skipped = conclude(data.decode("utf-8"), issue.decode("utf-8"), components)
    hit = any(r["result"] == "hit" for r in results)
    verdict = {
        "tool": TOOL,
        "version": VERSION,
        "decomposition": {"path": args["decomposition"], "sha256": hashlib.sha256(data).hexdigest()},
        "issue_document": {"path": args["issue_document"], "sha256": hashlib.sha256(issue).hexdigest()},
        "base": {"ref": args["base"], "commit": commit},
        "config": P.CONFIG,
        "components": components,
        "verdict": "hit" if hit else "clean",
        "rules": results,
        "not_evaluated": skipped,
    }
    P.write_verdict(verdict)
    for r in results:
        line = f"{r['result']:7} {r['rule']}"
        if r["reasons"]:
            line += f": {r['reasons'][0]}"
        print(line)
    print(f"floor: {verdict['verdict']}")
    return 1 if hit else 0


def main(argv: list) -> int:
    try:
        return floor(argv)
    except Exception as exc:  # a crash must never read as a hit or clean
        message = " ".join(str(exc).split())
        try:
            os.unlink(VERDICT)
        except Exception:  # the handler itself must always end in exit 2
            pass
        if isinstance(exc, InputError) or (P is not None and isinstance(exc, P.InputError)):
            print(f"floor.py: {message}", file=sys.stderr)
        else:
            print(f"floor.py: unexpected error: {type(exc).__name__}: {message}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
