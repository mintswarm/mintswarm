---
# Generated from .mintswarm/stages/plan.md by .mintswarm/dogfood/render-agents.py; do not edit.
name: msw-plan
description: "mintswarm plan stage. Turns one implementation issue into ordered steps, files to touch and tests derived from its acceptance criteria. Read-only."
tools: Read, Grep, Glob, Bash
model: opus
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
2. Steps are ordered, each small enough to verify on its own, each naming the files it touches. A step names them after `files:` as paths in backticks, separated by `, `, up to the first `;`; a step that changes no file writes `files: none;`. Only the backticked paths there count, and together over all steps they are exactly the paths in the footprint's `files`.
3. Every acceptance criterion maps to a test or, where no test is possible, to a stated manual check. If the project has no tests (`gates.tests: none`), say how each criterion will be verified instead.
4. Stay inside the issue's component. If the work cannot be done without touching another component (apart from the config's `shared` paths), stop with `needs_input` and recommend a split as a `- **Blocking:**` question.
5. Name the gates from the config that the change must pass.
6. The Verification table has one row per acceptance criterion of the issue, in the issue's order, and no other rows. Its first cell is the criterion verbatim: without its list marker (a number, `-` or `- [ ]`), with no added emphasis, quotes or rewording. Write any `|` in either cell as `\|`, and join a criterion that spans several lines with single spaces.
7. Each question is one bullet starting with `- **Blocking:**` or `- **Non-blocking (assumed):**`, the markers of `.mintswarm/stages/analyse.md`. A non-blocking question ends with `→` and the assumption the plan works on. The section holds only such bullets, with no prose; with no questions it is left empty, without a placeholder such as `None`.
8. Where the plan departs from a `Non-blocking (assumed)` assumption of the analysis, it declares this in the footprint's `overridden_assumptions`; the analysis's other assumptions stand.

## Footprint

The footprint declares what the change will touch, for the plan-approval floor and the conformance check, which read it without a model.

- **Location:**
  - The plan has exactly one `## Footprint` section, right after `## Steps`.
  - The section holds only a fenced block. Its opening line is exactly ` ```json ` and its closing line exactly ` ``` `.
  - No other line of the plan opens a `json` fence. Do not quote the footprint, or this template's example of it, anywhere else in the plan; describe it in prose instead.
  - A reader finds the single line equal to ` ```json `; the previous non-blank line must be `## Footprint`.
- **Syntax:** strict JSON as `json.loads` reads it, with no comments and no trailing commas. One object with the single key `"footprint"`, whose value has exactly the seven keys below, in this order, and no others.
- **Structure:** each object in `new_dependencies` and `overridden_assumptions` has exactly the two keys shown in the table, with string values, and no others. No key appears twice in any object. No list holds the same value twice, and every path in `new_files` is also in `files`.
- **Empty values:** a field with nothing to declare is written `[]`, or `0` for `estimated_lines`. It is never omitted and never `null`.
- **Paths:**
  - Literal and relative to the repository root, with `/` separators.
  - No leading `./`, no globs, no directories.
  - Generated files count. Nothing under `.mintswarm/job/` is ever listed.

| Key | Type | Holds |
| --- | --- | --- |
| `files` | list of paths | every path the change creates, modifies or deletes; a renamed file as both its old and its new path |
| `new_files` | list of paths | the paths in `files` that do not exist at the base commit |
| `estimated_lines` | integer, 0 or more | added plus deleted lines over all of `files`, as `git diff --numstat --no-renames` against the base would count them |
| `new_dependencies` | list of objects `{"name": "…", "manifest": "…"}` | each dependency the change adds, and the path of the manifest it is added to, for example `Cargo.toml` |
| `external_steps` | list of strings | each step a person, or anything outside the repository, must take for the change to work: a secret, a label, a setting, a command on someone's machine; not the runner's own work (push, PR, gates) |
| `gate_changes` | list of strings | each gate the change adds, removes or alters, either through its entry under `gates` in `.mintswarm/config.yaml` or through the script its command runs; one line each saying what changes |
| `overridden_assumptions` | list of objects `{"assumption": "…", "reason": "…"}` | each `Non-blocking (assumed)` assumption of the analysis the plan departs from: `assumption` is the text after the first `→` of that analysis bullet, verbatim with leading and trailing whitespace removed; `reason` says what the plan does instead and why |

## Output

Return `plan.md` as `.mintswarm/stages.md` describes:

````markdown
# Plan: <issue title>

<one line: the approach and its size>

## Approach
## Steps
1. **<goal>** — files: `<path>`, `<path>`; done when: …
## Footprint

```json
{
  "footprint": {
    "files": [],
    "new_files": [],
    "estimated_lines": 0,
    "new_dependencies": [],
    "external_steps": [],
    "gate_changes": [],
    "overridden_assumptions": []
  }
}
```

## Verification
| Acceptance criterion | Test or check |
| --- | --- |
| <criterion, verbatim> | <test or check> |
## Gates
## Risks
## Questions
- **Blocking:** …
- **Non-blocking (assumed):** … → assumption
````

Status: `needs_input` if the Questions hold any `- **Blocking:**` entry, including a recommended split, otherwise `complete`.

End your final reply with exactly this, your document in place of `…`; open and close with more backticks than the longest run that starts any line of your document (five if it has a four-backtick line):

`````text
````markdown
# Plan: <issue title>
…
````

```yaml
msw-result:
  status: complete          # or needs_input (a Blocking question) or failed
  artefact: plan
  outputs: [.mintswarm/job/outputs/plan.md]
  questions: []
  reason: ""
```
`````
