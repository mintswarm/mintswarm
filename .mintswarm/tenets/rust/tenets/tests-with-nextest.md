---
id: "@mintswarm/mintswarm:rust/tests-with-nextest"
statement: "Tests run under cargo-nextest, with doctests run separately by cargo test --doc."
strength: "must"
applies: { kind: ["plan", "test"] }
check: { gate: "rust-test" }
---
## Rationale

nextest runs each test in its own process, so a test that leaks global state or panics in a thread can't affect another. It doesn't run doctests, so `cargo test --doc` covers those.

## Implications

Tests don't rely on running in the same process as other tests, or in a fixed order. cargo-nextest is installed at the version `.github/workflows/gates.yml` pins.

## Compliant

- `cargo nextest run --workspace && cargo test --doc --workspace` passes.

## Contradicting

- A test that passes only when run after another one, through a shared static.
