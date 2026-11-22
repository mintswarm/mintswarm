# Rust conventions

How Rust code in this repository is built, formatted, linted, tested and organised.

## Toolchain and edition

- The toolchain is Rust `{{ rust_toolchain }}`, pinned in `rust-toolchain.toml` with the `rustfmt` and `clippy` components.
- The edition is `{{ edition }}`. It is set once, in `[workspace.package]` of the root `Cargo.toml`, and every crate inherits it with `edition.workspace = true`.
- Edition 2024 needs Rust 1.85 or later.

## Formatting

- rustfmt owns formatting, configured by `rustfmt.toml`, with lines of at most `{{ max_width }}` columns.
- Never format by hand and never disable rustfmt for a file; run `cargo fmt --all` before committing.

## Lints

- Clippy is clean under `-D warnings`: a warning fails the lint check.
- The lints are set once, in `[workspace.lints]` of the root `Cargo.toml`, and every crate takes them with `[lints] workspace = true`.
- Suppress a lint narrowly, with `#[expect(lint, reason = "…")]` on the smallest item that needs it, never with a crate-wide `#[allow]`.

## Errors

- Libraries return typed errors: an error enum per module or crate, implementing `std::error::Error`.
- Binaries may use a catch-all error type at their top level.
- `unwrap` is used only in tests. `expect` is used only with a message that states the invariant that makes it safe.

## Unsafe code

- Unsafe code is linted at `{{ unsafe_code }}` for the whole workspace.
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
- `Cargo.lock` is committed for applications.

## Local checks

All three pass before pushing:

- `cargo fmt --all --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo test --workspace`
