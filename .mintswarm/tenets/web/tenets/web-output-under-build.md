---
id: "@mintswarm/mintswarm:web/web-output-under-build"
statement: "The bundler's output, its cache and test output, coverage included, go under the repository's root build/."
strength: "must"
applies: { kind: ["code"], paths: ["web/**/vite.config.*", "web/**/vitest.config.*"] }
refines: "@mintswarm/mintswarm:repository/build-artefacts-under-build"
---
## Rationale

Web output stays under the one build root, never next to the sources.

## Compliant

-
  ```ts
  build: { outDir: "../../../build/web/ui", emptyOutDir: true },
  cacheDir: "../../../build/cache/vite",
  ```

## Contradicting

- Vite's default `dist/` inside `web/apps/ui`.
- Coverage written to `web/coverage/`.
