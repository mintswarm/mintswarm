//! The reads behind who approves (design B4, Q11): the tracker issue's owner, whether a sender
//! is its approver, and whether a pull request's approvals cover its CODEOWNERS approval groups.
//!
//! The owner is the tracker issue's owner, as [`mintswarm_rules::approver`] defines it, not the
//! owner of a work item: owning a work item gives no right to approve it. This module reads
//! parents, roles, the CODEOWNERS file and the changed paths, and decides nothing itself.

use std::collections::BTreeMap;

use mintswarm_domain::approval::{self, ApprovalGroup};
use mintswarm_domain::codeowners::{self, LineError};
use mintswarm_domain::ids::{PrNumber, RevId};
use mintswarm_domain::tracker::{CodeOwners, Collaborators, Issue, Login, is_bot};
use mintswarm_rules::approver::{
    self as rule, Approver, Cover, NotApprover, OwnerError, OwnerStep, OwnerWalk,
};

/// Why the owner of an issue could not be found.
#[derive(Debug, thiserror::Error)]
pub enum ApproverError<E: std::error::Error + 'static> {
    /// A tracker read failed.
    #[error(transparent)]
    Tracker(E),
    /// The parent chain gives no owner.
    #[error(transparent)]
    Owner(OwnerError),
}

/// The owner of `issue`, as the caller read it: its author when a person filed it, or else the
/// author of the nearest person-filed ancestor, read one parent at a time.
pub async fn issue_owner<T: Collaborators>(
    tracker: &T,
    issue: &Issue,
) -> Result<Login, ApproverError<T::Error>> {
    let (mut walk, mut step) =
        OwnerWalk::start(issue.number.clone(), &issue.author, issue.author_is_bot);
    loop {
        match step.map_err(ApproverError::Owner)? {
            OwnerStep::Owner { owner, .. } => return Ok(owner),
            OwnerStep::ReadParent(number) => {
                let parent = tracker
                    .read_parent(number)
                    .await
                    .map_err(ApproverError::Tracker)?;
                step = walk.parent(parent.as_ref());
            }
        }
    }
}

/// Whether `sender`, a bot when `sender_is_bot`, is the approver of `issue`: the inner result
/// is the rule's answer, the outer one whether the reads it needs succeeded.
///
/// It reads the owner, then the sender's role with the login as given.
pub async fn issue_approver<T: Collaborators>(
    tracker: &T,
    issue: &Issue,
    sender: &Login,
    sender_is_bot: bool,
) -> Result<Result<(), NotApprover>, ApproverError<T::Error>> {
    let owner = issue_owner(tracker, issue).await?;
    let role = tracker
        .read_permission(sender.clone())
        .await
        .map_err(ApproverError::Tracker)?;
    Ok(rule::issue_approver(sender, sender_is_bot, &owner, role))
}

/// A pull request's approval groups at a base, and how far its approvals cover them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrCover {
    /// The approval groups of the changed paths, in [`approval::groups`]' order.
    pub groups: Vec<ApprovalGroup>,
    /// How far the approvals cover them, by index into `groups`.
    pub cover: Cover,
    /// The CODEOWNERS lines left out as invalid, which own nothing.
    pub line_errors: Vec<LineError>,
}

/// Whether `approvers` cover the approval groups of `pr`'s changed paths under the CODEOWNERS
/// file at `base`, a commit the caller pins.
///
/// No CODEOWNERS file means no rules, so every path is in the unowned group. Each path a change
/// touches counts, a rename's previous one too. Each distinct non-bot approver's role is read
/// once, with the login as given.
pub async fn pr_cover<T: CodeOwners + Collaborators>(
    tracker: &T,
    pr: PrNumber,
    base: RevId,
    approvers: &[Approver],
) -> Result<PrCover, T::Error> {
    let parsed = tracker
        .read_codeowners(base)
        .await?
        .map(|file| codeowners::parse(&file.text))
        .unwrap_or_default();
    let files = tracker.read_changed_files(pr).await?;
    let groups = approval::groups(&parsed.rules, files.iter().flat_map(|file| file.paths()));
    let mut roles = BTreeMap::new();
    for approver in approvers {
        if approver.is_bot || is_bot(&approver.login.0, None) || roles.contains_key(&approver.login)
        {
            continue;
        }
        let role = tracker.read_permission(approver.login.clone()).await?;
        roles.insert(approver.login.clone(), role);
    }
    let cover = rule::cover(&groups, approvers, &roles);
    Ok(PrCover {
        groups,
        cover,
        line_errors: parsed.errors,
    })
}
