//! The tracker and code host abstractions, design §9.
//!
//! The types hold the fields the pipeline reads; implementations may extend them.

use std::future::Future;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::field::{FieldError, FieldRead, FieldRef};
use crate::hosting::CodeOwnersFile;
use crate::ids::{IssueNumber, PrNumber, RevId};
use crate::vcs::BranchName;

/// A login on the tracker or code host.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Login(pub String);

/// The kind of an issue.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum IssueType {
    Design,
    Plan,
    Implementation,
}

/// A login's role on a repository, as GitHub's collaborator permission read names it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RepoRole {
    Admin,
    Maintain,
    Write,
    Triage,
    Read,
    /// Not a collaborator, or no such login.
    NoAccess,
}

/// Whether a tracker user is a bot: its user type is `Bot`, or its login ends in `[bot]` in any
/// ASCII case. `user_type` is the type the tracker reports, when the read gives one.
pub fn is_bot(login: &str, user_type: Option<&str>) -> bool {
    const SUFFIX: &str = "[bot]";
    user_type == Some("Bot")
        || (login.len() >= SUFFIX.len()
            && login.is_char_boundary(login.len() - SUFFIX.len())
            && login[login.len() - SUFFIX.len()..].eq_ignore_ascii_case(SUFFIX))
}

impl RepoRole {
    /// Whether the role may push to the repository: admin, maintain and write.
    pub fn can_write(self) -> bool {
        matches!(self, Self::Admin | Self::Maintain | Self::Write)
    }
}

/// An issue.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Issue {
    pub number: IssueNumber,
    pub title: String,
    pub body: String,
    pub state: String,
    pub labels: Vec<String>,
    pub issue_type: Option<IssueType>,
    pub author: Login,
    /// Whether the author is a bot, as [`is_bot`] decides from the author's login and type.
    #[serde(default)]
    pub author_is_bot: bool,
    pub url: String,
    /// The total from the issue's sub-issue summary.
    pub sub_issues: u64,
    /// The total from the issue's dependency summary: the issues that block it.
    pub blocked_by: u64,
}

/// A comment on an issue or PR.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Comment {
    pub id: u64,
    pub author: Login,
    pub body: String,
    /// RFC 3339.
    pub created_at: String,
    /// RFC 3339.
    pub updated_at: String,
    pub url: String,
}

/// Where a read of an issue's comments left off.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Cursor(pub String);

/// An issue and its comments since a cursor.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct IssueRead {
    pub issue: Issue,
    pub comments: Vec<Comment>,
    pub cursor: Cursor,
}

/// What a comment is to the pipeline, design §9 "Reads and writes": a steer relays the
/// maintainer's direction, a note is the bot's own record.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommentMark {
    /// A trusted author's comment, or a bot comment carrying the steer marker.
    Steer,
    /// A bot comment carrying the note marker.
    Note,
}

/// A document published on an issue.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Artifact {
    pub name: String,
    pub body: String,
}

/// An issue's artifact as its list shows it, without the body.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactSummary {
    /// The tracker's id of the artifact.
    pub id: u64,
    /// The artifact's number on its issue (1, 2, …), by which it is read.
    pub number: u64,
    pub name: String,
    /// Where people read it.
    pub url: String,
}

/// Why people are assigned to an item.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssignmentReason {
    Plan,
    NeedsInput,
    Pr,
    ExternalSteps,
    Design,
    Decomposition,
    Escalation,
    Merge,
}

/// A child issue to file.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubItem {
    pub title: String,
    pub body: String,
    pub labels: Vec<String>,
    pub issue_type: Option<IssueType>,
}

/// How an issue relates to another item.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LinkKind {
    Parent,
    SubItem,
    PullRequest,
}

/// A link from an issue to another item.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Link {
    pub kind: LinkKind,
    pub number: u64,
}

/// An issue's parent issue as [`Collaborators::read_parent`] reads it: its number and its
/// author.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParentIssue {
    pub number: IssueNumber,
    /// The parent's author; `None` when the read named no user login, so the parent's author
    /// is no person.
    pub author: Option<Login>,
    /// Whether the author is a bot, as [`is_bot`] decides; false when there is no author.
    pub author_is_bot: bool,
}

/// Which way a dependency points from an issue.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    /// The issues that block this one.
    BlockedBy,
    /// The issues this one blocks.
    Blocking,
}

/// An issue on the other end of a dependency.
///
/// A dependency is read only through [`Links::read_dependencies`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Dependency {
    /// The issue's repository, in `owner/name` form.
    pub repository: String,
    pub number: IssueNumber,
    /// Whether the issue is still open.
    pub open: bool,
}

