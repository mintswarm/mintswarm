# Plan: CODEOWNERS for the maintainer

Fixture modelled on the plan of #19 (component `tooling`), for selftest.py.

## Approach

Derived from the approved plan of #19, merged as 4e7ba0a, component `tooling`. `files` follows the plan's Steps, checked against the merged commit: the new `.github/CODEOWNERS`. `estimated_lines` is 2, the `git show --numstat --no-renames 4e7ba0a` total. One external step is needed for the change to work: the maintainer turns on Code Owner review in the `master` ruleset. The runner's push, PR and errors checks are checks or publishing, not external steps.

## Steps

1. **Add the file** — files: `.github/CODEOWNERS` (new).
2. **Maintainer: turn on Code Owner review** — files: none.

## Footprint

```json
{
  "footprint": {
    "files": [
      ".github/CODEOWNERS"
    ],
    "new_files": [
      ".github/CODEOWNERS"
    ],
    "estimated_lines": 2,
    "new_dependencies": [],
    "external_steps": [
      "After merge, the maintainer turns on Code Owner review (require_code_owner_review) in the master ruleset under their own admin login."
    ],
    "gate_changes": [],
    "overridden_assumptions": []
  }
}
```
