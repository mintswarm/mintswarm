# Dogfooding tools

The scripts here run mintswarm's pipeline by hand while the Rust runner is being built. Each is a transient stand-in for a `mintswarm-runner tool <name>` command.

## Lockstep with Rust

While a tool exists both as a Python script here and as a Rust command, the two change together:

- A change to the tool's behaviour or wording changes the Python script and the Rust command in the same PR.
- Rust's wording is the reference. Where the two differ, the Python script takes the Rust text.
- Once the tool's gate lands, its self-test runs against either implementation. Unset, it runs the Python script; with `MSW_IMPL=rust` and `MSW_RUST_BIN=<path of mintswarm-runner>`, it runs `<path> tool <name>`. Any other `MSW_IMPL`, or a `MSW_RUST_BIN` that is not an executable file, exits 2.
- The gate is the self-test's gate name followed by `-rust` in `gates.run`, so one gate can serve two tools. It builds `mintswarm-runner` into `build/target` and runs the self-test with the switch on. Only tools whose gate is named in the table below have one.
- Under the switch, a case or wording that only the Python has is skipped, and the self-test prints the Rust test that mirrors it: `skip <check>: under Rust, <test> in <path> covers it`. The self-test fails unless that `#[test]` fn exists in that file.
- Once that gate passes, the session moves from the Python script to `mintswarm-runner tool <name>` (#410).

Ported tools:

| Tool | Python | Rust | Gate |
| --- | --- | --- | --- |
| `save-output` | `save-output.py` | `mintswarm-runner tool save-output` | `save-output-selftest-rust` |
| `msw-vcs` | `msw-vcs` | `mintswarm-runner tool msw-vcs` | `msw-vcs-selftest-rust` |
| `plan-floor` | `plan-approval/floor.py` | `mintswarm-runner tool plan-floor` | `plan-approval-selftest-rust` |
| `plan-decision` | `plan-approval/decision.py` | `mintswarm-runner tool plan-decision` | `plan-approval-selftest-rust` |
