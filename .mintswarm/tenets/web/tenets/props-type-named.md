---
id: "@mintswarm/mintswarm:web/props-type-named"
statement: "Each component's props are a named <Component>Props type, which also types its children."
strength: "must"
applies: { kind: ["code"], paths: ["web/**/*.tsx"] }
---
## Rationale

Props read the same in every component, and a component's contract is one named type.

## Compliant

-
  ```tsx
  type JobListProps = { readonly jobs: readonly Job[]; readonly children?: ReactNode };
  export function JobList({ jobs, children }: JobListProps) {
  ```

## Contradicting

-
  ```tsx
  export function JobList({ jobs }: { jobs: Job[] }) {
  ```
