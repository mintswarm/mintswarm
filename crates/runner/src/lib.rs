//! The mintswarm runner: the daemon that drives coding-agent harnesses on its own machine.
//!
//! Today it holds the git VCS adapter, `exec::git`; the binary comes later.

/// What stays on the runner whoever owns the job loop.
pub mod exec {
    pub mod git;
}
