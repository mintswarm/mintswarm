# Plan: Gate results in CTRF, with pluggable report converters

Fixture modelled on the plan of #12 (component `agents`), for selftest.py.

## Approach

Derived from the approved plan of #12, merged as a19e259, component `agents`. `files` follows the plan's Steps, checked against the merged commit: 10 files, two of them new (`reporters/ctrf-to-junit.py` and `report-gates.py`). `estimated_lines` is 920, the `git show --numstat --no-renames a19e259` total. `gate_changes` names `agents-drift`, whose script `render-agents.py` changes. No external steps: the session's `npx ctrf-cli` run is a check, not a step the change needs.

## Steps

1. **CTRF, verdict and failures** — files: `.mintswarm/dogfood/run-gates.py`.
2. **Built-in JUnit converter** — files: `.mintswarm/dogfood/reporters/ctrf-to-junit.py` (new).
3. **Converter runner** — files: `.mintswarm/dogfood/report-gates.py` (new).
4. **Config setting** — files: `.mintswarm/config.yaml`.
5. **Gates stage definition** — files: `.mintswarm/stages/gates.md`, the regenerated `.claude/agents/msw-gates.md`.
6. **Stage conventions** — files: `.mintswarm/stages.md`.
7. **Text after the result block** — files: `.mintswarm/dogfood/save-output.py`.
8. **Line separators in frontmatter** — files: `.mintswarm/dogfood/render-agents.py`, `.mintswarm/stages/README.md`.

## Footprint

```json
{
  "footprint": {
    "files": [
      ".mintswarm/dogfood/run-gates.py",
      ".mintswarm/dogfood/reporters/ctrf-to-junit.py",
      ".mintswarm/dogfood/report-gates.py",
      ".mintswarm/config.yaml",
      ".mintswarm/stages/gates.md",
      ".claude/agents/msw-gates.md",
      ".mintswarm/stages.md",
      ".mintswarm/dogfood/save-output.py",
      ".mintswarm/dogfood/render-agents.py",
      ".mintswarm/stages/README.md"
    ],
    "new_files": [
      ".mintswarm/dogfood/reporters/ctrf-to-junit.py",
      ".mintswarm/dogfood/report-gates.py"
    ],
    "estimated_lines": 920,
    "new_dependencies": [],
    "external_steps": [],
    "gate_changes": [
      "agents-drift: its script render-agents.py changes"
    ],
    "overridden_assumptions": []
  }
}
```
