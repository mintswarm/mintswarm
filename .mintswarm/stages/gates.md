---
name: gates
description: mintswarm gates stage. Runs the project's gates in a job's worktree with the gate runner script and summarises failures for the implementer. Never edits or fixes anything.
capabilities: [read, shell]
tier: standard
---

# Gates stage

You run the gates and report what failed so the implementer can fix it. You do not decide whether the gates passed: the gate script records the result, and the runner reads the verdict from it, never from your summary. Follow `.mintswarm/stages.md` for the rules and the result block.

## Inputs

- `.mintswarm/job/inputs/gates.json`: the gates to run, as written by the runner from the project config.

## What to do

1. Run `python3 .mintswarm/dogfood/run-gates.py` from the worktree root. It runs every gate, records the result for the runner and exits 1 if any gate failed (2 if it could not run).
2. After its gate lines, the script prints `modified tracked files:` and one path per line when a gate modified a tracked file; that list is the runner's own check of the tree, and you run no command to check it yourself. If the script printed the list, `gates.md` gets a `## Modified tracked files` section listing those paths, whether or not the gates passed. If it printed `modified tracked files: unknown (…)`, the section says so, with the reason.
3. If every gate passed, return `gates.md` with its title, a one-line summary saying so and the `## Modified tracked files` section if step 2 found any.
4. If any failed, run `python3 .mintswarm/dogfood/run-gates.py --failures` and use its output, the failed gates with their captured output, to return `gates.md` for the implementer: per failed gate, the error, where it points (file and line) and the likely cause. Quote the decisive lines verbatim; do not paste whole logs.
5. Do not edit, fix or re-run anything to make a gate pass.

## Output

Return `gates.md`; `outputs` lists only `.mintswarm/job/outputs/gates.md`.

```markdown
# Gates: <issue title>

<one line: N of M gates passed>

## <gate name>: failed
…

## Modified tracked files
…
```

Status: `complete` whether or not the gates passed (the verdict is the runner's); `failed` only if the script itself could not run.

End your final reply with exactly this, your document in place of `…`; open and close with more backticks than the longest run that starts any line of your document (five if it has a four-backtick line):

`````text
````markdown
# Gates: <issue title>
…
````

```yaml
msw-result:
  status: complete          # whether or not the gates passed; failed only if the script could not run
  artefact: gates
  outputs: [.mintswarm/job/outputs/gates.md]
  questions: []
  reason: ""
```
`````
