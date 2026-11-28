//! Whether a factory pull request is ready to merge, design §8: every guard `land.py` makes,
//! the linked issue, re-requested reviewers, and the approvals' cover of the CODEOWNERS
//! approval groups.
//!
//! [`evaluate`] is pure: the caller reads every fact from the tracker and passes it in
//! [`Facts`]; nothing here reads, posts or merges. The [`Outcome`] carries every failing guard
//! as a [`Reason`] class, each at most once and in guard order, so a reply keyed on the reason
//! set does not depend on how the guards are written. The approvals are collected by the
//! private `approvals` module, which states when an approval stands.

use std::collections::BTreeMap;

use mintswarm_config::{AWAITING_MERGE, ActionTrust, Trust};
use mintswarm_domain::approval::ApprovalGroup;
use mintswarm_domain::ids::{IssueNumber, RevId};
use mintswarm_domain::tracker::{
    CheckRun, CheckSuite, CombinedStatus, Comment, Issue, Login, PullDetail, RepoRole,
    RequestedReviewers, Review, ReviewThread,
};
use mintswarm_domain::vcs::BranchName;
use mintswarm_rules::approver::Cover;

mod approvals;

/// The check run whose suite holds the gate runs: the gates workflow's matrix job.
pub const GATES_MATRIX: &str = "gates-matrix";
/// The App that must have made the gates-matrix run and every gate run of its suite.
pub const ACTIONS_APP: &str = "github-actions";
/// The prefix of every factory branch.
pub const BRANCH_PREFIX: &str = "mintswarm/";
/// The label that holds a pull request back from the merge.
pub const HOLD_LABEL: &str = "hold";
/// The most commit statuses one page shows; more cannot all be checked.
pub const STATUS_PAGE: u64 = 100;

/// The issue a pull request's branch links, as the caller read it.
#[derive(Clone, Copy, Debug)]
pub struct LinkedIssue<'a> {
    /// The issue.
    pub issue: &'a Issue,
    /// Its `msw-state` value; `None` when it has none.
    pub state: Option<&'a str>,
}

/// What [`evaluate`] reads of one pull request and its surroundings.
#[derive(Clone, Copy, Debug)]
pub struct Facts<'a> {
    /// The repository the plane serves, in `owner/name` form.
    pub repo: &'a str,
    /// The branch factory pull requests merge into.
    pub base: &'a BranchName,
    /// The pull request's own fields.
    pub pull: &'a PullDetail,
    /// The issue its branch names (see [`linked_issue`]); `None` when it has none or it could
    /// not be found.
    pub linked: Option<LinkedIssue<'a>>,
    /// `tracker.ingest`, which the linked issue must pass.
    pub ingest: &'a Trust,
    /// The `approve_pr` row of `tracker.actions`.
    pub approve_pr: &'a ActionTrust,
    /// Every review of the pull request, in host order.
    pub reviews: &'a [Review],
    /// The pull request's conversation comments, read only for the Approve hint.
    pub comments: &'a [Comment],
    /// Every review thread of the pull request.
    ///
    /// The caller must treat a failed read as an error and never pass an empty list for it:
    /// an empty list passes the thread guard.
    pub threads: &'a [ReviewThread],
    /// The people and teams a review is requested from.
    ///
    /// The caller must treat a failed read as an error and never pass an empty value for it:
    /// an empty value passes the re-request guard.
    pub requested: &'a RequestedReviewers,
    /// The commit the check runs, suites and statuses were read at.
    pub checks_at: &'a RevId,
    /// The latest check runs on `checks_at`.
    pub runs: &'a [CheckRun],
    /// The check suites on `checks_at`.
    pub suites: &'a [CheckSuite],
    /// The commit statuses on `checks_at`.
    ///
    /// The caller must treat a failed read as an error and never pass an empty value for it:
    /// an empty value passes the status guard.
    pub statuses: &'a CombinedStatus,
    /// The approval groups of the pull request's changed paths at the base.
    pub groups: &'a [ApprovalGroup],
    /// The role of each approver on the repository, keyed by the login as the host spells it.
    pub roles: &'a BTreeMap<Login, RepoRole>,
    /// The approved commits below the head whose interdiff against the head is empty; a
    /// commit that is not listed counts as changed.
    pub same_shape: &'a [RevId],
}

