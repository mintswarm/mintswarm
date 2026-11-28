# mintswarm

A factory that turns tickets into reviewed pull requests: a Rust control plane over Postgres, Rust runner daemons that drive coding-agent harnesses (Claude Code, Codex), and a React/TypeScript UI.

These instructions are for any coding agent working in this repository, whatever its harness.

## Where things are

- **Design:** the Design issue [#3](https://github.com/mintswarm/mintswarm/issues/3) and its artifact "mintswarm design". Read the design from there; it is not in the repository.
- **Architecture:** `docs/architecture/` describes how the system is built, as it stands now.
- **Conventions:** [`docs/conventions/`](docs/conventions/) holds the Rust, TypeScript and React conventions the gates derive from, copied from the guideline packs.
- **Contribution rules:** `CONTRIBUTING.md`.
- The repository holds the current state only. Proposals, alternatives and reasoning live in issues and PRs, never in files here.

## Conventions

- British spelling in code, comments, docs and commit messages.
- The default branch is `master`, which is also this repository's work branch, the branch the factory integrates into (named in `.mintswarm/dogfood/project-setup.yaml`). Never commit or push to it directly; every change goes through a PR.
- Topic branches are named `<type>/<issue>-<slug>` (for example `feat/12-runner-stream-client`).
- `mintswarm/*` branches are the factory's own job branches, written only by mintswarm itself: its runner creates and pushes them, and its implement stage commits on its own job's branch. People never create, push to or rebase them.
- Commit messages and PR titles follow Conventional Commits; PRs are squash-merged, so the PR title is what lands on `master`.
- Stage files by name; never `git add -A` or `git add .`.
- Call the project "fair source", never "open source": it is licensed under FSL-1.1-ALv2 (see `LICENSE.md`).
- Build artefacts go only under the git-ignored `build/` at the repository root, never next to sources. Dependency installs such as `node_modules/` are not build artefacts.
