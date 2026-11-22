# Plan: Fix stale references in CONTRIBUTING.md and the layout document

Fixture modelled on the plan of #20 (component `docs`), for selftest.py.

## Approach

Derived from the approved plan of #20, merged as bb7f812, component `docs`. `files` follows the plan's Steps, checked against the merged commit: `CONTRIBUTING.md` and `docs/architecture/repository-layout.md`, neither new. `estimated_lines` is 6, the `git show --numstat --no-renames bb7f812` total. Nothing else is declared.

## Steps

1. **Rewrite the "Reserved prefix" paragraph** — files: `CONTRIBUTING.md`.
2. **Fix the commit example** — files: `CONTRIBUTING.md`.
3. **Reword the `dogfood/` entry** — files: `docs/architecture/repository-layout.md`.

## Footprint

```json
{
  "footprint": {
    "files": [
      "CONTRIBUTING.md",
      "docs/architecture/repository-layout.md"
    ],
    "new_files": [],
    "estimated_lines": 6,
    "new_dependencies": [],
    "external_steps": [],
    "gate_changes": [],
    "overridden_assumptions": []
  }
}
```
