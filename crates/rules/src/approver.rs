//! Who may approve: the tracker issue's owner, whether a sender counts as its approver, and
//! whether a pull request's approvals cover its CODEOWNERS approval groups (design B4, Q11).
//!
//! The owner here is the tracker issue's owner, as `assign-approvers.py`'s `issue_owner` finds
//! them: the issue's author when a person filed it, or else the author of the nearest
//! person-filed ancestor on its parent chain. It is not the owner of a work item in
//! [`mintswarm_domain::ownership`]: owning a work item gives no right to approve it.
//!
//! - [`OwnerWalk`]: the owner walk, one parent read at a time, so the caller reads lazily.
//! - [`issue_approver`]: whether a sender is the issue's approver: a person, the owner, and
//!   with write access.
//! - [`cover`]: whether the non-bot approvers with write access satisfy every approval group
//!   of a pull request's changed paths.
//!
//! Logins are compared without regard to ASCII case. Nothing here reads the tracker; the
//! caller supplies every read.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use mintswarm_domain::approval::{self, ApprovalGroup, Facts, Protection};
use mintswarm_domain::ids::IssueNumber;
use mintswarm_domain::tracker::{Login, ParentIssue, RepoRole, is_bot};

/// What the owner walk needs next, or the owner it found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OwnerStep {
    /// The owner: the person who filed issue `from`.
    Owner { owner: Login, from: IssueNumber },
    /// Read this issue's parent and pass it to [`OwnerWalk::parent`].
    ReadParent(IssueNumber),
}

/// Why the owner walk found no owner.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OwnerError {
    /// `issue` was filed by a bot, and no ancestor up its parent chain was filed by a person.
    NoPersonAncestor { issue: IssueNumber },
    /// The parent chain of `issue` comes back to `at`, which it has already passed.
    Loop { issue: IssueNumber, at: IssueNumber },
    /// The author of `from`, the owner found, is not a valid login.
    InvalidLogin { from: IssueNumber, login: Login },
}

impl fmt::Display for OwnerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoPersonAncestor { issue } => write!(
                f,
                "#{} was filed by a bot and no person-filed ancestor was found",
                issue.0
            ),
            Self::Loop { issue, at } => write!(
                f,
                "#{}: the parent chain loops at #{}; no person-filed ancestor was found",
                issue.0, at.0
            ),
            Self::InvalidLogin { from, login } => {
                write!(
                    f,
                    "#{}'s author is not a valid login: {:?}",
                    from.0, login.0
                )
            }
        }
    }
}

impl std::error::Error for OwnerError {}

/// The walk up an issue's parent chain to its owner, one parent read per step.
///
/// [`OwnerWalk::start`] takes the issue's own author. While it answers
/// [`OwnerStep::ReadParent`], the caller reads that issue's parent and passes it to
/// [`OwnerWalk::parent`], until an answer is the owner or an error. Once it has answered so,
/// it answers the same again.
#[derive(Clone, Debug)]
pub struct OwnerWalk {
    issue: IssueNumber,
    seen: BTreeSet<IssueNumber>,
    outcome: Result<OwnerStep, OwnerError>,
}

impl OwnerWalk {
    /// Starts the walk at `issue`, filed by `author`, a bot when `author_is_bot`; returns the
    /// walk and its first answer.
    pub fn start(
        issue: IssueNumber,
        author: &Login,
        author_is_bot: bool,
    ) -> (Self, Result<OwnerStep, OwnerError>) {
        let mut walk = Self {
            issue: issue.clone(),
            seen: BTreeSet::from([issue.clone()]),
            outcome: Ok(OwnerStep::ReadParent(issue.clone())),
        };
        walk.outcome = walk.at(issue, Some(author), author_is_bot);
        let first = walk.outcome.clone();
        (walk, first)
    }

    /// Takes the parent read that the last [`OwnerStep::ReadParent`] asked for: `None` when the
    /// issue has no parent.
    pub fn parent(&mut self, parent: Option<&ParentIssue>) -> Result<OwnerStep, OwnerError> {
        if !matches!(self.outcome, Ok(OwnerStep::ReadParent(_))) {
            return self.outcome.clone();
        }
        self.outcome = match parent {
            None => Err(OwnerError::NoPersonAncestor {
                issue: self.issue.clone(),
            }),
            Some(parent) if !self.seen.insert(parent.number.clone()) => Err(OwnerError::Loop {
                issue: self.issue.clone(),
                at: parent.number.clone(),
            }),
            Some(parent) => self.at(
                parent.number.clone(),
                parent.author.as_ref(),
                parent.author_is_bot,
            ),
        };
        self.outcome.clone()
    }

