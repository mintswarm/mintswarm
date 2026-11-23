# React conventions

How React is written and checked in the TypeScript package at `{{ package_root }}`.

## Toolchain

- `react` is a dependency and `@types/react` a devDependency.
- `eslint-plugin-react` and `eslint-plugin-react-hooks` join the TypeScript pack's devDependencies.
- `eslint` stays at 9 until `eslint-plugin-react` supports 10.
- JSX compiles with `react-jsx`, so the setup session sets the TypeScript pack's `jsx` parameter to `react-jsx`.

## Components

- Function components only; no class components.
- Components are named in `PascalCase`, as JSX and the rules of hooks require; the TypeScript pack's naming rules still cover other functions and file names.
- Each component's props are a named `<Component>Props` type.
- Children come through props, typed in that props type.
- Never define a component inside another component; it remounts on every render.
- JSX holds no logic beyond conditionals and mapping; compute values before the `return`.

## Hooks

- Follow the rules of hooks: call them only at the top level of a component or a custom hook, never in conditions, loops or callbacks.
- Custom hooks are named `use…`.
- Effects only synchronise with external systems, and each cleans up what it sets up.
- Derive values during render instead of mirroring them in state through effects.
- Dependency arrays are complete and never silenced.

## State

- Keep state minimal, and lift it to the nearest common owner of the components that use it.
- Updates replace state and never mutate it; an update that depends on the previous value uses the updater form.
- List `key`s are stable values from the data, never indices.

## Lint additions

- `eslint.extra.config.mjs` adds `eslint-plugin-react`'s `recommended` and `jsx-runtime` presets and `eslint-plugin-react-hooks`'s `recommended-latest` preset.
- `react/prop-types` is off, because types cover props; `react-hooks/exhaustive-deps` is an error.
- These rules run through the TypeScript pack's lint check.

## Tests

- Test behaviour through the rendered output and user events.
- Query elements by role and accessible name.
- Never test implementation details or component internals, such as state, hooks or instance methods.
