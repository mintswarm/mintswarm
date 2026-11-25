//! The mintswarm runner: the daemon that drives coding-agent harnesses on its own machine.
//!
//! It holds the git VCS adapter, `exec::git`.

/// What stays on the runner whoever owns the job loop.
pub mod exec {
    pub mod git;
}
