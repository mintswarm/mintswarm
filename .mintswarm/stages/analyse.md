---
name: analyse
description: mintswarm analyse stage. Studies one issue (design, plan or implementation) against the repository and its parent artefacts and returns an analysis. Read-only.
capabilities: [read, search, shell]
tier: strong
---

# Analyse stage

You analyse one issue so that the next stage (design, decompose or plan) can work from facts rather than guesses. Follow `.mintswarm/stages.md` for inputs, outputs, rules and the result block.

## Inputs

- `.mintswarm/job/inputs/issue.md`: the issue, its kind and, for implementation issues, its component.
- `.mintswarm/job/inputs/parent/`: artefacts of the parent plan and design issues, if any.
- `.mintswarm/config.yaml`: components and their paths.
- The repository at the job's base commit.

## What to do

1. Restate what is asked in your own words, and what is explicitly not asked.
2. Find what already exists: code, documents in `docs/architecture/`, conventions in `AGENTS.md` and `CONTRIBUTING.md`, earlier artefacts. Cite paths.
3. Name the components the work touches. For an implementation issue that should be exactly one; say so if it is not. Record the result in `## Split`: for an implementation issue that spans more than one component, or that should otherwise be split into one-component children, set `proposed` to `true`, `reason` to `components` or `other`, `components` to the components it spans and `finding` to the one-line finding that calls for the split. Otherwise set `proposed` to `false`, `reason` to `none` and `finding` to an empty string. The section is always there; `proposed` counts only for implementation issues.
4. Estimate the size in `## Size`. For an implementation issue, the size is the expected cost of its phase 2: implement, the gates, review and address rounds, and landing. For a design or plan issue, it is the judged size of the work the issue covers. A size states no wall-clock time.
5. Outline the approaches worth considering, with their main trade-off each. Do not choose for the plan stage unless one is clearly forced.
6. List risks and unknowns.
7. List questions in `## Questions`, each as one bullet with one of three markers. A question the next stage cannot proceed sensibly without is a `Must decide` or a `Recommended`; everything else is an `Assumption`:
    - `- **Must decide:** <question>`: a person must answer; there is no recommendation, or only a weak one.
    - `- **Recommended:** <question> → <answer>`: a person should confirm the recommended answer.
    - `- **Assumption:** <question> → <answer>`: you went ahead with the answer, and the next stage works on it.

    Under each bullet, indented sub-bullets follow in this order: `Why:`; then `Options:` on a `Must decide` or `Recommended`, or `Alternatives:` on an `Assumption`, where it is required; then `Recommendation:` on a `Must decide`, an optional weak recommendation, or `Reason:` on the other two. `Shape:` (`one`, `several`, `yes_no` or `open`) and `References:` are optional. Options are written `(a) <label>: <consequence; trade-off> · (b) …`, and their letters are the option ids. The answer after `→` starts with the letter of the chosen option, for example `→ (a) …`; on an `Assumption` the answer is option `(a)`, and `Alternatives:` lists `(b)` onwards. With `Shape: yes_no` the answer is `yes` or `no` and no options are listed; an `Assumption` then names the other answer under `Alternatives:`. The section holds only such bullets, with no prose, and any continuation line or sub-bullet is indented; with no questions it is left empty, without a placeholder such as `None`.

    Each bullet corresponds to one object in the result block's `questions` ("Questions" in `.mintswarm/stages.md`), in the same order and with the same kind (`must_decide`, `recommended` or `assumption`) and question text:
    - `id` is `q1`, `q2` and so on, in bullet order;
    - `question` is the text after the marker, up to the first ` → ` when there is one;
    - `Why:` gives `why`; `Shape:` gives `shape`, which without it is `one` when options are listed and `open` when none are;
    - `Options:` gives `options`; on an `Assumption`, `options` holds `(a)`, the answer, followed by the `Alternatives:`, except with `Shape: yes_no`, which lists none;
    - the `→` answer gives `recommendation.choice` and `Reason:` gives `recommendation.reason`; `Recommendation:` on a `Must decide` gives its optional weak `recommendation`;
    - `References:` gives `references`.

    The status is `needs_input` exactly when a `Must decide` or `Recommended` bullet is present, and otherwise `complete` (or `failed`).

## Output

Return `analysis.md` as `.mintswarm/stages.md` describes:

````markdown
# Analysis: <issue title>

<one line: the essence of the task and its main constraint>

## What is asked
## What is not asked
## Existing context
## Components
## Split

```json
{"split": {"proposed": false, "reason": "none", "components": ["…"], "finding": ""}}
```

## Size
size: M
- <reason>
## Approaches
## Risks
## Questions
- **Must decide:** <question>
  - Why: <what it blocks, what changes with the answer>
  - Options: (a) <label>: <consequence; trade-off> · (b) …
  - Recommendation: <weak recommendation, if any> (<reason>)
- **Recommended:** <question> → (a) <recommended answer>
  - Why: …
  - Options: (a) … · (b) …
  - Reason: …
- **Assumption:** <question> → (a) <what the stage goes on with>
  - Why: …
  - Alternatives: (b) … · (c) …
  - Reason: …
````

`## Split` holds exactly one json fence with that object: `proposed` is a boolean, `reason` is `components`, `other` or `none`, `components` is a list of component names and `finding` is one line.

`## Size` comes right after `## Split` and opens no fence. Ignoring blank lines, its first line is exactly `size: <value>`, the value one of `XS`, `S`, `M`, `L`, `XL`, `2XL`, `3XL` or `4XL`, as written (no other case, no leading or trailing text; trailing spaces and tabs on any line of the section are ignored); then come one to three reasons, each one line starting with `- `, and nothing else: no prose, continuation line, nested bullet or fence. A missing or malformed section makes the analysis unusable, as a malformed `## Split` does.

Status: `needs_input` exactly when the Questions hold a `Must decide` or `Recommended` bullet, otherwise `complete`.

End your final reply with exactly this, your document in place of `…`; open and close with more backticks than the longest run that starts any line of your document (five if it has a four-backtick line):

`````text
````markdown
# Analysis: <issue title>
…
````

```yaml
msw-result:
  status: complete          # or needs_input (a Must decide or Recommended question) or failed
  artefact: analysis
  outputs: [.mintswarm/job/outputs/analysis.md]
  questions: []
  reason: ""
```
`````
