# Repository layout

One repository holds the Rust workspace, the TypeScript web UI, the API contracts and the Docker definitions. Directories appear as the work that needs them lands; this document says where each thing goes.

```
mintswarm/
├── Cargo.toml              # workspace: members, [workspace.dependencies], [workspace.lints]
├── Cargo.lock
├── rust-toolchain.toml     # pinned toolchain + rustfmt, clippy
├── rustfmt.toml
├── deny.toml               # cargo-deny: licences, advisories, duplicates
├── compose.yaml            # development: Postgres; profile `image` builds and runs the local image
├── .dockerignore           # build context is the repo root
├── AGENTS.md               # conventions for any coding agent
├── CLAUDE.md               # Claude Code entry point
├── CONTRIBUTING.md
├── LICENSE.md              # FSL-1.1-ALv2
├── README.md
│
├── .claude/agents/         # stage agents for Claude Code, generated from `.mintswarm/stages/`; dogfooding only
├── .mintswarm/             # this repository's own mintswarm config
│   ├── config.yaml         # tracker mapping, components, stage models, pipeline, gates
│   ├── stages.md           # conventions every stage agent follows
│   ├── stages/             # harness-neutral stage definitions: capabilities, model tier, instructions
│   ├── dogfood/            # stand-ins for runner functions while the system does not run (gate runner, stage agent renderer)
│   ├── job/                # per-job inputs and outputs inside a worktree, never committed
│   └── worktrees/          # job worktrees created by the session acting as runner, never committed
│
├── api/                    # every contract, language-neutral
│   ├── proto/              # the only hand-written API contract
│   │   ├── buf.yaml        # lint + breaking-change rules; deps include googleapis (google.api.http)
│   │   ├── buf.gen.yaml
│   │   └── mintswarm/
│   │       ├── runner/v1/  # runner ⇄ plane stream (gRPC only)
│   │       ├── control/v1/ # Control API (CLI, console MCP; browser-facing RPCs annotated for HTTP)
│   │       └── ui/v1/      # UI-only services, if any (aggregations for screens), HTTP-annotated
│   ├── openapi/            # generated from proto; committed so PRs show JSON-level changes
│   └── schemas/            # JSON Schemas generated from Rust types (config, ticket, StageJob, plan.json, findings); committed
│
├── crates/                 # Rust, Cargo workspace members
│   ├── domain/             # ids, entities, states + transition table, usage shape, StageJob
│   ├── config/             # control-plane, runner and repo config; YAML 1.2 loader; schema generation
│   ├── proto/              # code generated from api/proto (build.rs, not committed)
│   ├── core/               # control plane library: store, scheduler, ingest, event log, trackers, ext traits, HTTP transcoding layer
│   │   └── migrations/     # SQL, append-only
│   ├── server/             # `mintswarm-server` binary: serve, migrate (the image's entry point)
│   ├── runner/             # `mintswarm-runner` binary + library
│   ├── console-mcp/        # console MCP server (library)
│   └── cli/                # `mintswarm` binary: CLI and `mcp console`
│
├── web/                    # TypeScript, pnpm workspace; its root is here, not at the repo root
│   ├── package.json        # workspace scripts; packageManager pins pnpm
│   ├── pnpm-workspace.yaml
│   ├── pnpm-lock.yaml
│   ├── .node-version
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
│   └── guides/             # user-facing: setup, credentials and usage policy, …
│
└── .github/
    ├── workflows/          # rust.yml, web.yml, image.yml (path-filtered), release.yml
    ├── CODEOWNERS
    └── pull_request_template.md
```

Not in this repository: the commercial crates, the Node BFF and the deployment config of our own hosted instance; they live in the private repository.

## Rules

- **Current state only.** Documents describe the system as it is and are edited in place when it changes. Proposals, alternatives and the reasoning behind changes live in the tracker (design issues, PRs) and in git history, never as files here.
- **Rust:** crate names are `mintswarm-<dir>`; every crate inherits `version`, `edition`, `license` and `lints` from the workspace.
- **Dependency direction is one-way:** `domain` ← `config` ← `proto` ← {`core`, `runner`, `console-mcp`} ← {`server`, `cli`}. `core` and `runner` never depend on each other; anything they share goes down a level.
- **Extension traits** (`Tenancy`, `AuthProvider`, `SecretStore`, `UsageSink`) live in `core::ext` with the self-hosted implementations. They get their own crate only if the private repository needs them without the rest of core.
- **Split a crate when it earns it:** an adapter or the probe engine becomes its own crate when a second consumer appears or compile time hurts.
- **TypeScript:** packages are `@mintswarm/<dir>`; Node and pnpm versions are pinned in `web/`. Only `packages/*` are publishable; `apps/*` are private.
- **Protos** (`api/proto/`): `buf lint` and `buf breaking` against `master` run in CI. Rust is generated in `build.rs` of `crates/proto`, not committed. Messages exposed over HTTP are written with the proto3 JSON mapping in mind.
- **HTTP routes** come from `google.api.http` annotations, never from hand-written handlers. The event stream for the browser (`text/event-stream`) is the one exception if the transcoder cannot serve a server-streaming RPC as SSE.
- **The transcoding layer is isolated.** HTTP JSON is transcoded in process by one module in core; nothing outside it names the transcoding crate, so it can be replaced.
- **Generated contracts are committed:** `api/openapi/` (from the protos) and `api/schemas/` (from Rust types, via `cargo xtask schemas`). CI regenerates both and fails on drift. TypeScript in `web/packages/api` is generated from `api/openapi/` and not committed.
- **Commercial BFF:** implements the same proto services, so the UI never gets more than the console MCP.
- **Versioning:** a breaking change to any contract gets a new version directory (`v2/`) beside the old one until the old version is retired.
- **Migrations are append-only.** A merged migration is never edited; fix forward.
- **Docker:** every image builds from the repo root as context; `.dockerignore` keeps `target/` and `node_modules/` out. Runner images, when they come, go in `docker/runner/`.
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
