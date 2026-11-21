# 0001 — Repository layout

- Status: proposed
- Date: 2026-11-21

## Context

`docs/concept.md` fixes a few things the layout has to serve:

- A Rust workspace with, at minimum, core (control plane), runner, proto, CLI and console MCP (work package 1).
- A React/TypeScript UI from the MVP on, speaking the UI API. The self-hosted edition and the commercial edition share one UI code path; the commercial edition adds a Node BFF that implements the same UI API.
- A private commercial repo that depends on the public crates and plugs in its own `Tenancy`, `AuthProvider`, `SecretStore` and `UsageSink`, plus the BFF. Crate and package boundaries are therefore public API.
- One control-plane image (nginx + core + UI static files under s6-overlay) for amd64 and arm64, a convenience compose file with Postgres, and signed native runner binaries via cargo-dist.
- Data that ships with the binaries but is not code: built-in capability probes, language-server definitions, stage prompt templates, generated JSON Schemas.
- A folder for decision records and the outputs of the harness experiments (work package 3).
- The repo itself will eventually be worked on by mintswarm, so `.mintswarm/` and the `mintswarm/*` branch prefix are reserved.

## Decision

**One repository** for Rust, TypeScript and Docker definitions. The UI and core meet at a contract (`api/proto/`); a change to it touches the definition, the Rust side and the TypeScript side, and in one repo that is one PR, one review and one CI run.

**All contracts live under `api/`.** Proto files are the only hand-written API contract. Services the browser uses carry `google.api.http` annotations; the HTTP JSON API is transcoded from them in process, calling the same tonic service implementations, so there is one handler per operation. The OpenAPI spec is generated from the protos, and the TypeScript client from the OpenAPI spec, so the browser sees plain HTTP JSON. JSON Schemas for files and payloads are generated from the Rust types. We start with structured-proxy in embedded mode, behind an isolated transcoding layer in core: nothing outside it names the crate, so it can be replaced by a fork or our own generator. The work package 3 spike validates it and gets its own ADR. The image bundles the UI built from the same commit, and the whole thing carries one version. Two toolchains in CI are handled with path filters; Rust-only contributors never need Node.

```
mintswarm/
├── Cargo.toml              # workspace: members, [workspace.dependencies], [workspace.lints]
├── Cargo.lock
├── rust-toolchain.toml     # pinned toolchain + rustfmt, clippy
├── rustfmt.toml
├── deny.toml               # cargo-deny: licences, advisories, duplicates
├── compose.yaml            # development: Postgres; profile `image` builds and runs the local image
├── .dockerignore           # build context is the repo root
├── AGENTS.md               # conventions for any coding agent (source of truth)
├── CLAUDE.md               # @AGENTS.md
├── CONTRIBUTING.md
├── LICENSE.md              # FSL-1.1-ALv2
├── README.md
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
│   ├── core/               # control plane library: store, scheduler, ingest, event log, trackers, ext traits
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
├── packaging/              # native runner distribution (MVP)
│   ├── systemd/            # user unit template
│   ├── launchd/            # agent plist template
│   └── macos/              # .pkg build, entitlements for notarisation
│
├── defaults/               # data embedded into binaries at build time
│   ├── probes/             # built-in capability probes
│   ├── lsp/                # language-server definitions (MVP)
│   └── prompts/            # stage instructions and artefact templates, versioned
│
├── tests/
│   └── e2e/                # end-to-end: plane + runner + Postgres + a scratch repo (work package 6)
├── xtask/                  # cargo xtask: codegen, schema export, drift checks, release helpers
├── experiments/            # spikes (work package 3); not built, never depended on
│
├── docs/
│   ├── concept.md
│   ├── adr/                # NNNN-title.md
│   ├── experiments/        # findings reports
│   └── guides/             # user-facing: setup, credentials and usage policy, …
│
└── .github/
    ├── workflows/          # rust.yml, web.yml, image.yml (path-filtered), release.yml
    ├── CODEOWNERS
    └── pull_request_template.md
```

Not in this repo: the commercial crates, the Node BFF and the deployment config for our own hosted instance (`fly.toml`, secrets); they live in the private repo. `.mintswarm/` is created when we start dogfooding.

### Rules that come with it