/// The error every tracker capability but [`Fields`] shares, so a [`Tracker`] has one error.
pub trait TrackerBase: Send + Sync {
    type Error: std::error::Error + Send + Sync + 'static;
}

/// Reading, filing and typing issues.
pub trait Issues: TrackerBase {
    /// Issues ready to be picked up.
    fn fetch_ready(&self) -> impl Future<Output = Result<Vec<Issue>, Self::Error>> + Send;

    /// An issue and its comments since a cursor.
    fn read_issue(
        &self,
        issue: IssueNumber,
        since: Option<Cursor>,
    ) -> impl Future<Output = Result<IssueRead, Self::Error>> + Send;

    fn create_sub_item(
        &self,
        parent: IssueNumber,
        item: SubItem,
    ) -> impl Future<Output = Result<Issue, Self::Error>> + Send;

    fn set_issue_type(
        &self,
        issue: IssueNumber,
        kind: IssueType,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send;
}

/// An issue's links to other issues and its dependencies.
pub trait Links: TrackerBase {
    fn read_links(
        &self,
        issue: IssueNumber,
    ) -> impl Future<Output = Result<Vec<Link>, Self::Error>> + Send;

    /// Record that `issue` is blocked by `blocker`.
    fn link_dependency(
        &self,
        issue: IssueNumber,
        blocker: IssueNumber,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send;

    /// The issues that block `issue`, or that it blocks.
    fn read_dependencies(
        &self,
        issue: IssueNumber,
        direction: Direction,
    ) -> impl Future<Output = Result<Vec<Dependency>, Self::Error>> + Send;
}

/// Linking pull requests to issues.
pub trait PullLinks: TrackerBase {
    fn link_pr(
        &self,
        issue: IssueNumber,
        pr: PrNumber,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send;
}

/// An issue's status.
pub trait Statuses: TrackerBase {
    /// The issue's status; `None` when it has none.
    fn read_status(
        &self,
        issue: IssueNumber,
    ) -> impl Future<Output = Result<Option<String>, Self::Error>> + Send;

    fn set_status(
        &self,
        issue: IssueNumber,
        status: String,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send;
}

/// Commenting on issues.
pub trait Comments: TrackerBase {
    fn post_comment(
        &self,
        issue: IssueNumber,
        body: String,
    ) -> impl Future<Output = Result<Comment, Self::Error>> + Send;
}

/// Publishing documents on issues.
pub trait Artifacts: TrackerBase {
    /// Publish a document on an issue; returns its URL.
    fn publish_artifact(
        &self,
        issue: IssueNumber,
        artifact: Artifact,
    ) -> impl Future<Output = Result<String, Self::Error>> + Send;
}

/// Assigning people to items.
pub trait Assignments: TrackerBase {
    /// Assign people to an item while it waits on them.
    fn assign(
        &self,
        issue: IssueNumber,
        who: Vec<Login>,
        reason: AssignmentReason,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send;

    /// Release the bot's own assignments of an item.
    fn release(&self, issue: IssueNumber) -> impl Future<Output = Result<(), Self::Error>> + Send;
}

/// Reacting to comments.
///
/// It is not part of [`Tracker`]: only the plane reacts, so a [`Tracker`] bound alone does not
/// give it.
///
/// ```compile_fail
/// use mintswarm_domain::tracker::{Reactions, Tracker};
///
/// fn react<T: Tracker>(tracker: &T) {
///     let _ = tracker.react_plus_one(1);
/// }
/// ```
pub trait Reactions: TrackerBase {
    /// Reacts `+1` to the comment `comment_id`. It is idempotent: a comment that already
    /// carries the tracker's own `+1` keeps that one reaction, and the call succeeds.
    fn react_plus_one(
        &self,
        comment_id: u64,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send;
}

/// Changing the body of an existing comment or artifact.
///
/// It is kept apart from [`Reactions`] and [`Collaborators`] so that a runner may implement it
/// under the cross-App fallback, editing what its own App posted, without the other two.
pub trait CommentEdits: TrackerBase {
    /// Sets the body of the comment `comment_id` to `body`; returns the comment as the tracker
    /// then holds it.
    fn edit_comment(
        &self,
        comment_id: u64,
        body: String,
    ) -> impl Future<Output = Result<Comment, Self::Error>> + Send;

