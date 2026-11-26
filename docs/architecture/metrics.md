# Factory metrics and facts

mintswarm measures itself from facts: small records of what happened to an item, written from the first dogfooding journal line onwards and later sent as OpenTelemetry (OTLP) spans and log records. This document describes the fact model, provenance and the ids. The keys, value sets and kinds are defined once, as Rust types in `domain::facts` ([`crates/domain/src/facts.rs`](../../crates/domain/src/facts.rs) and its submodules; `cargo doc -p mintswarm-domain` renders them), and the JSON Schema [`api/schemas/fact.schema.json`](../../api/schemas/fact.schema.json) is generated from those types. Read the keys and values there; this document does not restate them. The design is in [#412](https://github.com/mintswarm/mintswarm/issues/412).

## The fact model

- **An item** is one tracker issue. Each item is one OTLP trace, and its root span runs from the issue's creation to its landing.
- **A fact kind** is either a span below the root (a stage run, a model turn, a tool call, a phase, a wait) or a log record attached to the root or to one of those spans (a run started, a decision requested or taken, a PR opened, a state change and so on). `domain::facts::FactKind` lists every kind with its OTLP signal, its span or event name, what it attaches to and, for a log, its natural key. The schema carries the same metadata as an `x-otlp` annotation on each kind.
- **The serde form** (`domain::facts::Fact`) is what a writer hands to the journal. It is tagged by `kind`, and its envelope holds:
  - `item`: the item's tracker host, repository, number, kind and components;
  - `time` for a log, or `start` and `end` for a span, in Unix nanoseconds;
  - `concerns`: for a log, the span it attaches to (absent for the root); for a span, its own `local` parts and its parent (absent for the root);
  - `links`, on spans only: span links, such as a stage run's trigger or a child item's parent;
  - `attributes`: the kind's attributes, as one flat object of OTLP attribute keys.
- **Attributes** are grouped. A group such as provenance, the run or the token counts is one Rust type that several kinds hold as a field, so every key is declared by exactly one type. Each kind and group rejects keys it does not declare, and every closed value set is a Rust enum.
- **Semantic conventions.** `gen_ai.*` and `system.*` keys are used as the OpenTelemetry semantic conventions define them; the pinned version is the scope's `schema_url`, `domain::facts::SCHEMA_URL`.
- **Keys are frozen once written.** A renamed key would force readers to keep an alias, so keys and values only ever add. Each addition raises `domain::facts::CATALOGUE_VERSION`, the version of the `mintswarm.facts` instrumentation scope every fact is written under.
- **The schema** is regenerated with `cargo xtask schemas`; the `schemas-drift` gate runs `cargo xtask schemas --check`, which fails when the committed file differs from the types. It uses only `type`, `enum`, `const`, `required`, `properties`, `additionalProperties: false`, `oneOf` on `kind`, `items`, `minimum` and `pattern`, plus the `x-otlp` annotation, so a small validator can check facts against it.

## Provenance

Every fact says how it was obtained:

- `live` facts are written as the event happens, by the session, a tool, the runner or the plane;
- `reconstructed` facts are rebuilt afterwards from transcripts, GitHub or job records, and list what could not be recovered as gaps, for example an unrecorded start.

A live and a reconstructed log record of the same event derive the same fact id, so a reader keeps one of them.

## Ids

Ids are derived, never stored or allocated, so every writer in every language computes the same ones (`domain::facts::ids`).

- **The item key** is `<tracker host>/<owner>/<repo>#<number>`, with host, owner and repository lowercased, for example `github.com/mintswarm/mintswarm#412`.
- **`H(tag, parts…)`** is SHA-256 over the tag and then each part, each followed by a newline. An id is the first bytes of the digest; if they are all zero, the last byte is set to 1.
- **The trace id** is the first 16 bytes of `H("mintswarm.trace.v1", key)`, and **the root span id** the first 8 bytes of `H("mintswarm.root.v1", key)`.
- **A span id** is the first 8 bytes of `H("mintswarm.span.v1", key, kind, local…)`, where the `local` parts identify the span within its item:
  - a stage run: the run id (the subagent's agent id while dogfooding, the attempt id in the product);
  - a turn: the run's span id in hex and the model's message id;
  - a tool call: the run's span id in hex and the tool-use id;
  - a phase: the run's span id in hex, the phase name and its ordinal;
  - a wait: the wait kind and the id of what opened it.
- **A fact id** is the first 16 bytes of `H("mintswarm.fact.v1", key, event name, natural key…)`, written in hex as `mintswarm.fact.id` wherever a link names a record. Each log kind declares the ordered parts of its natural key, in `FactKind` and in the schema's `x-otlp`. A part is an attribute key (its string as it is, a number or boolean as JSON text, an absent value as the empty string), `concerns` (the concerned span's id in hex, the root's when absent) or `time` (decimal Unix nanoseconds). Decisions and interventions taken from GitHub key on the object's URL, so the live writer and the backfill derive the same id.
- **The shared fixture** [`crates/domain/tests/fixtures/fact-ids.json`](../../crates/domain/tests/fixtures/fact-ids.json) lists item keys with their trace, root and per-kind span ids, and one example fact per kind with its span and fact ids. Its expected ids were computed independently of the Rust code; the domain tests derive every id and match it exactly, and other implementations check themselves against the same file.
