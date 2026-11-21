# mintswarm — Design Document

Nov 19, 2026 · @Sergei Fedorov

**Status:** draft after two review rounds. POC and MVP scope are set in Scope; the first work packages are in Handover; open questions are at the end.

## Overview and goals

mintswarm turns tickets into reviewed pull requests: a human shapes an idea into one or more tickets, runners carry it through analyse, design, plan and implement, an independent reviewer checks it, and the human approves the PR.

It is built first for a solo developer or small team running several projects on one AI subscription, and later as a multi-tenant service.

**Why mintswarm** *(draft)*

mintswarm is built for one situation that other factories treat as an afterthought: a developer or small team running many projects on their own machines and one AI subscription, who wants reviewed PRs without babysitting agents. No single feature below is unique; the combination is.

| What mintswarm does | Why it matters | Common elsewhere |
| --- | --- | --- |
| **Schedules against the subscription.** The account is a first-class budget shared by every project and runner; fair share, throttle-aware pausing, limit percentages per window | one plan, many projects, no surprise lock-outs | each project or run spends independently; limits discovered by failing |
| **Runs on a heterogeneous fleet you own.** Linux and macOS runners, capabilities found by extensible probes (Xcode, sanitisers, game engines, generation tools), work matched to them | real toolchains, including ones no cloud sandbox has | one machine, or Linux containers only |
| **Is harness-neutral end to end.** Claude Code and Codex behind one adapter contract: the same messages, events, usage and commands | switch or mix harnesses without changing the pipeline | built around a single agent CLI |
| **Reviews independently.** A fresh session, on the harness and model the project picks for review, sees ticket, acceptance criteria and diff, never the transcript; structured findings, bounded rounds | the writer never grades its own work | self-review in the same session, or none |
| **Never lets the model declare success.** Gates run by the runner outside the agent's shell; config read from the base branch; agents commit, the runner pushes | prompt injection or a confident agent cannot fake a green PR | gates run by the agent, agent holds push credentials |
| **Puts you in the loop from your own agent.** A console MCP for Claude, Codex or any MCP client, with push notifications, steer and interrupt for any running agent; the UI has the same functions | shape tickets, answer questions and redirect work from the session you already use | a web dashboard, or nothing while a run is in flight |
| **Shapes the ticket first.** Discussion becomes schema-checked tickets, linked by their dependencies, plus `discussion.md`, which every later stage can read | agents work from the why, not only the what | work starts from a raw issue |
| **Measures everything the same way.** Tokens, cost with its basis, limit windows and host metrics, per stage run, per harness | you see what each ticket cost and where machines are saturated | partial, harness-specific numbers |

