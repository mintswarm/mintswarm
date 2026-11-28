---
id: "@mintswarm/mintswarm:web/kebab-case-files"
statement: "File names are kebab-case."
strength: "must"
applies: { kind: ["code", "test"] }
---
## Rationale

One file-name style across the package, safe on case-insensitive file systems.

## Exceptions

- Files a tool names itself, such as `README.md`.

## Compliant

- `job-list.tsx`, `use-job-status.ts`.

## Contradicting

- `JobList.tsx`, `useJobStatus.ts`.
