//! The project setup: its work branch must be a valid branch name.

use mintswarm_domain::setup::{ProjectSetup, SetupError};
use mintswarm_domain::vcs::BranchName;

#[test]
fn a_valid_work_branch_is_kept() {
    let setup = ProjectSetup::new(BranchName("develop".into())).unwrap();
    assert_eq!(setup.work_branch, BranchName("develop".into()));
}

#[test]
fn an_invalid_work_branch_is_refused() {
    assert_eq!(
        ProjectSetup::new(BranchName("a..b".into())),
        Err(SetupError::InvalidWorkBranch("a..b".into()))
    );
}
