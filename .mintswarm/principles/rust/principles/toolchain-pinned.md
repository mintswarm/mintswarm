---
id: "@mintswarm/mintswarm:rust/toolchain-pinned"
statement: "The toolchain is pinned to a release in rust-toolchain.toml, with the rustfmt and clippy components."
strength: "must"
applies: { kind: ["code"], paths: ["rust-toolchain.toml"] }
---
## Rationale

Every machine, runner and CI job builds with the same compiler, formatter and linter, so a gate never passes in one place and fails in another.

## Implications

Moving the toolchain is a change of its own, with its lint and format fallout fixed in the same PR.

## Compliant

- ```toml path=rust-toolchain.toml
  [toolchain]
  channel = "1.89"
  components = ["rustfmt", "clippy"]
  ```

## Contradicting

- No `rust-toolchain.toml`.
- `channel = "stable"`: the build changes whenever a release comes out.
