---
id: "@mintswarm/mintswarm:web/es2023-target"
statement: "The code is ES modules compiled for ES2023."
strength: "must"
applies: { kind: ["code"], paths: ["web/**/tsconfig*.json"] }
---
## Rationale

One target for the whole package, matching the browsers and the Node version mintswarm supports.

## Compliant

-
  ```json
  { "compilerOptions": { "target": "ES2023", "module": "ESNext" } }
  ```

## Contradicting

- A package in `web/` targeting `ES2017`.
