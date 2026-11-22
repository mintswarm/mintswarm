# Stage definitions

The harness-neutral source of every mintswarm stage agent: capabilities, model tier and instructions, one file per stage.

## What is here

- `<stage>.md` is the definition of one stage, the source of truth for its agent: `analyse`, `decompose`, `plan`, `implement`, `gates` and `review`.
- `.mintswarm/stages.md`, next to this directory, is different: it holds the rules every stage follows, and the definitions refer to it.
- `.claude/agents/msw-<stage>.md` is generated from these files by `.mintswarm/dogfood/render-agents.py`. Never edit it by hand.

## Format

A definition is Markdown with frontmatter:

```markdown
---
name: plan
description: mintswarm plan stage. Turns one implementation issue into ordered steps, …
capabilities: [read, search, shell, write-outputs]
tier: strong
---
<instructions>
```

The frontmatter is a subset of YAML:

- The file starts with a `---` line; the frontmatter ends at the next `---` line.
- Each line is `key: value`, split at the first `: `. The value is taken literally: no quoting, no multi-line values.
- A value in `[ ]` is a comma-separated list.
- Blank lines and lines starting with `#` are ignored.
- The keys are `name`, `description`, `capabilities` and `tier`, all required; no others are allowed. `name` equals the file name without `.md`.

The body is everything after the closing `---` line. It is the stage's instructions and is copied to every harness byte for byte.

## Capabilities

| Capability | Meaning | Claude tools |
| --- | --- | --- |
| `read` | read files in the worktree | `Read` |
| `search` | search file names and contents | `Grep`, `Glob` |
| `shell` | run shell commands | `Bash` |
| `write-outputs` | write the stage's output files under `.mintswarm/job/outputs/` | `Write` |
| `edit` | edit and create files in the repository | `Edit`, `Write` |

Several capabilities can give the same tool; the renderer lists each tool once, in the order `Read, Grep, Glob, Bash, Edit, Write`.

## Tiers

| Tier | Meaning | Claude model |
| --- | --- | --- |
| `strong` | the most capable model, for stages that reason over the issue and the code | `opus` |
| `standard` | a balanced model, for stages that mostly run tools and summarise | `sonnet` |
| `light` | a small, fast model, for mechanical stages | `haiku` |

## Limits

- `write-outputs` and `shell` are coarse. On Claude, `Write` can write anywhere and `Bash` can run anything. "Outputs only" and "read-only" are instructions in `.mintswarm/stages.md`, enforced by the runner's checks and the sandbox, not by the tool list.
- Other harnesses may express capabilities as sandbox modes rather than per-tool lists, so a capability says what a stage needs, not which tool it gets.
- Only Claude has a mapping. There is no Codex mapping yet.

## Model

`.mintswarm/config.yaml` `agents:` chooses the model at run time and wins over the tier. The tier is the stage's default, and `.claude/agents/` is rendered with it.

## Changing a stage

1. Edit `.mintswarm/stages/<stage>.md`.
2. Run `python3 .mintswarm/dogfood/render-agents.py`.
3. Commit both the source and the regenerated `.claude/agents/msw-<stage>.md`.

The `agents-drift` gate (`render-agents.py --check`) fails when the generated files differ from the sources, are missing or have no source.

## Target system

Repositories need no harness files. The runner's harness adapter builds each session from the stage template: the appended system prompt for Claude, the opening section of the prompt for Codex. `.claude/agents/` exists only while Claude Code is our runner.
