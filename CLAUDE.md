# mintswarm

A factory that turns tickets into reviewed pull requests: a Rust control plane over Postgres, Rust runner daemons that drive coding-agent harnesses (Claude Code, Codex), and a React/TypeScript UI.

The repository is being initialised; code, design documents and contribution guidelines land in follow-up PRs.

## Conventions

- British spelling in code, comments, docs and commit messages.
- The default branch is `master`. Never commit or push to it directly; every change goes through a PR.
- Work branches are named `<type>/<issue>-<slug>` (for example `feat/12-runner-stream-client`).
- `mintswarm/*` branches are reserved for the factory itself. Never create, push to or rebase them.
- Commit messages and PR titles follow Conventional Commits; PRs are squash-merged, so the PR title is what lands on `master`.
- Stage files by name; never `git add -A` or `git add .`.
- Call the project "fair source", never "open source": it is licensed under FSL-1.1-ALv2 (see `LICENSE.md`).
