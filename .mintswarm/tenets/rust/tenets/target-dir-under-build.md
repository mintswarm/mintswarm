---
id: "@mintswarm/mintswarm:rust/target-dir-under-build"
statement: "Cargo writes build output to build/target, set by target-dir in [build] of .cargo/config.toml."
strength: "must"
applies: { kind: ["code"], paths: [".cargo/config.toml"] }
refines: "@mintswarm/mintswarm:repository/build-artefacts-under-build"
---
## Rationale

Cargo's default `target/` sits next to the sources; `build/target` keeps Rust output under the one build root.

## Compliant

- ```toml path=.cargo/config.toml
  [build]
  target-dir = "build/target"
  ```

## Contradicting

- No `target-dir`, so output goes to `target/`.
