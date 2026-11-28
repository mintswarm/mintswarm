---
id: "@mintswarm/mintswarm:repository/build-artefacts-under-build"
statement: "Build artefacts go only under the git-ignored build/ at the repository root, never next to sources."
strength: "must"
applies: { kind: ["plan", "code", "test"] }
---
## Rationale

One place to clean, one ignore rule, and no generated file mistaken for a source or picked up by a glob meant for sources.

## Implications

Every tool's output directory is configured to a path under `build/`, and a new tool is configured before it is used.

## Exceptions

- Dependency installs such as `node_modules/` are not build artefacts.

## Compliant

- Cargo writes to `build/target`, Vite to `build/web`.
- `git status --ignored` after the gates shows ignored files only under `build/` and dependency installs.

## Contradicting

- A `target/` directory at the root or inside a crate.
- A test that writes its fixtures' output next to the fixtures.