- **Rust:** crate names are `mintswarm-<dir>`; every crate inherits `version`, `edition`, `license` and `lints` from the workspace.
- **Dependency direction is one-way:** `domain` ← `config` ← `proto` ← {`core`, `runner`, `console-mcp`} ← {`server`, `cli`}. `core` and `runner` never depend on each other; anything they share goes down a level.
- **Extension traits** (`Tenancy`, `AuthProvider`, `SecretStore`, `UsageSink`) live in `core::ext` with the self-hosted implementations. They get their own crate only if the private repo needs them without the rest of core.
- **Split a crate when it earns it:** an adapter or the probe engine becomes its own crate when a second consumer appears or compile time hurts.
- **TypeScript:** packages are `@mintswarm/<dir>`; Node and pnpm versions are pinned in `web/`. Only `packages/*` are publishable; `apps/*` are private.
- **Protos** (`api/proto/`): `buf lint` and `buf breaking` against `master` run in CI. Rust is generated in `build.rs` of `crates/proto`, not committed. Messages exposed over HTTP are written with the proto3 JSON mapping in mind (style rules in `AGENTS.md`).
- **HTTP routes** come from `google.api.http` annotations, never from hand-written handlers. The event stream for the browser (`text/event-stream`) is the one exception if the transcoder cannot serve a server-streaming RPC as SSE.
- **Generated contracts are committed:** `api/openapi/` (from the protos) and `api/schemas/` (from Rust types, via `cargo xtask schemas`). They are published and reviewed; CI regenerates both and fails on drift. TypeScript in `web/packages/api` is generated from `api/openapi/` and not committed.
- **Commercial BFF:** implements the same proto services (for example with Connect for Node), so the UI never gets more than the console MCP.
- **Versioning:** a breaking change to any contract gets a new version directory (`v2/`) beside the old one, never an in-place edit, until the old version is retired.
- **Migrations are append-only.** A merged migration is never edited; fix forward.
- **Docker:** every image builds from the repo root as context, so it can reach `crates/`, `web/` and `defaults/`; `.dockerignore` keeps `target/` and `node_modules/` out. Runner images are deferred; when they come, they go in `docker/runner/`.
- **`defaults/` is data.** Prompts carry a template version that ends up in each `StageJob`.
- **`experiments/` never ships.** Conclusions go to `docs/experiments/` and, where they settle a question, to an ADR.

## Alternatives considered

- **UI in its own repository.** Rejected: every UI API change would be a coordinated two-repo change, and the image would have to pin a UI release separately. Worth revisiting only if the UI gets its own team or release cadence.
- **pnpm workspace at the repo root.** Rejected: it puts `package.json` and `node_modules/` beside `Cargo.toml` for everyone. Keeping it under `web/` costs a `cd web` in scripts.
- **Separate repos for runner and control plane.** Rejected: they share `domain`, `config` and `proto`.
- **Hand-written OpenAPI beside the protos.** Rejected: two contract languages describing the same operations, and a second set of handlers in core.
- **grpc-gateway (Go) as a sidecar.** Rejected: adds Go to the build and a process to the image for something that can run in core's own process.
- **Connect or gRPC-Web in the browser.** Rejected: RPC-style URLs and a protocol the browser needs a library for; plain HTTP JSON is easier to debug and to call from scripts.
- **Code-first OpenAPI (generated from Rust handlers).** Rejected: the handlers would become the contract, and the BFF implements the same API in another language.
- **Contracts at the repo root (`proto/`, `openapi/`, `schemas/`).** Rejected: `api/` gives one place to review and one CODEOWNERS rule for everything that is a promise to another component.
- **One `mintswarm` binary that also serves.** Rejected: it would put server code on every client machine. The design doc's `mintswarm migrate` becomes `mintswarm-server migrate`.
- **Makefile or justfile instead of xtask.** Possible as a thin wrapper later; xtask keeps the logic in Rust and needs no extra tool.

## Open points

- HTTP ⇄ gRPC transcoding (work package 3 spike): adopt an existing crate or build a generator that works both in process and as a standalone proxy, possibly as a separate open-source repo. Lead candidate: [structured-proxy](https://github.com/structured-world/structured-proxy) (Apache-2.0), which has both modes: embedded, with `ProxyServer::service(tonic::service::Routes)` calling our tonic services in process through their full stack, and a standalone binary; REST and native gRPC on one port; NDJSON or SSE for server streaming; `google.rpc.Status` errors; `openapi::generate` callable from an xtask. Risks: one maintainer, eight major versions in eight months, and a dependency tree sized for a full edge (JWT, rate limits, Prometheus, ext_authz) of which we need only the transcoder. Auth stays in core: a tonic interceptor checks every call, native or transcoded, and the proxy's own auth is off; its `AuthDecider` and `TokenVerifier` hooks are there if we ever want to reject at the HTTP edge. JSON is ProtoJSON through `prost-reflect` (camelCase names, enum names, 64-bit integers as strings, well-known types in their string forms, unknown body keys rejected), with default-valued fields always emitted. Exit plan if we outgrow it: fork the transcoder and OpenAPI generator (about 13k lines with tests, Apache-2.0) into our own crate. Other candidates: [tonic-rest](https://docs.rs/crate/tonic-rest-build/latest/source/README.md) (build-time, `google.api.http`, axum handlers calling tonic traits, OpenAPI 3.1), [tinc](https://docs.rs/crate/tinc/latest) (codegen, own annotations, REST streaming, OpenAPI 3.1; self-described as unstable), [structured-proxy](https://docs.rs/crate/structured-proxy/2.2.1) (runtime proxy from a descriptor set), [g2h](https://docs.rs/crate/g2h/0.5.0/source/README.md) (axum handlers by method path, not REST bindings).
- Whether `docs/guides/` becomes a docs site (mintswarm.dev) built from this repo.
