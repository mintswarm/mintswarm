//! The mintswarm runner: the daemon that drives coding-agent harnesses on its own machine.
//!
//! It holds the git VCS adapter, `exec::git`, the runner's local state, `exec::store` (the
//! locked state directory, the SQLite database, the blob store, the outbox and leases), the stage
//! definitions rendered from the trusted store, `harness::render`, and the ported dogfood tools
//! behind `mintswarm-runner tool <name>`: [`tool::save_output`], the port of `save-output.py`.

/// What stays on the runner whoever owns the job loop.
pub mod exec {
    pub mod git;
    pub mod store;
}

pub mod harness;

/// The ported dogfood tools, each with the command line of the Python script it replaces.
pub mod tool;
