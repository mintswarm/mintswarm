//! The mintswarm runner: the daemon that drives coding-agent harnesses on its own machine.
//!
//! It holds the git VCS adapter, `exec::git`, and the runner's local state, `exec::store`: the
//! locked state directory, the SQLite database, the blob store, the outbox and leases.

/// What stays on the runner whoever owns the job loop.
pub mod exec {
    pub mod git;
    pub mod store;
}
