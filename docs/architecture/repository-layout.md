# Repository layout

One repository holds the Rust workspace, the TypeScript web UI, the API contracts and the Docker definitions. Directories appear as the work that needs them lands; this document says where each thing goes.

```
mintswarm/
├── Cargo.toml              # workspace: members, [workspace.dependencies], [workspace.lints]
├── Cargo.lock
├── rust-toolchain.toml     # pinned toolchain + rustfmt, clippy
├── rustfmt.toml
├── clippy.toml             # clippy settings for the workspace
├── deny.toml               # cargo-deny: licences, advisories, duplicates
├── compose.yaml            # development: Postgres; profile `image` builds and runs the local image
├── .dockerignore           # build context is the repo root
├── AGENTS.md               # conventions for any coding agent
├── CLAUDE.md               # Claude Code entry point
├── CONTRIBUTING.md
├── LICENSE.md              # FSL-1.1-ALv2
├── README.md
├── .cargo/config.toml      # Cargo: target-dir = "build/target"
│
├── .claude/agents/         # stage agents for Claude Code, generated from `.mintswarm/stages/`; dogfooding only
├── .mintswarm/             # this repository's own mintswarm config
│   ├── config.yaml         # tracker mapping, components, stage models, pipeline, gates
│   ├── stages.md           # conventions every stage agent follows
│   ├── stages/             # harness-neutral stage definitions: capabilities, model tier, instructions
│   ├── dogfood/            # stand-ins for runner and control-plane functions while mintswarm does not run; each script, and github/README.md, says what it replaces
│   ├── job/                # per-job inputs and outputs inside a worktree, never committed
│   └── worktrees/          # job worktrees created by the session acting as runner, never committed
│
├── build/                  # build output, git-ignored: build/target/ (Cargo), build/web/ui/ (Vite build of apps/ui), build/cache/vite/ (Vite and Vitest cache)
│
├── api/                    # every contract, language-neutral
│   ├── proto/              # the only hand-written API contract
│   │   ├── buf.yaml        # module root; lint (STANDARD) and breaking-change (FILE) rules
│   │   ├── buf.gen.yaml    # generator plugins; none yet, Rust comes from crates/proto/build.rs
│   │   └── mintswarm/
│   │       └── runner/v1/  # runner ⇄ plane stream (gRPC only)
│   ├── openapi/            # generated from proto; committed so PRs show JSON-level changes
│   └── schemas/            # JSON Schemas generated from Rust types (config, ticket, StageJob, plan.json, findings); committed
│
├── crates/                 # Rust, Cargo workspace members
│   ├── domain/             # ids; job states, events and the transition table; StageJob, StepJob, Usage; pipeline records; PlaneLink, Vcs, Tracker and CodeHost traits; Question objects and their checks; scales (size, priority), field maps, own level and the stride selector, cost model; capability probe results, namespaces and version requirements
│   ├── config/             # repository config (.mintswarm/config.yaml) and runner config; capability probe definitions and their built-in, machine and repository merge; native YAML loader matching yq
│   ├── github/             # the runner's GitHub App access: App JWT, installation tokens scoped to one repository, private-key store; issue-field and label reads and writes (FieldClient, field catalogue); GET-only issue, comment, link, artifact and PR reads (GitHubReads: paging, ETags, a retry-after pause), comment marks, content-creation limiter
│   ├── rules/              # pure rules shared by runner and plane: tracker field writes, size resolution; Python-parity globs, strict JSON reader and json.dumps writer, Markdown sections and fences, sha256 bindings, 0/1/2 exit contract; stage output capture (rules 1–6 and the msw-result block, with Claude Code transcript selection) and the stage-aware result checks; the plan floor over a sha256-bound input bundle
│   ├── proto/              # Rust generated from api/proto by build.rs (protox) into OUT_DIR, not committed; `fakes` feature for test fakes
│   ├── core/               # control plane library: store, scheduler, ingest, event log, trackers, ext traits, HTTP transcoding layer
│   │   └── migrations/     # SQL, append-only
│   ├── server/             # `mintswarm-server` binary: serve, migrate (the image's entry point)
│   ├── steward/            # `mintswarm-steward` binary: the process that holds the deployment's signing keys apart from the control plane
│   ├── runner/             # `mintswarm-runner` library and binary: `exec::git`, the git VCS adapter: trusted bare store, neutral git environment, fetch and read operations; `tool save-output`, the Rust port of `save-output.py`
│   ├── console-mcp/        # console MCP server (library)
│   └── cli/                # `mintswarm` binary: CLI and `mcp console`
│
├── web/                    # TypeScript, pnpm workspace; its root is here, not at the repo root
│   ├── package.json        # workspace scripts; packageManager pins pnpm
│   ├── pnpm-workspace.yaml
│   ├── pnpm-lock.yaml
│   ├── .node-version
│   ├── tsconfig.json       # shared compiler options
│   ├── eslint.config.mjs   # lint config
│   ├── eslint.extra.config.mjs
│   ├── .prettierrc.json    # formatter settings
│   ├── .prettierignore
│   ├── apps/
│   │   └── ui/             # React + Vite; builds static files for the image
│   └── packages/
│       └── api/            # @mintswarm/api: TS types and client generated from api/openapi
│
├── docker/
│   ├── control-plane/
│   │   ├── Dockerfile      # stages: rust build → web build → runtime (nginx + core + UI)
│   │   ├── nginx/          # nginx.conf: TLS, static UI, /api + SSE, grpc_pass for runners
│   │   └── s6/             # s6-overlay service definitions: nginx, core
│   └── compose.yaml        # self-hosting example: released image + Postgres
│
├── packaging/              # native runner distribution
│   ├── systemd/            # user unit template
│   ├── launchd/            # agent plist template
│   └── macos/              # .pkg build, entitlements for notarisation
│
├── defaults/               # data embedded into binaries at build time
│   ├── probes/             # built-in capability probes
│   ├── lsp/                # language-server definitions
│   └── prompts/            # stage instructions and artefact templates, versioned
│
├── tests/
│   └── e2e/                # end-to-end: plane + runner + Postgres + a scratch repo
├── xtask/                  # cargo xtask: codegen, schema export, drift checks, release helpers
│
├── docs/
│   ├── architecture/       # how the system is built, as it stands now
│   │   ├── repository-layout.md  # this document: where each thing goes
│   │   └── runner-protocol.md    # the runner ⇄ plane protocol as api/proto/mintswarm/runner/v1/ defines it
│   ├── conventions/        # Rust, TypeScript and React conventions copied from the guideline packs
│   └── guides/             # user-facing: setup, credentials and usage policy, …
│
└── .github/
    ├── workflows/          # gates.yml (one check per gates.run entry), image.yml (path-filtered), release.yml
    ├── CODEOWNERS
    └── pull_request_template.md
```

