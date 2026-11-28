# Dogfooding tools

The scripts here run mintswarm's pipeline by hand while the Rust runner is being built. Each is a transient stand-in for a `mintswarm-runner tool <name>` command.

## Project setup

`project-setup.yaml` stands in for the setting the plane stores with a project's setup: the factory's branches, starting with `branches.work`, the work branch the factory integrates into (`master` here). It is read from the checkout, never at a ref, and is not part of `.mintswarm/config.yaml`, which itself lives on the work branch.

- `project-setup.py work-branch` is its one reader outside Rust. The `whitespace` and `packs-check` gates call it, `github/common.sh` wraps it as `msw_work_branch` for `push.sh`, and the Python tools in `github/` load it by path for their `--base` defaults and `land.py`'s base.
- `load_project_setup` in `crates/config` is its schema: `branches` and `branches.work` are required, any other key is refused, and the name must be a valid branch name (`git check-ref-format refs/heads/<name>` accepts it, it does not start with `-` and it is not `HEAD`).
- Both readers run the case table `crates/config/tests/conformance/project-setup.json`: the Rust one in `crates/config/tests/setup.rs`, the Python one in `github/selftest.sh`.

## Lockstep with Rust

While a tool exists both as a Python script here and as a Rust command, the two change together:

- A change to the tool's behaviour or wording changes the Python script and the Rust command in the same PR.
- Rust's wording is the reference. Where the two differ, the Python script takes the Rust text.
- Once the tool's gate lands, its self-test runs against either implementation. Unset, it runs the Python script; with `MSW_IMPL=rust` and `MSW_RUST_BIN=<path of mintswarm-runner>`, it runs `<path> tool <name>`. Any other `MSW_IMPL`, or a `MSW_RUST_BIN` that is not an executable file, exits 2.
- The gate is the self-test's gate name followed by `-rust` in `gates.run`, so one gate can serve two tools. It builds `mintswarm-runner` into `build/target` and runs the self-test with the switch on. Only tools whose gate is named in the table below have one.
- Under the switch, a case or wording that only the Python has is skipped, and the self-test prints the Rust test that mirrors it: `skip <check>: under Rust, <test> in <path> covers it`, or `skip <check>: under Rust, <test> and <test> in <path> cover it` when two tests in one file mirror it. The self-test fails unless each named `#[test]` fn exists in that file.
- Once that gate passes, the session moves from the Python script to `mintswarm-runner tool <name>` (#410).

Ported tools:

| Tool | Python | Rust | Gate |
| --- | --- | --- | --- |
| `save-output` | `save-output.py` | `mintswarm-runner tool save-output` | `save-output-selftest-rust` |
| `msw-vcs` | `msw-vcs` | `mintswarm-runner tool msw-vcs` | `msw-vcs-selftest-rust` |
| `plan-floor` | `plan-approval/floor.py` | `mintswarm-runner tool plan-floor` | `plan-approval-selftest-rust` |
| `plan-decision` | `plan-approval/decision.py` | `mintswarm-runner tool plan-decision` | `plan-approval-selftest-rust` |
