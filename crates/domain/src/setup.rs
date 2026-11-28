//! A project's setup: the choices made when a repository is set up for the factory, which no
//! party discovers for itself.
//!
//! The **work branch** is the branch the factory integrates into: its config is read at the work
//! branch's head, work is based on it and lands into it. It defaults to the repository's default
//! branch at setup time, but may be any branch the owner's flow needs. It is not a key of
//! `.mintswarm/config.yaml`, which itself lives on the work branch.

use std::fmt;

use crate::hosting::{RepoReadError, RepoReads};
use crate::vcs::BranchName;

/// A project's setup.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectSetup {
    /// The branch the factory integrates into.
    pub work_branch: BranchName,
}

/// Why a setup value is refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SetupError {
    /// The work branch is not a valid branch name, as [`BranchName::is_valid`] reads it.
    InvalidWorkBranch(String),
}

impl fmt::Display for SetupError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidWorkBranch(name) => {
                write!(f, "the work branch {name:?} is not a valid branch name")
            }
        }
    }
}

impl std::error::Error for SetupError {}

impl ProjectSetup {
    /// A setup with `work_branch`, which must be a valid branch name.
    pub fn new(work_branch: BranchName) -> Result<Self, SetupError> {
        if !BranchName::is_valid(&work_branch.0) {
            return Err(SetupError::InvalidWorkBranch(work_branch.0));
        }
        Ok(Self { work_branch })
    }

    /// The setup-time default: the repository's default branch as the work branch.
    pub async fn from_default_branch(reads: &impl RepoReads) -> Result<Self, RepoReadError> {
        let work_branch = reads.default_branch().await?;
        Self::new(work_branch).map_err(|_| RepoReadError::Unexpected)
    }
}
