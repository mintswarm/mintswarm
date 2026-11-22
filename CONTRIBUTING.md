# Contributing to mintswarm

mintswarm is fair source under FSL-1.1-ALv2. Contributions need a signed CLA so they can also flow into the commercial edition.

## Branching model

Trunk-based: `master` is the only long-lived branch, always builds, always passes the gates, and is releasable at any commit.

| Branch | Purpose | Lifetime |
| --- | --- | --- |
| `master` | trunk; every change lands here through a PR | permanent |
| `<type>/<issue>-<slug>` | one change by a person | days, deleted on merge |
| `mintswarm/<id>-<slug>` | **reserved** for the factory's own work branches | managed by mintswarm |
| `release/<major>.<minor>` | patch line for an already-released version, cut from its tag only when a fix must ship without what is on `master` | until that line is unsupported |

### Work branches

Name them `<type>/<issue>-<slug>`, lower case, hyphens:

```
feat/12-runner-stream-client
fix/31-lease-renewal-race
docs/7-branching-guidelines
exp/3-claude-headless-interrupt
```

`<type>` is one of `feat`, `fix`, `refactor`, `perf`, `test`, `docs`, `build`, `ci`, `chore`, `exp`. `<issue>` is the GitHub issue number; omit it only for trivial changes (`chore/bump-tokio`).

- Keep branches short-lived: aim to merge within a few days. Large work lands in steps behind a disabled code path or config flag, not on a long-running branch.
- Rebase on `master` to pick up changes; do not merge `master` into your branch.
- Force-push your own work branch freely (`--force-with-lease`); never anyone else's.
- `exp/*` branches hold spikes. They never merge: the findings go to the spike's issue, and what they settle goes into `docs/architecture/` through a normal PR.

### Reserved prefix

`mintswarm/*` belongs to the factory: see the Design issue [#3](https://github.com/mintswarm/mintswarm/issues/3) and its artifact "mintswarm design", and `.mintswarm/stages/implement.md`. Only mintswarm writes these branches: its runner creates and pushes them, and its implement stage commits on its own job's branch. People never create, push to or rebase them. The `mintswarm-factory-branches` ruleset enforces this: only the GitHub App that mintswarm pushes with — in this repository `mintswarm-dev` (see `.mintswarm/dogfood/github/`) — may create, update or delete them.

### Stacked changes

When one change depends on another unmerged one, base the second PR on the first branch and say so in its description. After the first merges, rebase the second onto `master` and retarget it.

## Commits

[Conventional Commits](https://www.conventionalcommits.org/), with the component as scope:

```
feat(runner): reconnect and replay job events after stream loss
fix(core): renew leases on heartbeat, not on claim
docs(docs): describe the repository layout
```

Scopes are the project's components, defined under `components` in `.mintswarm/config.yaml`: CI workflows and repository tooling use `tooling`, documents use `docs`. A change that also touches the `shared` paths listed in that file takes the scope of the component it is made for; a change confined to `shared` paths has no scope (`chore: bump tokio`). A breaking change to a contract (proto, migration semantics, config schema, job MCP tools) carries `!` and a `BREAKING CHANGE:` footer.

Inside a work branch commits can be as small as you like; the PR title is what lands on `master`.

## Pull requests

- Every change reaches `master` through a PR; no direct pushes, including from maintainers.
- **Squash merge only.** The PR title becomes the commit subject, so it must be a valid Conventional Commit; the `pr-title` check will verify it once CI is added. The changelog is generated from these subjects.
- One logical change per PR. A refactor that a feature needs goes in its own PR first.
- Link the issue (`Closes #12`). Describe what changed, why, and how it was verified.
- Required before merge: one approving review, including a Code Owner's, and all conversations resolved. Once CI is added, all checks green and the branch up to date with `master` as well.
- A change to a contract (anything under `api/`, `crates/core/migrations/`, or the job MCP tool list) states its compatibility impact in the description.
- The repository describes the current state. A change that alters how the system works updates `docs/architecture/` in the same PR; the reasoning goes in the PR description or the issue, never in a file of its own.

## `master` protection

The `master` ruleset requires:

- a pull request with one approving review, including review from Code Owners (`.github/CODEOWNERS`); stale approvals are dismissed on push
- all review conversations resolved
- squash merge as the only allowed method, and linear history
- no force-push and no deletion

The repository Admin role may bypass the ruleset for pull requests only; nothing else bypasses it, and no one pushes to `master` directly. The maintainer's own PRs, whose only code owner is their author, merge through this bypass without an approving review, and GitHub logs each use of it.

The repository settings, outside the ruleset, allow squash merge only and delete head branches after merge.

Waiting for CI, not required yet: the status checks `fmt`, `clippy`, `test`, `deny`, `proto` (buf lint + breaking), `generated` (OpenAPI and JSON Schema drift), `web` (UI lint, typecheck, test) and `pr-title`, and the branch being up to date with `master`.

## Releases

- One version for the whole workspace, SemVer; `0.y.z` until the protocol and config are stable (a minor bump may break).
- A release is an annotated tag `vX.Y.Z` on a commit of `master`. Pushing the tag runs cargo-dist and the image build; signing happens only in the protected `release` environment.
- Patch for an older line: create `release/X.Y` from the `vX.Y.Z` tag, cherry-pick the fix from `master` (`git cherry-pick -x`), tag `vX.Y.(Z+1)` on that branch. Fixes always land on `master` first.
- Pre-releases: `vX.Y.Z-rc.N`.

## Local checks

Before pushing, run what CI runs:

```
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo xtask schemas --check
```