Not in this repository: the commercial crates, the Node BFF and the deployment config of our own hosted instance; they live in the private repository.

## Rules

- **Current state only.** Documents describe the system as it is and are edited in place when it changes. Proposals, alternatives and the reasoning behind changes live in the tracker (design issues, PRs) and in git history, never as files here.
- **Rust:** crate names are `mintswarm-<dir>`; every crate inherits `version`, `edition`, `license` and `lints` from the workspace.
- **Dependency direction is one-way** (`A` ← `B` means `B` may depend on `A` and `A` never on `B`; `domain` holds the shared abstractions that the crates above build on or implement): `domain` ← `config` ← `proto` ← {`core`, `runner`, `console-mcp`} ← {`server`, `cli`}, beside it `domain` ← `github` ← {`core`, `runner`}, and `domain` ← `config` ← `rules` ← `runner`; `proto` never depends on `github`. `core` and `runner` never depend on each other; anything they share goes down a level. Today `github` depends only on `domain`, `runner` on `domain` and `rules`, `rules` on `domain` and `config`, `config` only on `domain`, and `domain` and `proto` on no other workspace crate. `steward` stands apart: it depends on no workspace crate today.
- **Extension traits** (`Tenancy`, `AuthProvider`, `SecretStore`, `UsageSink`) live in `core::ext` with the self-hosted implementations. They get their own crate only if the private repository needs them without the rest of core.
- **Split a crate when it earns it:** an adapter or the probe engine becomes its own crate when a second consumer appears or compile time hurts.
- **TypeScript:** packages are `@mintswarm/<dir>`; Node and pnpm versions are pinned in `web/`. Only `packages/*` are publishable; `apps/*` are private.
- **Protos** (`api/proto/`): `buf lint` and `buf breaking` against `master` run in CI. Rust is generated in `build.rs` of `crates/proto`, not committed. Messages exposed over HTTP are written with the proto3 JSON mapping in mind. The `runner/v1` protocol is described in [runner-protocol.md](runner-protocol.md).
- **HTTP routes** come from `google.api.http` annotations, never from hand-written handlers. The event stream for the browser (`text/event-stream`) is the one exception if the transcoder cannot serve a server-streaming RPC as SSE.
- **The transcoding layer is isolated.** HTTP JSON is transcoded in process by one module in core; nothing outside it names the transcoding crate, so it can be replaced.
- **Generated contracts are committed:** `api/openapi/` (from the protos) and `api/schemas/` (from Rust types, via `cargo xtask schemas`). CI regenerates both and fails on drift. TypeScript in `web/packages/api` is generated from `api/openapi/` and not committed.
- **Commercial BFF:** implements the same proto services, so the UI never gets more than the console MCP.
- **Versioning:** a breaking change to any contract gets a new version directory (`v2/`) beside the old one until the old version is retired.
- **Migrations are append-only.** A merged migration is never edited; fix forward.
- **Build tree:** all build output goes only under the git-ignored `build/` at the repository root, never next to sources: Cargo's under `build/target` (set in `.cargo/config.toml`), Vite's build and cache under `build/web/ui` and `build/cache/vite` (set in `web/apps/ui/vite.config.ts`). Dependency installs such as `node_modules/` are not build output.
- **Docker:** every image builds from the repo root as context; `.dockerignore` keeps `build/` and `node_modules/` out. Runner images, when they come, go in `docker/runner/`.
- **`defaults/` is data.** Prompts carry a template version that ends up in each `StageJob`.
- **Spikes never land.** Experimental code lives on `exp/*` branches; its findings go to the spike's issue, and what they settle goes into these documents.

## Decisions

Reasoning and rejected alternatives: [#3, repository layout](https://github.com/mintswarm/mintswarm/issues/3#issuecomment-7078340560).

- One repository for Rust, TypeScript and Docker: contract changes stay one PR.
- Contracts under `api/`; proto is the only hand-written API contract; OpenAPI and TypeScript are generated from it.
- The browser speaks HTTP JSON, transcoded in process from `google.api.http` annotations; we start with structured-proxy in embedded mode.
- Separate `mintswarm-server` and `mintswarm` binaries, so client machines carry no server code.
- `cargo xtask` for repository tasks.
- No decision records in the repository: it holds the current state, the tracker holds the reasoning.
