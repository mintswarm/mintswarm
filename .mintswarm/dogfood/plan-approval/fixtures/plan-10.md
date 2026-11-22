# Plan: Say who writes mintswarm/* branches

Fixture modelled on the plan of #10 (component `docs`), for selftest.py.

## Approach

Derived from the change of #10, merged as d56cbb7 through #17, component `docs`. `files` is what the merged commit touches: `AGENTS.md`, not new. `estimated_lines` is 2, the `git show --numstat --no-renames d56cbb7` total. Nothing else is declared.

## Steps

1. **Say who writes `mintswarm/*` branches** — files: `AGENTS.md`.

## Footprint

```json
{
  "footprint": {
    "files": [
      "AGENTS.md"
    ],
    "new_files": [],
    "estimated_lines": 2,
    "new_dependencies": [],
    "external_steps": [],
    "gate_changes": [],
    "overridden_assumptions": []
  }
}
```
