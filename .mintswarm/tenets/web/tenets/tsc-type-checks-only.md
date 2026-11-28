---
id: "@mintswarm/mintswarm:web/tsc-type-checks-only"
statement: "tsc only type-checks (noEmit); a bundler or a separate build configuration emits."
strength: "must"
applies: { kind: ["design", "code"], paths: ["web/**/tsconfig*.json"] }
---
## Rationale

One tool emits, so the output is never produced twice in two shapes.

## Compliant

-
  ```json
  { "compilerOptions": { "noEmit": true } }
  ```

## Contradicting

- `tsc` writing `.js` next to the sources.
