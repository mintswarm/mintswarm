---
id: "@mintswarm/mintswarm:web/prettier-ignore-lists"
statement: "web/.prettierignore lists the lockfiles and build output."
strength: "must"
applies: { kind: ["code"], paths: ["web/.prettierignore"] }
---
## Rationale

Prettier doesn't read `.gitignore`, so without the list it would reformat lockfiles and generated output.

## Implications

Its `dist/` and `coverage/` entries are a fallback only; output belongs under the root `build/`.

## Compliant

- `pnpm-lock.yaml`, `package-lock.json`, `yarn.lock`, `dist/` and `coverage/` listed.

## Contradicting

- A `.prettierignore` without the lockfiles, so `prettier --check .` fails on them.
