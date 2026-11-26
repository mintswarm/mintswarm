//! The tracker and code host abstractions, design §9.
//!
//! The types hold the fields the pipeline reads; implementations may extend them.

use std::future::Future;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::field::{FieldError, FieldRead, FieldRef};
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
