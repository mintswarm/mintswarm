---
id: "@mintswarm/mintswarm:web/exact-pinned-dev-tools"
statement: "typescript, eslint, @eslint/js, typescript-eslint and prettier are exact-pinned devDependencies of web/, and the gates run them from web/node_modules/.bin."
strength: "must"
applies: { kind: ["code"], paths: ["web/package.json"] }
---
## Rationale

A tool upgrade then happens in one visible change, never through a range resolving to a new release.

## Compliant

-
  ```json
  "devDependencies": { "typescript": "6.0.3", "prettier": "3.10.1" }
  ```

## Contradicting

-
  ```json
  "devDependencies": { "typescript": "^6.0.0" }
  ```
