<!-- mintswarm-pack: typescript 2 -->
# TypeScript conventions

How TypeScript is written, checked and formatted in the package at `web`.

## Toolchain

- Node.js 24, pinned in `.node-version`.
- `typescript`, `eslint`, `@eslint/js`, `typescript-eslint` and `prettier` are exact-pinned devDependencies of the package at `web`, so the gates run its own binaries from `node_modules/.bin`, never a global install.
- `typescript` stays at 6.0 or lower until `typescript-eslint` supports a later release; avoid compiler options deprecated in 6.0, such as `baseUrl` and `moduleResolution: "node"`.
- The code is ES modules, compiled for `ES2023`; `tsc` only type-checks (`noEmit`), and a bundler or a separate build configuration emits. The bundler's output and test output, including coverage, go under the repository's root `build/`, which is git-ignored, never next to sources; `node_modules/` holds dependency installs and is not build output.

## Types

- `tsconfig.json` is strict, including `noUncheckedIndexedAccess` and `exactOptionalPropertyTypes`; never relax it per file.
- No `any`. Take outside data as `unknown` and narrow it with type guards or a schema.
- No non-null assertions (`x!`); narrow instead.
- Prefer `readonly` properties and `readonly T[]` arrays (`ReadonlyMap`, `ReadonlySet` where relevant) for data that is not mutated.
- Use unions of string literals, not `enum`.
- Make a `switch` over a union exhaustive with a `default` that assigns the value to `never` and uses it: `` const unreachable: never = value; throw new Error(`Unhandled: ${String(unreachable)}`); ``.
- `catch (error: unknown)`, and narrow before use; throw only `Error` objects.

## Modules

- Import types with `import type` (enforced by `verbatimModuleSyntax`).
- Named exports, no default exports, except where a tool requires one.
- No namespaces and no `require`.

## Naming

- `camelCase` for variables and functions, `PascalCase` for types and classes, `UPPER_SNAKE_CASE` only for constants fixed at build time.
- File names are `kebab-case`.

## Formatting and lint

- Prettier owns formatting, at 100 columns; never format by hand. `.prettierignore` lists lockfiles and build output; its local `dist/` and `coverage/` entries are only a fallback, because output belongs under the root `build/`.
- When the package sits at the repository root, its copies of the ESLint config and `tsconfig.json` add `build/` to the ESLint global ignores and to `exclude`, because ESLint does not read `.gitignore`.
- ESLint runs `@eslint/js` recommended and typescript-eslint's `strictTypeChecked` and `stylisticTypeChecked` sets with type information.
- Suppress a lint only on its line, with `// eslint-disable-next-line <rule> -- <reason>`.

## Local checks

Run these from the repository root before pushing; all three pass:

- `cd web && ./node_modules/.bin/tsc --noEmit`
- `cd web && ./node_modules/.bin/eslint .`
- `cd web && ./node_modules/.bin/prettier --check .`