/// Why a pull request is not ready to merge, in guard order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Reason {
    /// The branch names no issue, no issue was supplied, or the supplied one is another.
    NoLinkedIssue,
    /// The linked issue is not trusted by `tracker.ingest`.
    IssueUntrusted,
    /// The linked issue's `msw-state` is not `awaiting merge`.
    IssueNotAwaitingMerge,
    /// The pull request is not open.
    NotOpen,
    /// The pull request is a draft.
    Draft,
    /// The pull request targets another base.
    WrongBase,
    /// The head branch is not a `mintswarm/*` branch.
    NotFactoryBranch,
    /// The head branch is not in this repository.
    ForkHead,
    /// The pull request carries the `hold` label.
    Hold,
    /// The host has not worked out whether the pull request is mergeable.
    MergeabilityUnknown,
    /// The host reports the pull request not mergeable.
    NotMergeable,
    /// The pull request conflicts with its base.
    Conflict,
    /// The checks were read at a commit other than the head.
    ChecksNotAtHead,
    /// No gates-matrix check run by `github-actions`.
    NoGatesMatrix,
    /// The gates-matrix run's check suite is not among the commit's suites.
    SuiteMissing,
    /// The gates-matrix suite holds no gate run yet: the gates have not run.
    GatesNotRun,
    /// A run in the gates-matrix suite was made by another App.
    ForeignGateRun,
    /// A check suite with runs has not completed with `success`.
    SuiteNotSucceeded,
    /// A check run has not completed with `success`.
    RunNotSucceeded,
    /// There are more statuses than one page shows.
    TooManyStatuses,
    /// A commit status is not `success`.
    StatusNotSucceeded,
    /// A review thread is unresolved.
    UnresolvedThreads,
    /// A review was requested again from someone who has reviewed.
    ReviewRerequested,
    /// The `approve_pr` row trusts no people with CODEOWNERS, so no approval counts.
    ApprovalsNotTrusted,
    /// The approvals that count do not cover every approval group.
    NotCovered,
    /// An approval was not shown to keep the approved shape (changed, or not yet compared).
    ShapeChanged,
}

/// A person's Approve command comment on the pull request: it approves nothing, and the reply
/// points at the review.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApproveHint {
    /// The comment's id.
    pub comment: u64,
    /// The pull request's files page, where the review is given.
    pub review_url: String,
}

/// What [`evaluate`] decided.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Outcome {
    /// Whether the pull request may be merged: exactly when `reasons` is empty.
    pub merge_ready: bool,
    /// Every failing guard's class, each at most once, in guard order.
    pub reasons: Vec<Reason>,
    /// The pull request's head.
    pub head: RevId,
    /// The approving reviews that count, in host order.
    pub approvals: Vec<Review>,
    /// How far those approvals cover the approval groups.
    pub cover: Cover,
    /// The Approve command comments, in host order.
    pub hints: Vec<ApproveHint>,
}

/// The issue a factory branch names: `N` of `mintswarm/<N>-<slug>`, or `None` for any other
/// branch.
pub fn linked_issue(branch: &BranchName) -> Option<IssueNumber> {
    let rest = branch.0.strip_prefix(BRANCH_PREFIX)?;
    let (number, slug) = rest.split_once('-')?;
    if number.is_empty() || slug.is_empty() || !number.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    number.parse().ok().map(IssueNumber)
}

/// The reasons found so far, each class kept once.
#[derive(Default)]
struct Reasons(Vec<Reason>);

impl Reasons {
    fn push(&mut self, reason: Reason) {
        if !self.0.contains(&reason) {
            self.0.push(reason);
        }
    }

    fn push_if(&mut self, failed: bool, reason: Reason) {
        if failed {
            self.push(reason);
        }
    }
}

/// Whether the pull request in `facts` is ready to merge, with every failing guard's reason.
///
/// The guards run in this order, and all of them run:
///
/// 1. The linked issue exists, is trusted and is `awaiting merge`.
/// 2. The pull request is open, not a draft, on the base, from a `mintswarm/*` branch in this
///    repository, and has no `hold` label.
/// 3. It is mergeable and does not conflict.
/// 4. The checks were read at the head.
/// 5. A gates-matrix run by `github-actions` has its suite and at least one gate run, every
///    gate run of that suite is by `github-actions`, and every suite with runs and every run
///    completed with `success`.
/// 6. One page shows every status, and every status is `success`.
/// 7. No review thread is unresolved and no review is requested again.
/// 8. The approvals that count cover every approval group.
pub fn evaluate(facts: &Facts<'_>) -> Outcome {
    let mut reasons = Reasons::default();
    check_linked(facts, &mut reasons);
    check_pull(facts, &mut reasons);
    reasons.push_if(facts.checks_at != &facts.pull.head, Reason::ChecksNotAtHead);
    check_runs(facts.runs, facts.suites, &mut reasons);
    check_statuses(facts.statuses, &mut reasons);
    check_reviews(facts, &mut reasons);
    let collected = approvals::collect(facts);
    reasons.push_if(!collected.trusted, Reason::ApprovalsNotTrusted);
    if !collected.cover.covered {
        reasons.push(Reason::NotCovered);
        reasons.push_if(collected.shape_dropped, Reason::ShapeChanged);
    }
    Outcome {
        merge_ready: reasons.0.is_empty(),
        reasons: reasons.0,
        head: facts.pull.head.clone(),
        approvals: collected.approvals,
        cover: collected.cover,
        hints: approvals::hints(facts),
    }
}

