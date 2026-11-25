# Repository layout

One repository holds the Rust workspace, the TypeScript web UI, the API contracts and the Docker definitions. Directories appear as the work that needs them lands; this document says where each thing goes.

```
mintswarm/
├── Cargo.toml              # Cargo workspace manifest
├── Cargo.lock
├── rust-toolchain.toml     # pinned Rust toolchain
├── rustfmt.toml
├── clippy.toml             # clippy settings for the workspace
├── deny.toml               # cargo-deny dependency policy
├── compose.yaml            # development compose file
├── .dockerignore           # Docker build context filter
├── AGENTS.md               # conventions for any coding agent
├── CLAUDE.md               # Claude Code entry point
├── CONTRIBUTING.md
├── LICENSE.md              # FSL-1.1-ALv2
├── README.md
├── .cargo/config.toml      # Cargo build settings
│
├── .claude/agents/         # generated Claude Code agents
├── .mintswarm/             # this repository's own mintswarm config
│   ├── config.yaml         # repository mintswarm config
│   ├── stages.md           # conventions every stage agent follows
│   ├── stages/             # harness-neutral stage definitions
│   ├── dogfood/            # temporary runner and plane stand-ins
│   ├── job/                # uncommitted per-job inputs and outputs
│   └── worktrees/          # uncommitted job worktrees
│
├── build/                  # git-ignored build output
│
├── api/                    # every contract, language-neutral
│   ├── proto/              # the only hand-written API contract
│   │   ├── buf.yaml        # buf module and rules
│   │   ├── buf.gen.yaml    # buf generator plugins
│   │   └── mintswarm/
│   │       └── runner/v1/  # runner ⇄ plane gRPC stream
│   ├── openapi/            # OpenAPI generated from proto
│   └── schemas/            # JSON Schemas from Rust types
│
├── crates/                 # Rust workspace members
│   ├── domain/             # shared types and traits
│   ├── config/             # repository and runner config
│   ├── github/             # GitHub App client
│   ├── rules/              # shared pure rules
│   ├── proto/              # Rust generated from protos
│   ├── core/               # control plane library
│   │   └── migrations/     # append-only SQL migrations
│   ├── server/             # `mintswarm-server` binary
│   ├── steward/            # `mintswarm-steward` signing-key binary
│   ├── runner/             # `mintswarm-runner` daemon
│   ├── console-mcp/        # console MCP server (library)
│   └── cli/                # `mintswarm` CLI binary
│
├── web/                    # TypeScript pnpm workspace root
│   ├── package.json        # pnpm workspace scripts
│   ├── pnpm-workspace.yaml
│   ├── pnpm-lock.yaml
│   ├── .node-version
│   ├── tsconfig.json       # shared compiler options
│   ├── eslint.config.mjs   # lint config
│   ├── eslint.extra.config.mjs
│   ├── .prettierrc.json    # formatter settings
│   ├── .prettierignore
│   ├── apps/
│   │   └── ui/             # React + Vite UI
│   └── packages/
│       └── api/            # generated `@mintswarm/api` client
│
├── docker/
│   ├── control-plane/
│   │   ├── Dockerfile      # control-plane image build
│   │   ├── nginx/          # nginx front-end config
│   │   └── s6/             # s6-overlay service definitions
│   └── compose.yaml        # self-hosting compose example
│
├── packaging/              # native runner distribution
│   ├── systemd/            # user unit template
│   ├── launchd/            # agent plist template
│   └── macos/              # macOS .pkg and notarisation
│
├── defaults/               # data embedded into binaries
│   ├── probes/             # built-in capability probes
│   ├── lsp/                # language-server definitions
│   └── prompts/            # versioned stage prompts and templates
│
├── tests/
│   └── e2e/                # end-to-end system tests
├── xtask/                  # `cargo xtask` repository tasks
│
├── docs/
│   ├── architecture/       # how the system is built
│   │   ├── repository-layout.md  # where each thing goes
│   │   └── runner-protocol.md    # runner ⇄ plane protocol
│   ├── conventions/        # Rust, TypeScript and React conventions
│   └── guides/             # user-facing setup and usage guides
│
└── .github/
    ├── workflows/          # GitHub Actions CI workflows
    ├── CODEOWNERS
    └── pull_request_template.md
```

Not in this repository: the commercial crates, the Node BFF and the deployment config of our own hosted instance; they live in the private repository.

## Rules

- **Current state only.** Documents describe the system as it is and are edited in place when it changes. Proposals, alternatives and the reasoning behind changes live in the tracker (design issues, PRs) and in git history, never as files here.
- **Briefs only.** Each entry in the tree above is a 3–5 word brief saying what the directory or file is, never what it contains. Adding a module, type or subcommand needs no edit to this document; only a new directory, a new dependency edge or a changed rule does.
- **Rust:** crate names are `mintswarm-<dir>`; every crate inherits `version`, `edition`, `license` and `lints` from the workspace.
- **Dependency direction is one-way.** `A` ← `B` means `B` may depend on `A` and `A` never on `B`; `domain` holds the shared abstractions that the crates above build on or implement. The allowed edges between workspace crates:
  - `domain` ← `config`
  - `domain` ← `github`
  - `domain` ← `rules`
  - `config` ← `rules`
  - `domain` ← `runner`
  - `config` ← `runner`
  - `rules` ← `runner`
  - `core` ← `server`
  - `github` ← `server`

  `core` and `runner` never depend on each other; anything they share goes down a level. `proto` never depends on `github`. `steward` stands apart and depends on no workspace crate.
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
