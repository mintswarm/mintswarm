<!-- mintswarm-pack: rust 2 -->
# Rust conventions

How Rust code in this repository is built, formatted, linted, tested and organised.

## Toolchain and edition

- The toolchain is Rust `1.89`, pinned in `rust-toolchain.toml` with the `rustfmt` and `clippy` components.
- The edition is `2024`. It is set once, in `[workspace.package]` of the root `Cargo.toml`, and every crate inherits it with `edition.workspace = true`.
- The toolchain is at least Rust 1.85, the first release with edition 2024 and rustfmt's `style_edition`.

## Formatting

- rustfmt owns formatting, configured by `rustfmt.toml`, with lines of at most `100` columns.
- Never format by hand and never disable rustfmt for a file; run `cargo fmt --all` before committing.

## Lints

- Clippy is clean under `-D warnings`: a warning fails the lint check.
- The lints are set once, in `[workspace.lints]` of the root `Cargo.toml`, and every crate takes them with `[lints] workspace = true`.
- Suppress a lint narrowly, with `#[expect(lint, reason = "…")]` on the smallest item that needs it, never with a crate-wide `#[allow]`.

## Errors

- Libraries return typed errors: an error enum per module or crate, implementing `std::error::Error`.
- Binaries may use a catch-all error type at their top level.
- `unwrap` is used only in `#[test]` functions and `#[cfg(test)]` code. An integration-test crate in `tests/` starts with `#![cfg(test)]`, or its helpers use `expect` with a message.
- Outside tests, `expect` is used only with a message that states the invariant that makes it safe.

## Unsafe code

- Unsafe code is linted at `forbid` for the whole workspace.
- Where it is allowed, every `unsafe` block has a `// SAFETY:` comment saying why the block is sound.

## Naming and documentation

- Names and public APIs follow the Rust API Guidelines.
- Every public item has a doc comment, and every crate a crate-level `//!` comment.

## Tests

- Unit tests live in a `#[cfg(test)] mod tests` next to the code they test.
- Integration tests live in the crate's `tests/` directory.

## Dependencies

- Dependency versions are declared once, in `[workspace.dependencies]` of the root `Cargo.toml`, and crates take them with `workspace = true`.
- Features are additive: enabling a feature never removes or changes behaviour.
- `Cargo.lock` at the workspace root is committed.

## Build output

- Build output goes under `build/target`, set by `target-dir` in `[build]` of `.cargo/config.toml`, never under Cargo's default `target/`.
- `build/target` is git-ignored, by default through the root `build/` entry in `.gitignore`, so build output is never committed.
- Where `.cargo/config.toml` already has a `[build]` header without `target-dir`, the setting is not applied there, so add `target-dir = "build/target"` to that section by hand.

## Local checks

All three pass before pushing:

- `cargo fmt --all --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo nextest run --workspace && cargo test --doc --workspace`, which needs `cargo-nextest` at the version `.github/workflows/gates.yml` pins; nextest runs each test in its own process and does not run doctests, so `cargo test --doc` runs those
