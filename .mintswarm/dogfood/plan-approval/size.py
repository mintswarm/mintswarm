#!/usr/bin/env python3
"""Read the `## Size` of an analysis or a plan, and print a plan's size for the approval package.

    size.py analysis PATH
    size.py plan PATH

Dogfooding stand-in for the control plane's size estimate. PATH is read as
UTF-8, relative to the working directory. The `## Size` section is the one
the analyse and plan stages write (`.mintswarm/stages/analyse.md` and
`.mintswarm/stages/plan.md`, rule 12):

- lines are split at LF, and a trailing CR is dropped;
- there is exactly one `## Size` heading outside fenced blocks, found as
  floor.py finds headings, and the section runs to the next `# ` or `## `
  heading outside a fence;
- the heading just before it is `## Split` in an analysis and
  `## Footprint` in a plan;
- ignoring blank lines, its first line is exactly `size: V`, V one of
  XS, S, M, L, XL, 2XL, 3XL, 4XL, case-sensitive;
- then come one to three lines, each `- ` followed by non-blank text, with
  blank lines allowed; any other line (prose, an indented continuation
  line, a nested bullet, a fence line) makes the section malformed.

Modes:

- `analysis`: exit 0 and print `size: V` when the section is usable;
  exit 1 and print one line with the reason when it is missing or
  malformed.
- `plan`: read the plan's `## Size` and its footprint, as floor.py reads it,
  whose `production_lines` (or `estimated_lines` in a seven-key footprint)
  gives the lines floor below. Exit 0 and print the approval package's
  size lines:
  - `size: <judged>` when the judged size is at or above the floor;
  - `size: <floor>`, then `size raised from <judged> to <floor> by planned
    lines` when it is below;
  - `size: <floor>`, then `the plan states no usable size; the size shown
    is the lines floor` when the section is missing or malformed.

Exit 2, with one line on stderr and nothing on stdout, on bad arguments,
an unknown mode, a file that cannot be read or is not UTF-8, an unusable
footprint in `plan` mode, or any unexpected error. Standard library only.
"""

import importlib.util
import sys
from pathlib import Path

USAGE = "usage: size.py analysis PATH | size.py plan PATH"
SIZES = ("XS", "S", "M", "L", "XL", "2XL", "3XL", "4XL")
# The lines floor of design §5 (mintswarm/mintswarm#299, artifact 5): the
# smallest size whose upper bound holds the production lines; 4XL above 7000.
BANDS = ((25, "XS"), (75, "S"), (200, "M"), (500, "L"), (1200, "XL"), (3000, "2XL"), (7000, "3XL"))
AFTER = {"analysis": "## Split", "plan": "## Footprint"}
NO_SIZE = "the plan states no usable size; the size shown is the lines floor"


class UsageError(Exception):
    pass


class SizeError(Exception):
    pass


def load_floor():
    sys.dont_write_bytecode = True  # leave no __pycache__ next to the scripts
    path = Path(__file__).resolve().parent / "floor.py"
    spec = importlib.util.spec_from_file_location("msw_floor", path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def floor_size(lines: int) -> str:
    for bound, size in BANDS:
        if lines <= bound:
            return size
    return SIZES[-1]


def read_size(lines: list, after: str, floor) -> str:
    """The size the one `## Size` section states; raises SizeError on a missing or malformed section."""
    marks = floor.headings(lines)
    starts = [n for n, (i, level) in enumerate(marks) if level == 2 and lines[i].rstrip() == "## Size"]
    if not starts:
        raise SizeError("no '## Size' section")
    if len(starts) > 1:
        raise SizeError(f"{len(starts)} '## Size' sections")
    n = starts[0]
    previous = lines[marks[n - 1][0]].rstrip() if n > 0 else None
    if previous != after:
        raise SizeError(f"'## Size' must follow '{after}', it follows {repr(previous) if previous else 'nothing'}")
    start = marks[n][0]
    end = marks[n + 1][0] if n + 1 < len(marks) else len(lines)
    body = [(i, lines[i]) for i in range(start + 1, end) if lines[i].strip()]
    if not body:
        raise SizeError("'## Size' is empty")
    i, first = body[0]
    if not first.startswith("size: ") or first[len("size: ") :] not in SIZES:
        raise SizeError(f"line {i + 1}: the first line must be exactly 'size: <{'|'.join(SIZES)}>'")
    reasons = body[1:]
    for i, line in reasons:
        if not line.startswith("- ") or not line[2:].strip():
            raise SizeError(f"line {i + 1}: '## Size' holds only the size line and '- ' reasons")
    if not 1 <= len(reasons) <= 3:
        raise SizeError(f"'## Size' has {len(reasons)} reasons, expected one to three")
    return first[len("size: ") :]


def planned_lines(text: str, floor) -> int:
    """The footprint's production_lines, which floor.py sets to estimated_lines in a seven-key footprint."""
    try:
        return floor.parse_footprint(text)["production_lines"]
    except floor.FootprintError as exc:
        raise UsageError(f"the plan's footprint is unusable: {exc}") from exc


def run(argv: list) -> tuple:
    """(exit code, stdout lines)."""
    if len(argv) != 2:
        raise UsageError(USAGE)
    mode, path = argv
    if mode not in AFTER:
        raise UsageError(f"unknown mode {mode!r}; {USAGE}")
    try:
        text = Path(path).read_bytes().decode("utf-8")
    except (OSError, UnicodeDecodeError) as exc:
        raise UsageError(f"cannot read {path}: {exc}") from exc
    floor = load_floor()
    lines = floor.document_lines(text)
    if mode == "analysis":
        try:
            return 0, [f"size: {read_size(lines, AFTER[mode], floor)}"]
        except SizeError as exc:
            return 1, [f"unusable size: {exc}"]
    lowest = floor_size(planned_lines(text, floor))
    try:
        judged = read_size(lines, AFTER[mode], floor)
    except SizeError:
        return 0, [f"size: {lowest}", NO_SIZE]
    if SIZES.index(judged) >= SIZES.index(lowest):
        return 0, [f"size: {judged}"]
    return 0, [f"size: {lowest}", f"size raised from {judged} to {lowest} by planned lines"]


def main(argv: list) -> int:
    try:
        code, out = run(argv)
    except UsageError as exc:
        print(f"size.py: {exc}", file=sys.stderr)
        return 2
    except Exception as exc:  # a crash must never read as a size
        message = " ".join(str(exc).split())
        print(f"size.py: unexpected error: {type(exc).__name__}: {message}", file=sys.stderr)
        return 2
    for line in out:
        print(line)
    return code


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