    /// The answer at `current`, filed by `author`.
    fn at(
        &self,
        current: IssueNumber,
        author: Option<&Login>,
        author_is_bot: bool,
    ) -> Result<OwnerStep, OwnerError> {
        match person(author, author_is_bot) {
            None => Ok(OwnerStep::ReadParent(current)),
            Some(login) if is_login(&login.0) => Ok(OwnerStep::Owner {
                owner: login.clone(),
                from: current,
            }),
            Some(login) => Err(OwnerError::InvalidLogin {
                from: current,
                login: login.clone(),
            }),
        }
    }
}

/// The author when a person filed the issue: named, not flagged as a bot, and with no
/// `[bot]` login, as `assign-approvers.py`'s `person()` decides.
fn person(author: Option<&Login>, author_is_bot: bool) -> Option<&Login> {
    author.filter(|login| !author_is_bot && !is_bot(&login.0, None))
}

/// Whether `login` is a valid owner login: `^[A-Za-z0-9-]+$`.
fn is_login(login: &str) -> bool {
    !login.is_empty() && login.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

/// Why a sender is not the issue's approver.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NotApprover {
    /// The sender is a bot.
    Bot,
    /// The sender is not the issue's owner.
    NotOwner,
    /// The sender is the owner but has this role, which cannot write to the repository.
    NoWriteAccess(RepoRole),
}

impl fmt::Display for NotApprover {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Bot => f.write_str("the sender is a bot"),
            Self::NotOwner => f.write_str("the sender is not the issue's owner"),
            Self::NoWriteAccess(role) => {
                write!(f, "the sender has no write access (role {role:?})")
            }
        }
    }
}

impl std::error::Error for NotApprover {}

/// Whether `sender`, a bot when `sender_is_bot`, with `role` on the repository, is the approver
/// of an issue whose owner is `owner`.
///
/// A bot never is, whatever its role: one flagged so, or with a `[bot]` login. Then the
/// sender must be the owner, compared without regard to ASCII case, and must have write,
/// maintain or admin access.
pub fn issue_approver(
    sender: &Login,
    sender_is_bot: bool,
    owner: &Login,
    role: RepoRole,
) -> Result<(), NotApprover> {
    if sender_is_bot || is_bot(&sender.0, None) {
        return Err(NotApprover::Bot);
    }
    if !sender.0.eq_ignore_ascii_case(&owner.0) {
        return Err(NotApprover::NotOwner);
    }
    if !role.can_write() {
        return Err(NotApprover::NoWriteAccess(role));
    }
    Ok(())
}

/// A login that approved a pull request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Approver {
    pub login: Login,
    /// Whether the login is a bot, as the review read flags it.
    pub is_bot: bool,
}

/// How far a pull request's approvals cover its approval groups.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cover {
    /// The indices of the groups an approver satisfies, in group order.
    pub satisfied: Vec<usize>,
    /// The indices of the groups still waiting for an approver, in group order.
    pub open: Vec<usize>,
    /// Whether there is at least one group and every group is satisfied.
    pub covered: bool,
}

/// Whether `approvers` cover every one of `groups`, with each login's role from `roles`.
///
/// Bots are dropped first, flagged or with a `[bot]` login, and a login missing from `roles`
/// has [`RepoRole::NoAccess`]. The rest go through [`approval::evaluate`] with no team members
/// and the default protection, so only the groups decide: a group owned only by teams or
/// e-mail owners stays open, and the unowned group takes any approver with write access.
/// No groups, as for a pull request that changes no path, is never covered.
pub fn cover(
    groups: &[ApprovalGroup],
    approvers: &[Approver],
    roles: &BTreeMap<Login, RepoRole>,
) -> Cover {
    let people: Vec<Login> = approvers
        .iter()
        .filter(|approver| !approver.is_bot && !is_bot(&approver.login.0, None))
        .map(|approver| approver.login.clone())
        .collect();
    let teams = BTreeMap::new();
    let has_write = |login: &Login| {
        roles
            .get(login)
            .copied()
            .unwrap_or(RepoRole::NoAccess)
            .can_write()
    };
    let facts = Facts {
        teams: &teams,
        has_write: &has_write,
    };
    let status = approval::evaluate(groups, &people, &facts, &Protection::default());
    Cover {
        covered: !groups.is_empty() && status.open.is_empty(),
        satisfied: status.satisfied,
        open: status.open,
    }
}