The [survey of open-source factories](https://github.com/OscarDoh/varun1505-awesome-software-factories) behind this table covered 16 projects in depth; individual projects may already do one or two of these.

**Non-goals** *(draft)*

- Not an IDE or chat assistant: interactive coding stays in your editor and harness.
- Not a CI system: it runs gates before review, and your existing CI still runs on the PR.
- Not an issue tracker: GitHub Issues (and later Linear, Jira) remain the system people use.
- Not a general workflow engine: the pipeline is fixed; no workflow DSL.
- Not an agent compute provider, for now: runners are yours; metered hosted runners are deferred.

**Glossary**

| Term | Meaning |
| --- | --- |
| Control plane | the central service: state, scheduling, Control API, UI |
| Runner | a daemon on a machine, serving one project, that executes stage runs |
| Worker | one concurrent job inside a runner (`workers: N`) |
| Harness | the coding agent CLI a runner drives: Claude Code or Codex |
| Stage run (job) | one attempt at one pipeline stage, on one runner |
| Changeset | a work item's branch and the runner worktree that holds it; later stages of the item reuse it |
| Gate | a deterministic check (build, tests, linters) run by the runner, not the agent |
| Artefact | a file a stage produces: analysis, design, plan, handoff |
| `complete_stage` | the job MCP tool an agent calls to finish a stage; the only way a stage can succeed |
| Job MCP | the MCP server the runner gives each agent session |
| Console MCP | the MCP server your own session (Claude, Codex) uses to drive the factory |
| Account | a subscription or API key whose budget runners share |
| Ticket | a tracker issue that enters the factory; intake can produce several, related by the tracker's dependency links (depends on, blocks, blocked by) |
| Sub-item | a piece of one ticket's work, created by the plan stage as a tracker sub-issue; a ticket's sub-items can form a DAG |

**Design principles**

- **The factory owns state.** Trackers (GitHub Issues first) are a UI and notification surface; the control plane's state machine in Postgres is the source of truth.
- **The model never declares success.** A stage passes only through `complete_stage` plus deterministic gates run by the runner, outside the agent's shell.
- **The writer never grades its own work.** Review runs in a fresh session, on the harness and model the project sets for review (a different one where the user has access to it), and sees the ticket, acceptance criteria and diff, not the implementer's transcript.
- **Harness-neutral.** Claude Code and Codex sit behind adapters; messages, events, usage and commands never carry harness specifics.
- **Agents never touch GitHub.** Agents commit; the runner pushes with a short-lived per-job GitHub App token.
- **Warn, don't block.** Provider policy is the user's responsibility; mintswarm surfaces warnings and never forbids a credential mode.
- **One human's attention is the scarce resource.** Every stage writes artefacts a person can read, and the approval package explains what was and wasn't verified.

## Scope

| Milestone | Scope |
| --- | --- |
| POC | the control plane running locally, with runners connecting to it |
| MVP | a self-hosted control plane with authentication for people (UI, console MCP, CLI); runners authenticate with their keys from the POC on; language servers on runners for a limited set of languages |

Everything else (the commercial edition, the later features in the roadmap section) comes after the MVP.

**What each milestone contains** *(draft, to confirm)*

| Area | POC | MVP | Later |
| --- | --- | --- | --- |
| Harness adapters | Claude Code only (decided) | + Codex | others |
| Stages | all stages, review loop, plan approval when required, approval | unchanged | discovery pipelines |
| Decomposition | ticket dependencies honoured; no sub-items | sub-items (a DAG), one PR each | integration branch, stacked PRs |
| Runners | Linux and macOS, run by hand | signed services (systemd, launchd) | runner images, Windows |
| Sandboxing | worktree isolation only, trusted repos | bubblewrap / sandbox-exec + egress allowlist | srt, if adopted |
| Language servers | none | clangd, rust-analyzer, Python, TypeScript | index caches, more languages |
| Console MCP | inbox + `wait_for_events` | + push through Claude Code channels | other push channels |
| UI | none: CLI and console MCP | items, runners, approvals, usage | richer dashboards |
| Auth for people | none: human endpoints on loopback | token, OIDC, proxy | organisation SSO (commercial) |
| Tracker | GitHub Issues, polling | + webhooks when hosted | Linear, Jira |
| Distribution | built from source | signed runner binaries, control-plane image |  |

**Pipeline by milestone**: what each stage does at each milestone.

| Stage | POC | MVP | Later |
| --- | --- | --- | --- |
| Intake | your session drafts one or more tickets through the console MCP; `submit_tickets` creates labelled issues and their dependency links | + drafting and submitting from the UI | discovery pipelines propose tickets |
| Ingest | trusted label and ticket schema; polling; dependency links mirrored; blocked tickets wait | + webhooks | dedupe against open items |
| Analyse, design, plan | Claude Code, a fresh session per stage, artefacts stored; questions suspend and resume; plan approval when required | + plan splits tickets into sub-items | knowledge base |
| Implement | worktree per work item, agent commits, runner re-runs gates and pushes, first push opens a draft PR | + sandbox and egress allowlist, language servers | index caches for warm start |
| Gates | defined in repo config, run by the runner; `tests: none` forces plan approval | + gate tiers | test-sensitivity gate, hidden acceptance tests |
| Review | fresh Claude session, structured findings, bounded rounds | + Codex, harness and model per stage, fallbacks | review bricks, review from CI, CUJ QA |
| Address | resumes the implementer's session on the same runner | unchanged | unchanged |
| Approval | approval package, PR marked ready, you approve, the control plane merges | + approvals in the UI, outcome metrics | auto-landing |

Sections after Scope describe the full design. A paragraph whose bold lead carries **\[MVP\]** is not part of the POC; everything after the MVP is in the roadmap section.

## Architecture overview

Three clients drive one control plane, which owns all state in Postgres and hands stage runs to runners over a gRPC stream that runners open themselves.

&#91;embedded content: mintswarm architecture · clients, control plane, runners, GitHub\]

Runners claim work over their stream and the scheduler assigns it; the runner drives a harness through its adapter and pushes results to GitHub itself.

**Core entities** (what the control plane stores; the glossary above defines the terms)

| Entity | What it is |
| --- | --- |
| Tenant | identity boundary; commercial edition only; the self-hosted edition has no tenant concept |
| Project | a repo, its tracker binding and policies |
| Account | a subscription or API key; holds the shared usage budget |
| Runner | a registered daemon: capabilities, workers, account, key |
| Work item | a ticket's or sub-item's life in the factory, with dependency links mirrored from the tracker |
| Stage run | one stage attempt on one runner, with its session and usage; the runner calls it a job (job\_id, StageJob, per-job tokens) |
| Artefact | analysis, design, plan, discussion, handoff files |
| Finding | a structured review finding and its response |
| Event | an append-only, sequenced record of everything that happened |

## Work item lifecycle and pipeline

A work item moves through a fixed pipeline of stage runs; each stage is dispatched separately, so analyse, review and an iOS sub-item can land on different runners.

**Stages and their artefacts**

| Stage | Output | Notes |
| --- | --- | --- |
| Draft | one or more tickets + `discussion.md` | shaped with you in a root session through the console MCP; validated against the ticket schema (ready check); dependencies between the tickets become tracker links |
| Analyse | `analysis.md` | may ask questions → `needs_input` |
| Design | `design.md` | chosen approach, rejected alternatives |
| Plan | `plan.json` | steps, files, tests from acceptance criteria; may split the ticket into sub-items (a DAG) |
| Implement | commits on `mintswarm/<id>-<slug>` | agent commits; runner pushes |
| Gates | structured results | build, tests, linters, sanitisers, run by the runner |
| Review | findings | fresh session; harness and model from the project's stage settings |
| Address | fixed / disputed per finding | resumes the implementer session where possible |
| Approval | approval package | you approve, reject or comment; comments become another address round |

**Rules**

- Each stage runs in a fresh session fed only prior artefacts, except Address, which prefers resuming the implementer session.
- Gates run before review; the reviewer never sees broken code.
- Findings are structured (`severity, location, claim, evidence, suggested_fix`); the implementer answers every one with `fixed` or `disputed`.
- The review loop is bounded: up to 3 rounds by default (a per-project setting), only blocker or major findings continue it, then escalate to you with the disagreement written up.
- Two kinds of structure. **Ticket dependencies**: intake can produce several tickets, related through the tracker's own links (depends on, blocks, blocked by); the factory mirrors them, and a ticket with an unmerged blocker waits in `blocked` and is not dispatched, then runs against the default branch once every blocker has merged. **Sub-items**: the plan stage can break one ticket's work down into sub-items, created as tracker sub-issues; they can form a DAG through the same dependency links, each goes through the full pipeline with its own PR, and the parent waits. Default integration: one PR per ticket or sub-item to the repository's default branch, with dependent PRs opened in dependency order. Merge policy is set per project; a parent integration branch is opt-in.
- States: ` draft → ready [⇄ blocked] → analysing ⇄ needs_input →  `` designing →  ``planning → [awaiting_plan_approval →] implementing → verifying → reviewing ⇄ addressing → awaiting_approval → merged | rejected | escalated`, plus `paused` and `cancelled` for any running stage. `awaiting_plan_approval` is entered only when the project requires plan approval (`approve_plan: true`) or declares no tests; you approve, comment (back to planning) or reject. `blocked` holds an item while any of its blockers is unmerged. A ticket with sub-items waits in `awaiting_sub_items`; comments at approval send the item back to `addressing`.

**The approval package** lists how each acceptance criterion is met (with tests), design decisions, the review log with resolutions, operator messages and steers, what wasn't verified, and usage.

**Worked example**

1. In your Claude Code session you discuss an idea for slugkit; `submit_ticket` creates issue #482 with the `mintswarm` label and stores `discussion.md`.
2. Ingest sees the trusted label, validates the ticket schema and marks the item ready.
3. Runner A (Linux, Claude Code) has an idle worker and claims Analyse; the agent writes `analysis.md` and calls `complete_stage`. Design and Plan follow the same way; `plan.json` has four steps and tests derived from the acceptance criteria.
4. Runner A claims Implement, creates a worktree on `mintswarm/482-short-slug`, the agent commits and calls `complete_stage`. The runner re-runs the gates, pushes with a token minted for this job, and the first push opens draft PR #77.
5. The project sets Codex for review, so runner B (Codex) claims it. It submits two findings, one major.
6. Address prefers runner A, which resumes the implementer's session: the major finding is fixed, the minor one disputed with reasoning. The runner pushes again.
7. The second review round passes. The approval package is built, PR #77 is marked ready, and your session is notified.
8. You approve; the PR merges and the issue closes.

## Control plane

The control plane is a single Rust service over Postgres that owns all work-item state; it never runs agent work itself.

**Responsibilities**

- **State machine and queue** in Postgres: work items, stage runs, leases, artefacts, findings, event log, cost ledger. Runners pull work: an idle worker sends a claim on the stream, and the control plane answers with an assignment and a 60-second lease renewed by heartbeat (every 10 seconds). There are no offers to accept or decline; a claim with no eligible work simply waits until work arrives.
- **Tracker adapters** (GitHub Issues first; Linear and Jira later): `fetch_ready`, `post_comment`, `set_status`, `create_sub_item`, `link_pr`. Plus `read_links` and `link_dependency` for dependency links and sub-issues, which every target tracker has natively (GitHub issue dependencies and sub-issues, Linear blocking relations and sub-issues, Jira issue links and subtasks). Polling with ETags in the POC; HMAC-verified webhooks from the MVP, once GitHub can reach the control plane.
- **Ingest and ready gate**: a ticket enters only with a trusted signal (a maintainer-only label or an allowlisted author) and a valid schema. Tickets drafted through the console MCP take the same path: submit\_ticket creates a labelled issue through the tracker adapter, so every ticket passes one gate. Issue bodies are untrusted input.
- **Scheduler**: matches stage requirements to runner capabilities (`os`, toolchains, harness); budgets against each account's shared subscription with weighted fair share across projects (weights are a per-project setting in the control plane, equal by default); pauses all runners on an account when it is throttled. It never dispatches a blocked item. Placement preferences use delay scheduling: a stage run can prefer a runner (every later stage of a changeset prefers the runner holding its worktree and session). Harness and model are requirements, not preferences: each stage's come from the project's settings. Until a deadline passes (5 minutes by default, set per project), claims from other runners skip it; an offline or throttled preferred runner ends the wait at once.
- **Reconciliation** every tick: running stage runs are checked against tracker state and dependency links are re-read; an issue closed or stripped of its trusted label cancels its runs.
- **Runner availability**: runners report state, capacity (free and total workers) and load (CPU, memory, active jobs) on every heartbeat; the control plane schedules on that and marks a runner offline after 3 missed heartbeats, letting its leases expire for reassignment. Detecting a stalled harness is the runner's job (see Runner internals); the control plane only sees the outcome.

**Tenancy.** Tenancy is an interface (`Tenancy`) with two implementations, and core code depends only on the interface. The self-hosted implementation is single-tenant: its schema and queries have no notion of a tenant. The commercial implementation, in the private repo, brings its own schema with `tenant_id` and tenant-scoped queries.

**Configuration.** `mintswarm.yaml` mounted at `/etc/mintswarm/mintswarm.yaml` (or `--config`); environment variables override keys (`MINTSWARM_DATABASE__URL`); secrets also accept a `*_FILE` variant. Migrations run at start under an advisory lock, or separately via `mintswarm migrate`. Health endpoints: `/healthz`, `/readyz`.

## Runners: bootstrap and registration

A runner is a Rust daemon started in a project's checkout under an OS user that holds the harness logins; it dials out to the control plane, so it needs no inbound route. A runner serves exactly one project with a local cap of N workers; several projects on one machine means one daemon per project, all sharing the same OS user, harness login and account budget. That is the headline setup: one developer, several projects, one subscription.

**Bootstrap**

1. Create an OS user `mintswarm`; run `claude` / `codex login` once so credentials persist in that user's home.
2. Clone the repo; this checkout becomes the runner's base clone.
3. On the control plane, from the UI (Project → Runners → Add runner, which shows the connection string and a ready-to-copy join command), the console MCP, or the CLI: `mintswarm runner-token create --project slugkit --ttl 24h [--max-uses 5] [--tags os=linux]` prints a connection string `msw1_…` = base64url of `{url, token, ca_fingerprint}`.
4. On the machine: `mintswarm-runner join msw1_… [--tag gpu] [--workers 2] [--account ultra-main]`, then `install-service` (systemd user unit with linger on Linux, launchd agent on macOS).

**Registration token.** Multi-use, short-lived, project-scoped, optional default tags and `max_uses`, stored hashed, revocable. Registration fails unless the clone's `git remote` matches the project's repo. The CA fingerprint pins the control plane's certificate on first contact, so TLS works on `*.local` without a public certificate.

**Identity: a key pair, no refresh tokens.** At `join` the runner generates an Ed25519 key pair and sends only the public key. On each start it signs an assertion (`aud`, `iat`, `jti`) and receives a 30–60 minute access token carrying `runner_id` and `project`, plus `tenant` in the commercial edition (OAuth `private_key_jwt`, RFC 7523). Revoking a runner deletes its public key.

**Live connection: one gRPC bidirectional stream.**

- Runner → plane: `claim`, `start_failed`, `hello`, ` heartbeat {state,  ``workers free/total, load, throttled_until, active_jobs}`, `job_event {job_id, seq, …}`, artefact uploads, `push_token_request {job_id, commit}`.
- Plane → runner: `assigned`, `cancel`, `interrupt`, `deliver_message`, `drain`, `upgrade_required`, `push_token {job_id, token, expires_at}` (an installation token for one push). `assigned` carries the StageJob and, for an `api_key` account, that job's key.
- Job events carry sequence numbers; after a reconnect the runner replays from the last acknowledged one. Keepalives on both sides.

**What a runner reports** in `hello`, at registration and on every start: runner and protocol version, host, OS, arch, harnesses (version, logged in, credential mode), detected toolchains, free-form tags, account label, workers, repo remote and head. The plane keeps a history of capability changes.

**Capability probes are extensible.** Detection is data, not code: each capability is a probe definition in YAML, and the runner ships only a probe engine plus a built-in set.

```yaml
# probes/unreal.yaml
id: engine.unreal
kind: engine            # toolchain | engine | tool | service
detect:
  - path: "/Users/Shared/Epic Games/UE_*/Engine/Binaries"
  - command: ["UnrealEditor-Cmd", "-version"]
    version: 'Unreal Engine (\d+\.\d+)'
timeout: 10s
```

- **Built-in probes** at launch: C++ (clang, gcc, cmake, ninja), Rust, Python, Node, Flutter/Dart, Xcode (version, SDKs, simulator runtimes).
- **Detection methods:** a command with a version pattern, a path or glob, an environment variable, or a script that prints JSON for anything harder (engines installed through launchers, licensed tools, access to image, video or audio generation models).
- **Where probes come from:** built in, then `~/.config/mintswarm/probes/` on the machine, then `.mintswarm/probes/` in the repo; a later file with the same id overrides.
- **Namespaced results** such as `toolchain.cpp.clang = 19.1`, `engine.unreal = 5.5`, `tool.imagegen.flux = available`; stage requirements match them with version ranges.
- Probes run at start, on demand from the UI, and on a slow timer; each has a timeout, and a failing probe reports `unknown` rather than blocking the runner.
- Later additions (C#, Unity, Unreal Engine, CryEngine, other engines, generation models) are new probe files, not runner releases.

**Lifecycle:** `online → draining → offline` (3 missed heartbeats at a 10 s interval), `revoked`; stale runners are removed after 14 days; an unsupported protocol gets `upgrade_required`.

**State on disk**, outside the repo: `~/.local/state/mintswarm/<id>/` holding `key.ed25519` (0600), `runner.yaml` and the pinned CA; never mounted into the agent sandbox.

## Runner internals

Each assignment becomes one stage run (a job, to the runner): one harness process in its changeset's worktree and, from the MVP, its own sandbox, talking to the factory only through a job-scoped MCP server.

**Components:** PlaneClient (gRPC stream), WorkerPool, one JobSupervisor per active run (Workspace, HarnessAdapter, Sandbox, GateRunner), JobMcpHub (local socket), Pusher.

**The job spec** (`StageJob`): job and work-item ids, stage, attempt, base ref, branch, requested harness and model, budget, input references (ticket snapshot, prior artefacts, findings), rendered prompts with template version, allowed MCP tools, and an optional session to resume.

**Worktrees.** The checkout the runner starts in is the base clone and stays clean. A worktree belongs to a changeset (a work item's branch), not to a stage run. The first stage that needs one runs `git worktree add ../<repo>.wt/<branch>`; later stages of the same changeset on that runner (Implement, Gates, Review, each Address round, retries) reuse it, and Address resumes the implementer's harness session there. Stages of one changeset run one after another, so a worktree serves one job at a time; `workers` = concurrent jobs, each in a different changeset's worktree. A completed stage leaves its worktree clean: the agent has committed everything and the runner has pushed it, and the stage counts as done only once that push succeeds (see Failure and recovery). The runner checks the tree when the agent calls complete\_stage. Uncommitted changes at that point mean a bug in mintswarm (an adapter, a hook, the agent instructions), not something to absorb quietly: the runner raises a warning event on the stage run with the changed files and keeps the uncommitted diff as an artefact, but does not commit it; the stage completes on what was committed. Failed and cancelled stages may leave leftovers; those are discarded without a warning. Before the next stage starts, the runner resets the worktree to the pushed head whatever its state. The scheduler prefers the runner that already holds a changeset's worktree. Worktrees are removed when the work item reaches a terminal state, when idle past a TTL (24 hours by default), or least recently used first under disk pressure; a later stage recreates one from the pushed branch.

**Launching a session.** The adapter builds the command line (Claude: headless print mode with streaming JSON in and out, appended stage instructions, a strict per-job MCP config, per-job settings with hooks, allowed tools, max turns; Codex: `exec --json` or `app-server`, sandboxed workspace writes, per-job `-c` overrides). The supervisor runs it in its own process group, parses the event stream into `job_event`s, enforces wall-clock and budget limits, and detects rate limits.

**Exit code is not success.** A stage succeeds only when the agent called `complete_stage` and validation passed. Each run records three results separately: agent result, verification result, factory result.

**Stall detection** is the runner's responsibility. No harness event for 10 minutes is only a suspicion; the runner then checks other liveness signals: the harness's process tree (a live child such as a test runner or compiler whose CPU time or I/O is still advancing), output still growing in the job's logs, or a declared long step (`report_progress` with an expected duration, or a gate the runner itself is running). It kills and retries only when every signal stays quiet past a per-stage grace period (20 minutes by default); the stage's hard wall-clock limit still applies. Outcomes are reported as events, and end states stay distinct: succeeded, failed, timed out, stalled, cancelled.

**Waiting for a human.** When a stage asks a question (`ask_human`), the runner saves the session id and branch, stops the harness and frees the worker; no process stays alive while the item sits in `needs_input`. The answer arrives as a new stage run that resumes the session, preferring the same runner. Resuming after an answer is not a retry: it does not count towards the 3-attempt limit in Failure and recovery.

**Instruction layers**

| Layer | Owner | Mechanism |
| --- | --- | --- |
| Repo conventions | the repo | `CLAUDE.md` / `AGENTS.md`, read natively |
| Stage behaviour | control-plane templates, repo overrides | appended system prompt (Claude); opening section of the prompt (Codex) |
| Task material | the work item | files in `.mintswarm/job/inputs/`, referenced by a short prompt |

Ticket text and findings are data, not instructions; stage instructions say so.

**Job MCP tools** (served by the runner over a local socket via a stdio shim; the server enforces each stage's tool list):

| Group | Tools | Stages |
| --- | --- | --- |
| Context | `get_ticket`, `get_artefact`, `get_related_items`, `get_findings` | all |
| Artefacts | `put_artefact` (schema-validated) | analyse, design, plan |
| Control | `ask_human`, `propose_split`, `complete_stage`, `fail_stage` | per stage |
| Gates | `run_gates` (executed by the runner) | implement, address |
| Review | `submit_finding` / `respond_to_finding` | review / address |
| Progress | `report_progress`, `complete_step`, `notify` | all |

Every job MCP result also carries `pending_messages`, so operator notes reach any harness.

**Pushing.** After `complete_stage` the runner checks the branch (no commits outside it, no workflow changes unless the project's policy allows), re-runs the gates authoritatively, and pushes with that job's short-lived GitHub App token, which the control plane mints for that repository (contents and pull requests write) and sends with the push request, never earlier. Implement and every Address round push the branch; the first push opens a draft PR, so reviewers and you can follow it, and the PR is marked ready for review when the approval package is complete. Pushes are idempotent: the runner pushes a known commit to its own branch, and a retry after a crash simply repeats it. No GitHub credential enters the sandbox.

**Sandboxing. \[MVP\]** bubblewrap on Linux and `sandbox-exec` on macOS, with the worktree writable, the runner's state directory hidden, and network restricted. [Anthropic's sandbox-runtime](https://github.com/anthropics/sandbox-runtime) (`srt`) is a candidate to replace a hand-written layer.

**Egress allowlist. \[MVP\]** Built from three sources: the harness adapter's defaults (its provider's API hosts), package registries for the toolchains the job needs (crates.io, PyPI, npm, pub.dev), and the project's own entries under `sandbox.egress` in the repository config. Anything else is blocked and logged as an event.

**Language servers. \[MVP\]** Part of the MVP, for a limited set of languages to start (C++ with clangd, Rust with rust-analyzer, Python, TypeScript). Agents reach them through the harness's own LSP support, not through tools on the job MCP: in practice agents tend to ignore custom MCP tools for browsing and editing code. A language server definition is declarative, in three parts: a probe (is the server installed, which version), a start script (command, arguments, prerequisites such as generating `compile_commands.json`), and, per harness adapter, how to tell the harness about the server (an entry written into the job's harness config). A harness without LSP support runs without it. mintswarm ships definitions, and repos add or override them under `.mintswarm/lsp/`. Servers are started by the harness session itself, through its own LSP support, rooted in that session's worktree, and stop with the session; mintswarm only writes the definitions into the job's harness config. No server outlives a session or is shared between sessions, so none can point at another source tree. Servers are part of the job's process tree, so their CPU and memory show in the per-job host metrics. Because a work item keeps one worktree across its stages, a server's on-disk index (for example clangd's background index under .cache/clangd) stays in that tree: later stages warm-start against the same sources and reindex only what changed. These cache directories are excluded from git and survive the reset between stages. Warm-starting a fresh worktree from an index built elsewhere (the base clone or another worktree) is welcome but needs research first; it is listed under open questions.

## Harness adapters and credential policy

Harnesses are drivers: each adapter declares its capabilities, and the runner falls back to kill-and-resume, which every harness that can resume a session supports.

**Harness and model per stage.** The project sets a default harness and model in its repository config, and any stage can override either (review on Codex while everything else runs on Claude, a cheaper model for analyse). Not every user has more than one harness, so nothing assumes it: with no overrides every stage, review included, runs on the default, and review's independence comes from the fresh session that never sees the implementer's transcript. The choice is a requirement in the job spec: the scheduler places a stage only on runners that have that harness logged in, and among those prefers the one holding the work item's worktree.

```yaml
agents:
  default: { harness: claude, model: opus }
  stages:
    analyse: { model: sonnet }
    review:  { harness: codex }
```

Models are requirements too. Runners report the models each harness can use where the harness exposes them (a probe), otherwise `unknown`, which the scheduler treats as eligible. If the provider rejects the model at run time, the stage fails as `model_unavailable` without a retry, the runner records that model as unavailable for its account, and the item escalates to you.

**Fallback. \[MVP\]** A project can list fallbacks, for all stages or per stage, tried in order only when the primary account is throttled or the model is unavailable, never to chase quality. The fallback used is recorded on the stage run and shown in the approval package. Fallbacks are optional: with none configured, a throttled stage waits for its account to recover.

```yaml
agents:
  default:  { harness: claude, model: opus }
  fallback: [{ harness: codex }]
  stages:
    review: { harness: codex, fallback: [{ harness: claude, model: sonnet }] }
```

| Capability | Claude Code | Codex | Universal fallback |
| --- | --- | --- | --- |
| Inject a message mid-session | streaming JSON input on stdin | `app-server` turn input | deliver at the next job MCP call or hook |
| Interrupt the current turn | interrupt control request | `app-server` turn interrupt | kill the process group |
| Resume a session | `--resume <id>` | `exec resume` / thread resume | fresh session from artefacts |
| Rate-limit signal | rate-limit events in the stream | rate-limit fields in token-count events | none |

The control messages in this table are not yet verified: the harness experiments work package pins versions and confirms each one before the adapter is built.

**Credential modes**, set per account: `subscription` (the login already on the machine) or `api_key` (injected by the runner into that one child process). Users pick freely; mintswarm never blocks either. The runner clears stray API-key variables from the child environment unless the account is set to `api_key`.

**Where API keys live. \[MVP\]** An `api_key` account's key is stored in the control plane's `SecretStore` (encrypted in Postgres by default). It is sent with each assignment that needs it, set only in that harness process's environment, and never written to the runner's disk. An operator who prefers to keep keys off the control plane can instead point the account at a key file on the runner machine.

**Policy warnings \[MVP\]** are advisory, acknowledgeable, and re-raised when the underlying facts change:

| Rule | Trigger |
| --- | --- |
| `public-repo-subscription` | public repo with an account in subscription mode; [Codex docs](https://developers.openai.com/codex/auth) recommend API keys for automation on public repos |
| `multi-user-subscription` | tickets from authors other than the project owner, other users with push rights, or several humans with access to the project |
| `sustained-load` | concurrency or hours well above typical individual use over a rolling window |
| `unexpected-api-key` | a runner reports an active API key while its account is in subscription mode |

Warnings appear at `join`, at project setup, through the console MCP (`get_warnings`) and in the approval package while unacknowledged; never in tracker comments.

**Documentation** carries a "Credentials and usage policy" page with links to each provider's terms and a "last checked" date, including the recommendation to use an API key for Codex on public repositories.

## Context and budget control

Context size is a neutral policy in `.mintswarm/config.yaml`, translated by each adapter and backed by a runner-side safeguard, with handoff to a fresh session as the default when a context fills.

```yaml
context:
  window: auto            # auto | 200k | 1m
  compact_at_pct: 70
  on_full: handoff        # handoff | compact
  keep: prompts/keep.md   # what any summary must keep
  tool_output_limit: 12k
budget:
  max_turns: 200
  max_tokens: 3M          # per stage run, input + output
  soft_pct: 80
```

**Mapping**

- Codex: per-job `-c` overrides for `model_context_window`, `model_auto_compact_token_limit` (Codex caps it at 90% of the window), `tool_output_token_limit` and `compact_prompt`.
- Claude Code: `CLAUDE_CODE_AUTO_COMPACT_WINDOW` and `CLAUDE_AUTOCOMPACT_PCT_OVERRIDE` set in the child process environment (settings-file env blocks have proven unreliable), plus a `PreCompact` hook that injects `keep.md`.

**Runner safeguard.** The input tokens of each turn approximate context size. The runner emits `context_pct` and every compaction (tokens before and after) as events, and applies `on_full` itself if fill passes the threshold and the harness does not act.

**Handoff over compaction.** The runner steers the agent to write `.mintswarm/job/handoff.md` and mark finished plan steps, ends the session, and starts a fresh one on the same stage run from artefacts plus the handoff. `compact` stays available for exploratory stages.

**Budgets.** `max_turns` via `--max-turns` for Claude, counted from events for Codex. `max_tokens` is enforced from usage for every harness: at `soft_pct` the agent is told to wrap up; at 100% the run is interrupted, marked `budget_exceeded` and escalated.

`max_tokens` counts input, output and cache-write tokens. Cache reads grow with every turn of a long session (a 200-turn run re-reads its context each turn), so they are tracked separately, with an optional cap of their own (`max_cache_read_tokens`).

**Feedback.** Repeated handoffs on one run mean the task was too big for a session; this is shown on the run and later fed back to the plan stage as a signal to split.

## Control API, UI and console MCP

The factory UI, the console MCP and the CLI are three clients of one Control API (gRPC + HTTP/SSE); every UI action is an API call the console MCP also exposes, and the UI only adds richer presentation.

**Console MCP** runs as `mintswarm mcp console`, a local stdio process started by any MCP-capable root session (Claude Code, Codex, others). In the POC it talks to the control plane on loopback without auth; from the MVP it authenticates with your token, so it also works against a remote control plane.

| Group | Tools |
| --- | --- |
| Intake | `draft_ticket`, `validate_ticket`, `submit_ticket`, `submit_tickets`, `search_items` |
| Observe | `list_items`, `get_item`, `get_stage_run`, `tail_events`, `get_review_log`, `get_budget`, `get_warnings` |
| Control | `approve`, `reject`, `interrupt`, `steer`, `resume`, `cancel`, `retry`, `set_priority` |
| Talk | `list_threads`, `send_message`, `read_messages`, `answer_question`, `wait_for_events` |

A work item starts as a `draft` (or several drafts, when the discussion splits the idea) during discussion; `submit_ticket` writes `discussion.md` as its first artefact. `submit_tickets` submits a set with its dependency edges: it creates every issue, then the links, and each ticket gets the shared `discussion.md`.

**Events.** One append-only log, sequenced per work item, fanned out with Postgres `LISTEN/NOTIFY`. Two sources: the runner and harness automatically (stage changes, turns, tool calls, tokens, gates, rate limits, stalls, context fill) and the agent through the job MCP (`report_progress`, `complete_step` against `plan.json`, `notify`, `ask_human`).

**Attached sessions** register with subscriptions (project, item, event types). Delivery depends on the client: push where supported (Claude Code channels, a research preview needing `--channels`), an `inbox` summary piggybacked on every console tool result (any client), and long-poll via `wait_for_events`. When an agent asks a question, the first answer from any session or the UI wins and is recorded with who answered.

**Messaging.** One thread per work item; authors are you, `agent:<stage_run>` or `system`. Agent to you: `notify` (keeps working) or `question` (suspends the stage).

| Command | Effect |
| --- | --- |
| `steer(run, msg)` | inject at the next boundary, no interruption |
| `interrupt(run)` | stop the turn, pause, keep the session |
| `interrupt(run, msg)` | stop the turn, resume immediately with the message |
| `resume(run, msg?)` | paused → running |
| `cancel(run)` | end the run, discard the session |

**Rules.** Operator messages are trusted instructions; ticket and tracker text stays data, and operator messages never arrive via tracker comments. Every command is logged with who sent it and from which client, and appears in the approval package. Messaging the reviewer is allowed but flagged in its report. Steers count against budget.

## Usage and host metrics

Every stage run reports usage in one harness-neutral shape, aggregated per work item, project and account; the account totals drive the fair-share scheduler.

```yaml
usage:
  tokens: { input: 182340, output: 24110, cache_read: 1203000, cache_write: 88000, reasoning: 0 }
  cost:   { usd: 4.12, basis: harness_reported }   # harness_reported | computed | unknown
  limits:
    - { window: 5h, used_pct: 63, resets_at: 2026-11-19T18:00Z }
    - { window: 7d, used_pct: 22, resets_at: 2026-11-24T09:00Z }
  source: claude_stream_json
  observed_at: 2026-11-19T14:58:12Z
```

- Tokens are always available from both harnesses' streams.
- Cost always carries its basis: on a subscription a harness-reported figure is notional (API-equivalent); API-key runs are computed from a price table in the control plane; `unknown` is never shown as zero.
- Limit percentages are filled only from the harness's own stream. Runners never read credential stores to query usage endpoints.
- `observed_at` governs trust: the UI greys stale figures, and the scheduler uses only recent limits.

**Host metrics. \[MVP\]** Alongside usage, each runner reports machine metrics on a fixed interval (default 30 s) and in every heartbeat summary:

```yaml
metrics:
  cpu:    { used_pct: 72, load_1m: 9.4, cores: 16 }
  memory: { used_bytes: 41.2e9, total_bytes: 64e9 }
  disk:
    - { mount: /home, used_bytes: 812e9, total_bytes: 1e12 }   # base clone + worktrees
  per_job:
    - { job_id: job-482, cpu_pct: 380, rss_bytes: 6.1e9 }
  observed_at: 2026-11-19T17:01:30Z
```

- Collected by the runner for the host and per job (the job's process tree), with the same code on Linux and macOS (the `sysinfo` crate).
- Disk covers the volumes holding the base clone, worktrees and runner state; low free space is a runner warning, and below a hard floor the runner stops claiming work.
- The control plane keeps recent samples at full resolution and downsamples older ones; the UI shows them per runner and per stage run, next to token and cost usage.
- The scheduler may use load as a soft signal (for example, prefer the less loaded of two eligible runners), while the runner's own `workers` cap stays the hard limit.

**Outcome metrics. \[MVP\]** Usage says what work cost; outcome metrics say what it bought. The control plane computes them from the event log and cost ledger, per project and per account:

- cost and wall-clock time per merged PR, from ready to merged;
- review rounds per item and the escalation rate;
- the autonomy ratio: the share of merged items that needed no steer, no answered question and no address round triggered by your comments;
- later, code shelf life: how long merged lines survive before they are reverted or rewritten, from git history.

The readiness check is the leading signal; these are the lagging ones that show whether the factory pays off.

## Repository config

Each repo has one config path, `.mintswarm/config.yaml`; the `.mintswarm/` directory holds everything the repo owns for mintswarm plus a never-committed job scratch area.

```
.mintswarm/
├── config.yaml   # committed: gates, hooks, labels, protected paths, context policy, budgets, agents, sandbox egress, stage overrides
├── prompts/      # committed: optional stage prompt overrides, keep.md
├── probes/       # committed: capability probe definitions
├── lsp/          # committed: language server definitions
├── hooks/        # committed: after_create, before_run, after_run, before_remove
└── job/          # never committed: per-job scratch, listed in .git/info/exclude
```

- YAML everywhere (control plane, runner, repo), parsed with YAML 1.2 semantics; unknown keys are rejected.
- A JSON Schema generated from the Rust types is published on mintswarm.dev for editor autocompletion.
- Workspace hooks run with timeouts; templates render strictly, so an unknown variable fails.
- **The base ref decides, not the job branch.** Config, prompts, hooks, gates and probes are read from the base ref (the default branch when the stage was dispatched), never from the branch an agent writes to. An agent cannot change its own rules; changes take effect only after a human merges them. A PR that adds or changes gates is therefore checked with the old gates. With an opt-in integration branch, config is still read from the default branch, not from the integration branch. **Workspace hooks** (the repo's `hooks/`) run in a sandbox with the job profile from the MVP (in the POC, which runs only on repos you trust, they run as the runner's user): worktree writable, runner state hidden, egress allowlist. For hooks that fire around worktree creation and removal, when no job is running, the runner creates that sandbox for the hook itself. **Harness hooks** are a different thing: the runner installs them into each session's harness config (PreCompact, guardrails against force-pushes and protected-file edits), and they are never read from the repo.
- **Gates are required.** A project defines at least one gate, or its implement stages fail as MISCONFIGURED. A repo without tests says so explicitly (`gates: { tests: none }`): that forces your approval of the plan (the item waits in awaiting\_plan\_approval) and marks the approval package "no automated tests".

## Product shape

mintswarm ships as a fair-source self-hosted edition and a commercial multi-tenant SaaS, built from one codebase with tenancy behind an interface.

|  | Fair source (self-hosted) | Commercial (mintswarm.app) |
| --- | --- | --- |
| Repository | public, `mintswarm` GitHub org | private, depends on the public crates |
| Tenancy | single-tenant `Tenancy` implementation | multi-tenant implementation, one shared database |
| Control plane | self-hosted: Docker image | hosted by us (Fly.io) |
| Runners | self-hosted; subscription or API key | self-hosted; subscription or BYO API key |
| Auth | pluggable `AuthProvider`: token, OIDC, proxy | same trait, plus organisation SSO |
| Deliverables | source, build system, signed runner binaries, control-plane image | service |

**Licence.** FSL-1.1-ALv2: free use, modification and self-hosting, commercial use included, except offering a competing product or service. Each release converts to Apache-2.0 after two years. Contributions require a CLA so they can flow into the commercial edition. Describe the project as "fair source", not "open source".

**Extension points** (traits in the public repo, richer implementations in the private one): `Tenancy`, `AuthProvider`, `SecretStore`, `UsageSink`; later `CredentialBroker` and `Provisioner`.

## Deployment and distribution

mintswarm starts local-first and becomes hostable without code changes; runners ship as signed native services on Linux and macOS.

**Deployment path**

1. **POC, local:** control plane on your machine, reached by `*.local` on the LAN; GitHub polling with ETags; runners authenticate with their keys. In the POC the human-facing endpoints (UI, and the Control API used by the console MCP and CLI) bind to loopback and run without auth, while the runners' gRPC listener is on the LAN and authenticates runners by their keys. The MVP adds auth providers for people, starting with the token provider (a control-plane API token, which authenticates you to the control plane, not to GitHub), so the human endpoints can listen beyond loopback; a control plane told to expose them beyond loopback without an auth provider refuses to start. GitHub access comes from a GitHub App that the operator creates once during setup through GitHub's manifest flow (one click from the setup wizard); the control plane stores the App's private key and mints per-job installation tokens, in local and hosted mode alike. The App asks for: Contents read and write (push branches), Pull requests read and write (open, update, mark ready), Issues read and write (labels, comments, child issues), Checks and Commit statuses read (CI results for the approval package) and Metadata read. It does not ask for Workflows, so GitHub itself rejects a push that touches workflow files; a project whose policy allows such changes grants that permission explicitly. With webhooks it subscribes to issues, issue comments, pull requests, pull request reviews and check runs. The console is the console MCP running in your own Claude Code or Codex session on that machine. To follow work from a phone or another computer you reach that session through the harness's own remote access (for example Claude Code's remote control), so the control plane's human endpoints never need to leave loopback.
2. **MVP, self-hosted anywhere:** the same code on whatever host the operator chooses (a container platform, a VM, Kubernetes). Our first deployment will be on Fly.io, but nothing depends on it. **A hosted control plane requires authentication:** it refuses to listen on a non-loopback address without an auth provider configured. Pluggable auth (`token`, `oidc` for GitHub, Google or any issuer, `proxy` trusting an identity header only from configured proxy addresses), public TLS and HMAC-verified webhooks. Where the platform offers workload identity (for example Fly's machine OIDC tokens), it can authenticate the control plane to a KMS or Vault behind `SecretStore`, and later identify runners.

**Control plane components and images.** A deployed control plane is nginx in front of the Rust core, plus a Node BFF in commercial installations:

| Component | Role |
| --- | --- |
| nginx (edge) | external TLS termination, serves the UI's static files, routes `/api` and SSE to core (to the BFF in commercial installations), proxies runners' gRPC streams (`grpc_pass`, HTTP/2, long read timeouts) |
| Node BFF (commercial only) | Commercial installations only, so aggregating endpoints can change without rebuilding the core binary. UI convenience: aggregates and shapes Control API calls for the UI; calls core with the user's own token, so the UI never gets more than the console MCP |
| Core (Rust) | Control API, scheduler, ingest, event log, and the UI-facing endpoints in the self-hosted edition; enforces auth itself even behind nginx |

The components ship in one image, built for amd64 and arm64; core takes a mounted config, and Postgres can live anywhere. A convenience `docker-compose.yaml` starts that image plus Postgres. Inside the container a small process supervisor (for example s6-overlay) starts nginx and core (plus the BFF in the commercial image), restarts them, and forwards signals. Runner images are deferred. The UI speaks one contract, the UI API: core implements it in the self-hosted edition, and the BFF implements the same contract plus commercial-only endpoints. The UI learns which optional features exist from a capabilities document, so it has one code path.

**Release pipeline. \[MVP\]** cargo-dist builds archives and installers. Targets: runner on Linux x86\_64/aarch64 and macOS arm64/x86\_64; control plane as the image only (amd64 and arm64; on a Mac it runs under a container runtime). There is no standalone control-plane binary release: developers run core from source with the compose file.

| Platform | Signing | Service |
| --- | --- | --- |
| macOS | Developer ID + notarisation (hardened runtime), via `rcodesign` from Linux CI; `.pkg` installer | launchd |
| Linux | signed checksums (cosign or minisign) | systemd user unit with linger |
| Windows | deferred | deferred |

Signing keys live only in a protected release environment; forks and source builds are unsigned.

## Security model

Every boundary assumes the agent may be prompt-injected: the sandbox, the scoped job MCP and the runner-owned push are what hold, not instructions. The POC has no sandbox, so it is meant only for repos and tickets you trust.

- **Untrusted input:** ticket and issue text, review findings, fetched content. Gated at ingest by a trusted label or author; marked as data in stage instructions.
- **Trusted input:** authenticated operator messages through the control plane only.
- **Credentials an agent can reach:** the harness login (unavoidable). Out of reach: the runner key, GitHub tokens, the control plane's secrets.
- **Credentials never in the sandbox:** GitHub tokens (runner pushes after validation), the runner's state directory.
- **Harness hooks** (blocking merge, force-push, edits to protected files) are a guardrail, not a security boundary; branch protection is the real limit.
- **Network:** egress restricted in the sandbox; the runner dials out only; webhooks expose a single verified path.
- **Audit:** every command, steer, answer and warning acknowledgement is in the event log with actor and client.

## Failure and recovery

Postgres holds all durable state, branches on GitHub hold all code, and every stage run can be repeated from those two, so recovery is mostly retry.

| Failure | Behaviour |
| --- | --- |
| Control plane restarts mid-stage | Runners keep working, reconnect with backoff and replay job events from the last acknowledged sequence number. Leases of runners that reconnect within 2 minutes of the restart are kept. |
| Runner dies mid-stage | Its lease expires after 60 s; the stage run is marked lost and retried, preferably elsewhere, from the last pushed branch and the stored artefacts. After 3 attempts it escalates to you. |
| Runner dies between commit and push | On restart the runner finds completed but unpushed jobs; if the lease is still valid it re-runs the gates and pushes, otherwise it resets the worktree to the last pushed commit and the retry starts there. |
| Duplicate execution | Every assignment and completion carries the stage run id and attempt; the control plane accepts results only from the current lease holder. |
| Push repeated | Each item has its own branch; the runner pushes a known commit with `--force-with-lease` against the expected remote head, and PR creation reuses an existing PR for the branch. |
| GitHub unavailable | Pushes and tracker sync retry with backoff; finished stages wait for the push before they count as done. |

**Backups and retention.** Back up Postgres (pg\_dump or point-in-time recovery); artefacts live there too for now. Worktrees are disposable. Events are kept at full detail for 90 days, then summarised per stage run.

## Handover: first work packages

This document is the output of intake for mintswarm itself. Development starts with the packages below; each becomes one or more tickets, linked by the dependencies in the last column.

| Package | What it delivers | Kind | Depends on |
| --- | --- | --- | --- |
| 1. Engineering scaffold | Cargo workspace (core, runner, proto, CLI, console MCP), pinned toolchain, CI running format, lint and test gates, compose file with Postgres, `CLAUDE.md` / `AGENTS.md` with coding conventions, a folder for decision records | development | none |
| 2. Contracts | gRPC `.proto` for the runner stream and the Control API, with `google.api.http` annotations for the browser-facing RPCs; the first Postgres migration; the state transition table (states, events, guards); JSON schemas for the ticket, `StageJob`, job MCP tools, artefacts (`plan.json`, findings) and the POC subset of `config.yaml` | development | 1; informed by 3 |
| 3. Harness experiments | Claude Code headless: streaming JSON in and out, mid-session inject, interrupt, resume, usage and rate-limit events, compaction settings and the PreCompact hook, MCP and LSP config. GitHub: App manifest flow, installation tokens, issue dependency and sub-issue APIs. HTTP JSON ⇄ gRPC transcoding in Rust from `google.api.http` annotations, in process (HTTP routes calling the tonic service implementations directly) and as a standalone proxy, with OpenAPI generated from the same protos: evaluate existing crates against one unary RPC, one server-streaming RPC served as SSE, error mapping and auth pass-through, then adopt one or build our own as a separate open-source repo. Output: a findings report and pinned versions | experiments | none |
| 4. Stage prompts and artefact templates | instructions for analyse, design, plan, implement, review and address, with artefact templates, tried by hand on real repos | development and experiments | 2, 3 |
| 5. POC components | control plane core (state machine, scheduler, leases, event log, GitHub adapter), runner (stream client, worktrees, Claude adapter, job MCP, gates, pusher), console MCP, CLI; each split into tickets | development | 2, 3 |
| 6. POC acceptance | the worked example run end to end on a real repo, plus a ticket set with one dependency link | development | 4, 5 |

## Roadmap candidates and open questions

Ideas for later versions, mostly from a survey of open-source software factories; none is in scope until the roadmap schedules it.

**Post-MVP features**

| Feature | What it is | Design notes |
| --- | --- | --- |
| **Discovery pipelines** | Specialised agents on runners that *produce* work instead of consuming it: goal-driven app development (propose the next tickets toward a stated product goal), bug hunter, performance nerd, and others | A pluggable pack = instructions, tools, trigger (schedule, CI failure, error-tracker event, manual) and budget share. Output is proposed tickets with evidence (a failing test, a profile, a log excerpt) that land as drafts for your approval, deduplicated against open items. A cap on open proposals keeps them from flooding the queue. |
| **Review bricks** | Review as parallel sub-reviewers, each looking from one angle and all drawing on a shared principles dictionary. The angles are open and will be defined later; one working set is implementation approach (with alternative solutions), architecture and fit with the project, and line findings. Bricks contribute angles and principles: language (C++, Rust, Python, TypeScript), technology (userver, React, …) and concern (coding style beyond formatting, performance, concurrency, security). | A brick = principles (id, statement, rationale, examples, default severity) plus rules for when they apply (file globs, detected capabilities) and optional deterministic checks. Each finding cites the principle it rests on, which makes severity consistent, findings easy to deduplicate across sub-reviewers, and lets us track which principles fire, get accepted or get disputed. Sub-reviewers can run on different harnesses and on specialised review runners. Review jobs can also run from CI: the same bricks against a PR, started by a CI step (for example a GitHub Action calling `mintswarm review`), with findings posted as PR review comments, so teams can adopt review on its own and factory and CI reviews stay consistent. Projects pick bricks in their repository config; we ship a versioned catalogue, and repos add their own under `.mintswarm/review/`. The model is simplified and will be refined. |
| **Project knowledge base** | Project knowledge served to agents as tools, in three layers: prose docs, an ontology binding domain concepts to code symbols, and live code through the LSP (the model of [agent-kb](https://github.com/zmij/agent-kb), a local-first knowledge base that serves docs, an ontology and live code to coding agents over MCP) | A deterministic, self-healing pipeline (chunk, embed, index incrementally; verify concept bindings after each merge; heal what it can, file a ticket for what it cannot) plus separate knowledge agents that write docs and ontology entries from merged tickets and code, delivered as PRs reviewed like any other. Indexes track the base ref; an agent sees its own edits through the harness's LSP. The vector store is configured per project: a local Qdrant for the project, a Qdrant on the runner machine, or a cloud-hosted one. Sharing one store between projects is discouraged because of cross-contamination; where it happens anyway, each project gets its own collections and queries never cross them. Expected payoff: shorter analyse and plan stages from the KB, faster implement and review from KB plus LSP; measured as tokens and time per stage. |
| **Repository readiness check** | A deterministic check of how ready a repository is for agent work, run at project setup and on demand: binary, automatable signals such as linter, formatter and type checker configured; build succeeds on a fresh checkout; unit tests run, and how long they take; AGENTS.md or CLAUDE.md present; branch protection and CODEOWNERS | Runs as a runner job in a fresh worktree, in under five minutes, with signals defined as probe-style YAML so projects can add their own. Results feed project setup: suggested gate definitions, a warning per gap, and plan approval on by default while tests are missing. A discovery pack can turn the gaps into proposed tickets ("raise the floor"). The idea comes from Factory.ai's Agent Readiness scoring; we keep the binary signals and drop the points and levels. |
| **CUJ QA** | Customer user journey QA: given a scenario, an agent drives the software as a user would, through its API, CLI or web UI, and reports how well the docs and the interface carry it through. Non-web UIs come much later | Scenarios live in the repo under .mintswarm/cuj/ (goal, persona, surface, success criteria). The agent sees only what a user sees: the running software and its public docs, never the source, so gaps show up instead of being read around. The project says how to start the software for a run (a start script, like a workspace hook); web journeys need a browser, found by a capability probe. The report lists the steps taken, where the agent got stuck or guessed, missing or wrong docs, unhelpful errors, and turns, tokens and time per journey. Runs on demand, on a schedule, or on PRs that touch the user-facing surface; findings become proposed tickets, or go into the approval package when run on a PR. Covers the earlier change-aware QA candidate. |
| **Auto-landing** | Merging without a human once a project is mature: when its automated gates are strong enough, they replace your approval wherever the project's policy allows. Every PR still needs your approval in the POC and MVP | Gated by the project's autonomy tier and a risk score (diff size, paths touched, protected paths), and only when every gate, CI, review and acceptance check is green and no operator steered the run. Merges go through a merge queue; a post-merge watch reverts automatically if the default branch goes red; you get a digest of what landed. Depends on the trust features below (test-sensitivity gate, hidden acceptance tests, back-pressure) landing first. |

**Candidates from prior art** (see the [awesome-software-factories survey](https://github.com/OscarDoh/varun1505-awesome-software-factories))

| Idea | Source |
| --- | --- |
| Test-sensitivity gate: revert non-test changes, confirm new tests fail | [addyosmani/factory](https://github.com/addyosmani/factory) |
| Hidden acceptance tests written by a different model family, run by the runner | [dark-factory](https://github.com/DUBSOpenHub/dark-factory), [coleam00](https://github.com/coleam00/ai-software-factory) |
| Back-pressure: stop dispatching when N items await your review | [addyosmani/factory](https://github.com/addyosmani/factory) |
| Autonomy tiers per project with a default-deny charter | [addyosmani/factory](https://github.com/addyosmani/factory) |
| Gate verdict line quoted verbatim; required skip = MISCONFIGURED; fast / full / deep tiers | [addyosmani/factory](https://github.com/addyosmani/factory) |
| Model-independence provenance on review reports | [dark-factory](https://github.com/DUBSOpenHub/dark-factory) |
| Per-transition context fidelity (full, summary, fresh) | [StrongDM Attractor](https://github.com/strongdm/attractor) |
| Headless IMPLEMENT writes `assumptions.md` instead of asking | [Machinist](https://github.com/owainlewis/machinist) |
| Deduplicate new tickets against open items at ingest, not only discovery proposals | Factory.ai white paper |
| Docs updated in the same change as the code: an implement-stage instruction, and the approval package flags behaviour changes with no doc update where the repo has docs | Factory.ai white paper |
| Code-quality thresholds (file and function size, complexity) as default principles in a review brick, raised as findings, never as gates | Factory.ai white paper |

**Deferred:** Windows; runner Docker images; metered runners and hosted runner infrastructure; notification sinks (ntfy, Telegram) for when no session is attached.

**Open questions**

- What exactly goes into `.mintswarm/config.yaml`: gate definitions, autonomy tier, protected paths, required labels.
- Versioned roadmap: milestones after the MVP.
- Merge policies beyond the default (integration branch, stacked PRs) and how a dependent child PR waits for its predecessor to merge.
- A written answer from Anthropic and OpenAI on subscription-mode runners orchestrated by the commercial service.
- Sandboxing layer: adopt Anthropic's sandbox-runtime (`srt`) or keep our own bubblewrap / sandbox-exec layer.
- Warm-starting language servers in a fresh worktree from an index built elsewhere (the base clone or another worktree): whether it is safe and worth it, and per server how stale entries are detected.
