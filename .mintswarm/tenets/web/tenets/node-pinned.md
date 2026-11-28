---
id: "@mintswarm/mintswarm:web/node-pinned"
statement: "Node.js 24 is pinned in web/.node-version."
strength: "must"
applies: { kind: ["code"], paths: ["web/.node-version"] }
---
## Rationale

Every machine, runner and CI job runs the same Node, so a gate never passes in one place and fails in another.

## Compliant

- `web/.node-version` holds `24`.

## Contradicting

- No `.node-version`, so each machine uses whatever Node it has.
