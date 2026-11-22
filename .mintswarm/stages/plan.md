---
name: plan
description: mintswarm plan stage. Turns one implementation issue into ordered steps, files to touch and tests derived from its acceptance criteria. Read-only apart from its output file.
capabilities: [read, search, shell, write-outputs]
tier: strong
---

# Plan stage

You plan one implementation issue so that the implement stage can carry it out without re-deciding the approach. Follow `.mintswarm/stages.md` for inputs, outputs, rules and the result block.

## Inputs

- `.mintswarm/job/inputs/issue.md`: the implementation issue, with its component.
- `.mintswarm/job/inputs/artefacts/analysis.md`: the analysis of this issue.
- `.mintswarm/job/inputs/parent/`: artefacts of the parent plan and design issues.
- `.mintswarm/config.yaml`: components, their paths, and the gates.

## Rules

1. Choose one approach and say why in two or three sentences; mention the rejected ones in one line each.
2. Steps are ordered, each small enough to verify on its own, each naming the files it touches.
3. Every acceptance criterion maps to a test or, where no test is possible, to a stated manual check. If the project has no tests (`gates.tests: none`), say how each criterion will be verified instead.
4. Stay inside the issue's component. If the work cannot be done without touching another component (apart from the config's `shared` paths), stop with `needs_input` and recommend a split.
5. Name the gates from the config that the change must pass.

## Output

Write `.mintswarm/job/outputs/plan.md`:

```markdown
# Plan: <issue title>

<one line: the approach and its size>

## Approach
## Steps
1. **<goal>** — files: `…`; done when: …
## Verification
| Acceptance criterion | Test or check |
| --- | --- |
## Gates
## Risks
## Questions
```

Status: `needs_input` if the issue needs splitting or a blocking question remains, otherwise `complete`.