/// Guard 1: the linked issue.
fn check_linked(facts: &Facts<'_>, reasons: &mut Reasons) {
    let linked = linked_issue(&facts.pull.branch)
        .and_then(|number| facts.linked.filter(|linked| linked.issue.number == number));
    let Some(linked) = linked else {
        reasons.push(Reason::NoLinkedIssue);
        return;
    };
    reasons.push_if(!facts.ingest.trusts(linked.issue), Reason::IssueUntrusted);
    reasons.push_if(
        linked.state != Some(AWAITING_MERGE),
        Reason::IssueNotAwaitingMerge,
    );
}

/// Guards 2 and 3: the pull request's own fields and its mergeability, as `land.py`'s
/// `check_pull`.
fn check_pull(facts: &Facts<'_>, reasons: &mut Reasons) {
    let pull = facts.pull;
    reasons.push_if(pull.state != "open", Reason::NotOpen);
    reasons.push_if(pull.draft, Reason::Draft);
    reasons.push_if(&pull.base != facts.base, Reason::WrongBase);
    reasons.push_if(
        !pull.branch.0.starts_with(BRANCH_PREFIX),
        Reason::NotFactoryBranch,
    );
    reasons.push_if(
        pull.head_repo.as_deref() != Some(facts.repo),
        Reason::ForkHead,
    );
    reasons.push_if(
        pull.labels
            .iter()
            .any(|label| label.eq_ignore_ascii_case(HOLD_LABEL)),
        Reason::Hold,
    );
    match pull.mergeable {
        None => reasons.push(Reason::MergeabilityUnknown),
        Some(false) => reasons.push(Reason::NotMergeable),
        Some(true) => {}
    }
    reasons.push_if(
        pull.mergeable_state.as_deref() == Some("dirty"),
        Reason::Conflict,
    );
}

/// Whether a run, suite or status ended well: completed with `success`.
fn succeeded(status: Option<&str>, conclusion: Option<&str>) -> bool {
    status == Some("completed") && conclusion == Some("success")
}

/// Guard 5: the check runs and suites, as `land.py`'s `check_runs`.
fn check_runs(runs: &[CheckRun], suites: &[CheckSuite], reasons: &mut Reasons) {
    let matrices: Vec<&CheckRun> = runs
        .iter()
        .filter(|run| run.name == GATES_MATRIX && run.app.as_deref() == Some(ACTIONS_APP))
        .collect();
    reasons.push_if(matrices.is_empty(), Reason::NoGatesMatrix);
    for matrix in matrices {
        let Some(suite) = matrix
            .suite
            .filter(|id| suites.iter().any(|suite| suite.id == *id))
        else {
            reasons.push(Reason::SuiteMissing);
            continue;
        };
        let mut gates = runs
            .iter()
            .filter(|run| !std::ptr::eq(*run, matrix) && run.suite == Some(suite))
            .peekable();
        reasons.push_if(gates.peek().is_none(), Reason::GatesNotRun);
        if gates.any(|gate| gate.app.as_deref() != Some(ACTIONS_APP)) {
            reasons.push(Reason::ForeignGateRun);
        }
    }
    reasons.push_if(
        suites.iter().any(|suite| {
            suite.latest_check_runs_count > 0
                && !succeeded(suite.status.as_deref(), suite.conclusion.as_deref())
        }),
        Reason::SuiteNotSucceeded,
    );
    reasons.push_if(
        runs.iter()
            .any(|run| !succeeded(Some(&run.status), run.conclusion.as_deref())),
        Reason::RunNotSucceeded,
    );
}

/// Guard 6: the commit statuses, as `land.py`'s `check_statuses`.
fn check_statuses(combined: &CombinedStatus, reasons: &mut Reasons) {
    let listed = u64::try_from(combined.statuses.len()).unwrap_or(u64::MAX);
    reasons.push_if(
        combined.total_count > STATUS_PAGE || combined.total_count > listed,
        Reason::TooManyStatuses,
    );
    reasons.push_if(
        combined
            .statuses
            .iter()
            .any(|status| status.conclusion.as_deref() != Some("success")),
        Reason::StatusNotSucceeded,
    );
}

/// Guard 7: unresolved threads, as `land.py`'s `check_threads`, and re-requested reviewers.
///
/// A requested user is pending again when they have an earlier `APPROVED` or
/// `CHANGES_REQUESTED` review; a first request, and any team request, does not block.
fn check_reviews(facts: &Facts<'_>, reasons: &mut Reasons) {
    reasons.push_if(
        facts.threads.iter().any(|thread| !thread.resolved),
        Reason::UnresolvedThreads,
    );
    let rerequested = facts.requested.users.iter().any(|user| {
        facts.reviews.iter().any(|review| {
            review.author.0.eq_ignore_ascii_case(&user.0)
                && matches!(review.state.as_str(), "APPROVED" | "CHANGES_REQUESTED")
        })
    });
    reasons.push_if(rerequested, Reason::ReviewRerequested);
}
