---
id: "@mintswarm/mintswarm:web/version-holds"
statement: "typescript stays at 6.0 or lower until typescript-eslint supports a later release, and eslint at 9 until eslint-plugin-react supports 10."
strength: "must"
applies: { kind: ["plan", "code"], paths: ["web/package.json"] }
---
## Rationale

The linters reject compiler and ESLint versions they don't support yet, which would break the lint gate.

## Implications

Avoid compiler options deprecated in 6.0, such as `baseUrl` and `moduleResolution: "node"`. Lift a hold in the same change that upgrades past it.

## Compliant

- `typescript` at `6.0.3` and `eslint` at `9.39.5`.

## Contradicting

- `typescript` bumped to 6.1 while typescript-eslint supports only up to 6.0.
