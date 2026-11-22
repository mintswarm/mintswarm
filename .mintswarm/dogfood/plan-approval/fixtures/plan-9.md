# Plan: Stage agents return their output documents as text

Fixture modelled on the plan of #9 (component `agents`), for selftest.py.

## Approach

Derived from the approved plan of #9, merged as e867c3f, component `agents`. `files` follows the plan's Steps, checked against the merged commit: 16 files, one of them new (`save-output.py`). `estimated_lines` is 342, the `git show --numstat --no-renames e867c3f` total. `gate_changes` names `agents-drift`, whose script `render-agents.py` changes. No external steps: the agent reload after merge is a check, not a step the change needs.

## Steps

1. **Renderer** — files: `.mintswarm/dogfood/render-agents.py`.
2. **Save script** — files: `.mintswarm/dogfood/save-output.py` (new).
3. **Convention** — files: `.mintswarm/stages.md`.
4. **Stage definitions** — files: `.mintswarm/stages/analyse.md`, `.mintswarm/stages/decompose.md`, `.mintswarm/stages/plan.md`, `.mintswarm/stages/review.md`, `.mintswarm/stages/gates.md`, `.mintswarm/stages/implement.md`, and the regenerated `.claude/agents/msw-analyse.md`, `.claude/agents/msw-decompose.md`, `.claude/agents/msw-plan.md`, `.claude/agents/msw-review.md`, `.claude/agents/msw-gates.md`, `.claude/agents/msw-implement.md`.
5. **Vocabulary and README** — files: `.mintswarm/stages/README.md`.

## Footprint

```json
{
  "footprint": {
    "files": [
      ".mintswarm/dogfood/render-agents.py",
      ".mintswarm/dogfood/save-output.py",
      ".mintswarm/stages.md",
      ".mintswarm/stages/analyse.md",
      ".mintswarm/stages/decompose.md",
      ".mintswarm/stages/plan.md",
      ".mintswarm/stages/review.md",
      ".mintswarm/stages/gates.md",
      ".mintswarm/stages/implement.md",
      ".claude/agents/msw-analyse.md",
      ".claude/agents/msw-decompose.md",
      ".claude/agents/msw-plan.md",
      ".claude/agents/msw-review.md",
      ".claude/agents/msw-gates.md",
      ".claude/agents/msw-implement.md",
      ".mintswarm/stages/README.md"
    ],
    "new_files": [
      ".mintswarm/dogfood/save-output.py"
    ],
    "estimated_lines": 342,
    "new_dependencies": [],
    "external_steps": [],
    "gate_changes": [
      "agents-drift: its script render-agents.py changes"
    ],
    "overridden_assumptions": []
  }
}
```
