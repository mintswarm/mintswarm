---
id: "@mintswarm/mintswarm:web/identifier-case"
statement: "camelCase for variables and functions, PascalCase for types, classes and components, UPPER_SNAKE_CASE only for constants fixed at build time."
strength: "must"
applies: { kind: ["code", "test"] }
---
## Rationale

One naming style, so a name says what kind of thing it is.

## Compliant

-
  ```ts
  const pollInterval = 5_000;
  type JobState = "idle" | "running";
  const API_BASE = import.meta.env.VITE_API_BASE;
  ```

## Contradicting

-
  ```ts
  const PollInterval = 5_000;
  const MAX_ROWS = rows.length;
  ```
