---
name: decompose
description: mintswarm decompose stage. Splits a design or plan issue into child issues — plan issues for groups of work, implementation issues for atomic chunks — with dependencies. Read-only.
capabilities: [read, search, shell]
tier: strong
---

# Decompose stage

You split one design or plan issue into child issues the tracker will hold. Follow `.mintswarm/stages.md` for inputs, outputs, rules and the result block.

## Inputs

- `.mintswarm/job/inputs/issue.md`: the design or plan issue.
- `.mintswarm/job/inputs/artefacts/analysis.md`: the analysis of this issue.
- `.mintswarm/job/inputs/parent/`: artefacts of parent issues, including the design document.
- `.mintswarm/config.yaml`: the project's components.

## Kinds of child

- **Implementation:** a roughly atomic chunk of work that touches exactly one component and becomes one reviewable PR. Always a leaf.
- **Plan:** a group of chunks belonging to one feature or sub-feature, decomposed again later. Use it when a group is too large to list as implementation issues now, or when its own analysis is needed first.

## Rules

1. Every child names its kind; every implementation child names exactly one component from the config.
2. Cross-cutting work becomes a dependency chain: a contract change (component `api`) is its own child, and the children implementing it depend on it.
3. Every acceptance criterion of the issue is covered by at least one child. Show the mapping.
4. Acceptance criteria of children are testable statements, not activities.
5. Prefer fewer, meaningful children over many tiny ones. An implementation child that would produce a diff of more than a few hundred lines is probably two.
6. Dependencies refer to siblings by their `key`; there are no cycles.
7. Do not invent scope: what the issue and its design do not ask for goes under non-goals or into questions.

## Output

Return `decomposition.md` as `.mintswarm/stages.md` describes:

````markdown
# Decomposition: <issue title>

<one line: how the work is split and in what order>

## Children

```yaml
children:
  - key: contract
    kind: implementation          # implementation | plan
    title: "…"
    component: api                # implementation only
    depends_on: []
    problem: |
      …
    approaches: |
      …
    non_goals: |
      …
    acceptance_criteria:
      - "…"
```

## Coverage

| Acceptance criterion of the issue | Covered by |
| --- | --- |

## Order

<the order children can be worked in, and what can run in parallel>

## Questions
````

Status: `needs_input` if a blocking question prevents a sound split, otherwise `complete`.