    /// Sets the body of the artifact numbered `number` on `issue` (its per-issue number, as in
    /// [`ArtifactSummary::number`]) to `body`, in place; returns the artifact as the tracker
    /// then holds it.
    fn update_artifact(
        &self,
        issue: IssueNumber,
        number: u64,
        body: String,
    ) -> impl Future<Output = Result<Artifact, Self::Error>> + Send;
}

/// Who may act on the repository, and which issue an issue belongs to.
///
/// It is not part of [`Tracker`]: only the plane reads them, so a [`Tracker`] bound alone does
/// not give it.
///
/// ```compile_fail
/// use mintswarm_domain::tracker::{Collaborators, Login, Tracker};
///
/// fn role<T: Tracker>(tracker: &T) {
///     let _ = tracker.read_permission(Login("someone".into()));
/// }
/// ```
pub trait Collaborators: TrackerBase {
    /// `login`'s role on the repository; a login that is no collaborator, or no user, is
    /// [`RepoRole::NoAccess`].
    fn read_permission(
        &self,
        login: Login,
    ) -> impl Future<Output = Result<RepoRole, Self::Error>> + Send;

    /// The issue's parent issue with its author, or `None` when it has none.
    fn read_parent(
        &self,
        issue: IssueNumber,
    ) -> impl Future<Output = Result<Option<ParentIssue>, Self::Error>> + Send;
}

/// A role's field on an issue, design §4.6.
pub trait Fields: Send + Sync {
    /// The value of a role's field on an issue, design §4.6.
    fn read_field(
        &self,
        issue: IssueNumber,
        field: &FieldRef,
    ) -> impl Future<Output = Result<FieldRead, FieldError>> + Send;

    /// Set a role's field on an issue to the named option; `None` clears the field.
    ///
    /// It is never called for a read-only role.
    fn set_field(
        &self,
        issue: IssueNumber,
        field: &FieldRef,
        option: Option<&str>,
    ) -> impl Future<Output = Result<(), FieldError>> + Send;
}

/// The issue tracker: every capability at once. Any type implementing them all is one.
pub trait Tracker:
    Issues + Links + PullLinks + Statuses + Comments + Artifacts + Assignments + Fields
{
}

impl<T> Tracker for T where
    T: Issues + Links + PullLinks + Statuses + Comments + Artifacts + Assignments + Fields
{
}

/// A pull request to open.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewPr {
    pub branch: BranchName,
    pub base: BranchName,
    pub title: String,
    pub body: String,
    pub draft: bool,
}

/// A review of a pull request.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Review {
    pub id: u64,
    pub author: Login,
    /// For example `APPROVED` or `CHANGES_REQUESTED`.
    pub state: String,
    /// RFC 3339; `None` while pending.
    pub submitted_at: Option<String>,
    pub commit: Option<RevId>,
    pub url: String,
    /// Whether the author is a bot: a `Bot` account, or a login ending in `[bot]`.
    #[serde(default)]
    pub author_is_bot: bool,
}

/// A comment on a pull request's diff, part of a review.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewComment {
    pub id: u64,
    /// The review the comment belongs to, if any.
    pub review: Option<u64>,
    pub author: Login,
    /// Whether the author is a bot, as for [`Review::author_is_bot`].
    pub author_is_bot: bool,
    pub body: String,
    /// RFC 3339.
    pub created_at: String,
    /// RFC 3339.
    pub updated_at: String,
    pub url: String,
}

/// A review thread on a pull request and whether it is resolved.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewThread {
    /// The host's id of the thread.
    pub id: String,
    pub resolved: bool,
}

/// The people and teams a pull request's review is requested from and not yet given by.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequestedReviewers {
    pub users: Vec<Login>,
    /// Team slugs.
    pub teams: Vec<String>,
}

/// A check run on a commit.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckRun {
    pub id: u64,
    pub name: String,
    /// For example `queued`, `in_progress` or `completed`.
    pub status: String,
    /// How the run ended (`success`, `failure`, ...); `None` until it completes.
    pub conclusion: Option<String>,
    /// The slug of the app that made the run.
    pub app: Option<String>,
    /// The id of the check suite the run belongs to.
    pub suite: Option<u64>,
}

/// A check suite on a commit: one app's runs.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckSuite {
    pub id: u64,
    /// The slug of the app the suite belongs to, which names it.
    pub app: Option<String>,
    /// For example `queued`, `in_progress` or `completed`.
    pub status: Option<String>,
    /// How the suite ended; `None` until it completes.
    pub conclusion: Option<String>,
    /// How many runs the suite's latest attempt holds.
    pub latest_check_runs_count: u64,
}

/// The commit statuses on a commit, combined.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CombinedStatus {
    /// The combined state: `success`, `failure` or `pending`.
    pub state: String,
    /// How many statuses the host counts; the list may hold fewer.
    pub total_count: u64,
    /// The latest status of each context, with source `status`.
    pub statuses: Vec<CheckState>,
}

/// A file a pull request changes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangedFile {
    pub path: String,
    /// The path before a rename; `None` for any other change.
    pub previous_path: Option<String>,
    /// For example `added`, `modified`, `removed` or `renamed`.
    pub status: String,
}

impl ChangedFile {
    /// Every path the change touches: the path, and the previous one of a rename.
    pub fn paths(&self) -> impl Iterator<Item = &str> {
        std::iter::once(self.path.as_str()).chain(self.previous_path.as_deref())
    }
}

/// A pull request's own fields, without its reviews or checks.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PullDetail {
    pub number: PrNumber,
    pub url: String,
    pub title: String,
    pub state: String,
    pub draft: bool,
    pub base: BranchName,
    pub branch: BranchName,
    pub head: RevId,
    /// The head's repository, in `owner/name` form; `None` when it was deleted.
    pub head_repo: Option<String>,
    pub labels: Vec<String>,
    /// Whether it can be merged; `None` while the host is still working it out.
    pub mergeable: Option<bool>,
    pub mergeable_state: Option<String>,
    pub merged: bool,
    /// The merge commit once merged; before that, the host's test merge, if any.
    pub merge_commit: Option<RevId>,
    /// How many files it changes, as the host counts them.
    pub changed_files: u64,
}

/// A pull request merged, with its merge commit.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Merged {
    pub sha: RevId,
}

/// A check run or commit status on a pull request's head.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckState {
    pub name: String,
    /// How the check ended (`success`, `failure`, a status's `error`, ...); `None` while it
    /// is still running or pending.
    pub conclusion: Option<String>,
    /// `check-run` or `status`.
    pub source: String,
    pub app: Option<String>,
}

/// A pull request as the pipeline reads it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PullRequest {
    pub number: PrNumber,
    pub url: String,
    pub branch: BranchName,
    pub base: BranchName,
    pub head: RevId,
    pub title: String,
    pub state: String,
    pub draft: bool,
    pub mergeable_state: Option<String>,
    pub reviews: Vec<Review>,
    pub requested_reviewers: Vec<Login>,
    pub checks: Vec<CheckState>,
}

/// How a merge the host answered ended: merged, or refused for one of two reasons.
///
/// A failure that leaves the merge's fate unknown is the tracker's error instead; see
/// [`Merges::may_have_landed`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MergeOutcome {
    /// The pull request was merged, with this merge commit.
    Merged(Merged),
    /// The pull request's head is no longer the sha the merge was pinned to; nothing merged.
    HeadMoved,
    /// The host refused the merge as the pull request stands; nothing merged.
    NotMergeable,
}

/// A pull request's reviews, review comments and threads, requested reviewers, and the checks
/// and statuses on a commit.
///
/// It is not part of [`Tracker`]: only the plane reads them, so a [`Tracker`] bound alone does
/// not give it.
///
/// ```compile_fail
/// use mintswarm_domain::ids::PrNumber;
/// use mintswarm_domain::tracker::{Reviews, Tracker};
///
/// fn reviews<T: Tracker>(tracker: &T) {
///     let _ = tracker.read_reviews(PrNumber(1));
/// }
/// ```
pub trait Reviews: TrackerBase {
    /// The pull request's own fields.
    fn read_pull(
        &self,
        pr: PrNumber,
    ) -> impl Future<Output = Result<PullDetail, Self::Error>> + Send;

    /// Every review of the pull request, in the host's order.
    fn read_reviews(
        &self,
        pr: PrNumber,
    ) -> impl Future<Output = Result<Vec<Review>, Self::Error>> + Send;

    /// Every comment on the pull request's diff.
    fn read_review_comments(
        &self,
        pr: PrNumber,
    ) -> impl Future<Output = Result<Vec<ReviewComment>, Self::Error>> + Send;

    /// Every review thread on the pull request, and whether each is resolved.
    fn read_review_threads(
        &self,
        pr: PrNumber,
    ) -> impl Future<Output = Result<Vec<ReviewThread>, Self::Error>> + Send;

    /// The people and teams the pull request's review is still requested from.
    fn read_requested_reviewers(
        &self,
        pr: PrNumber,
    ) -> impl Future<Output = Result<RequestedReviewers, Self::Error>> + Send;

    /// The latest check runs on the commit `sha`.
    fn read_check_runs(
        &self,
        sha: RevId,
    ) -> impl Future<Output = Result<Vec<CheckRun>, Self::Error>> + Send;

    /// The check suites on the commit `sha`.
    fn read_check_suites(
        &self,
        sha: RevId,
    ) -> impl Future<Output = Result<Vec<CheckSuite>, Self::Error>> + Send;

    /// The commit statuses on the commit `sha`, combined.
    fn read_statuses(
        &self,
        sha: RevId,
    ) -> impl Future<Output = Result<CombinedStatus, Self::Error>> + Send;
}

/// The CODEOWNERS file at a commit, and the paths a pull request changes.
///
/// It is not part of [`Tracker`]: only the plane reads them, so a [`Tracker`] bound alone does
/// not give it.
///
/// ```compile_fail
/// use mintswarm_domain::ids::RevId;
/// use mintswarm_domain::tracker::{CodeOwners, Tracker};
///
/// fn owners<T: Tracker>(tracker: &T) {
///     let _ = tracker.read_codeowners(RevId("0".repeat(40)));
/// }
/// ```
pub trait CodeOwners: TrackerBase {
    /// The CODEOWNERS file in force at commit `rev`: the first of
    /// [`CODEOWNERS_PATHS`](crate::hosting::CODEOWNERS_PATHS) that holds a file, or `None` when
    /// none does. `rev` must be a full commit id; a moving ref such as a branch name is refused
    /// before the host is asked.
    fn read_codeowners(
        &self,
        rev: RevId,
    ) -> impl Future<Output = Result<Option<CodeOwnersFile>, Self::Error>> + Send;

    /// Every file the pull request changes.
    fn read_changed_files(
        &self,
        pr: PrNumber,
    ) -> impl Future<Output = Result<Vec<ChangedFile>, Self::Error>> + Send;
}

/// Merging pull requests.
///
/// It is not part of [`Tracker`], and the runner never implements it: only the plane merges
/// (design §13 P4), so a [`Tracker`] bound alone does not give it.
///
/// ```compile_fail
/// use mintswarm_domain::tracker::{Merges, Tracker};
///
/// fn merges<M: Merges>() {}
///
/// fn tracker<T: Tracker>() {
///     merges::<T>();
/// }
/// ```
pub trait Merges: TrackerBase {
    /// The credential a merge is made with, which its caller mints and passes in.
    type MergeToken: Send + Sync;

    /// Squash-merges `pr` if its head is still `sha`, with `title` as the commit title. The
    /// call uses `token` and mints nothing. A host refusal is an [`MergeOutcome`]; an error
    /// may leave the merge's fate unknown, which [`Merges::may_have_landed`] tells.
    fn merge(
        &self,
        token: &Self::MergeToken,
        pr: PrNumber,
        sha: RevId,
        title: String,
    ) -> impl Future<Output = Result<MergeOutcome, Self::Error>> + Send;

    /// Whether a merge that failed with `error` may have landed all the same, so the caller
    /// must read the pull request back to learn whether it merged.
    fn may_have_landed(error: &Self::Error) -> bool;
}

/// The code host. It declares no merge: the runner never merges.
pub trait CodeHost: Send + Sync {
    type Error: std::error::Error + Send + Sync + 'static;

    /// Open a pull request, or return the branch's existing one.
    fn open_pr(&self, pr: NewPr) -> impl Future<Output = Result<PullRequest, Self::Error>> + Send;

    fn mark_ready(&self, pr: PrNumber) -> impl Future<Output = Result<(), Self::Error>> + Send;

    fn request_review(
        &self,
        pr: PrNumber,
        reviewers: Vec<Login>,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send;

    /// Reviews, requested reviewers, mergeable state, and checks and statuses.
    fn read_pr(
        &self,
        pr: PrNumber,
    ) -> impl Future<Output = Result<PullRequest, Self::Error>> + Send;
}

#[cfg(test)]
mod tests {
    use super::RepoRole;

    #[test]
    fn only_admin_maintain_and_write_can_write() {
        for (role, writes) in [
            (RepoRole::Admin, true),
            (RepoRole::Maintain, true),
            (RepoRole::Write, true),
            (RepoRole::Triage, false),
            (RepoRole::Read, false),
            (RepoRole::NoAccess, false),
        ] {
            assert_eq!(role.can_write(), writes, "{role:?}");
        }
    }
}
