//! The tracker and code host abstractions, design §9.
//!
//! The types hold the fields the pipeline reads; implementations may extend them.

use std::future::Future;

use serde::{Deserialize, Serialize};

use crate::field::{FieldError, FieldRead, FieldRef};
use crate::ids::{IssueNumber, PrNumber, RevId};
use crate::vcs::BranchName;

/// A login on the tracker or code host.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Login(pub String);

/// The kind of an issue.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
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

/// A document published on an issue.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Artifact {
    pub name: String,
    pub body: String,
}

/// Why people are assigned to an item.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssignReason {
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
    BlockedBy,
    Blocks,
    PullRequest,
}

/// A link from an issue to another item.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Link {
    pub kind: LinkKind,
    pub number: u64,
}

/// The issue tracker.
pub trait Tracker: Send + Sync {
    type Error: std::error::Error + Send + Sync + 'static;

    /// Issues ready to be picked up.
    fn fetch_ready(&self) -> impl Future<Output = Result<Vec<Issue>, Self::Error>> + Send;

    fn post_comment(
        &self,
        issue: IssueNumber,
        body: String,
    ) -> impl Future<Output = Result<Comment, Self::Error>> + Send;

    fn set_status(
        &self,
        issue: IssueNumber,
        status: String,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send;

    fn create_sub_item(
        &self,
        parent: IssueNumber,
        item: SubItem,
    ) -> impl Future<Output = Result<Issue, Self::Error>> + Send;

    fn link_pr(
        &self,
        issue: IssueNumber,
        pr: PrNumber,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send;

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

    /// An issue and its comments since a cursor.
    fn read_issue(
        &self,
        issue: IssueNumber,
        since: Option<Cursor>,
    ) -> impl Future<Output = Result<IssueRead, Self::Error>> + Send;

    /// Publish a document on an issue; returns its URL.
    fn publish_artifact(
        &self,
        issue: IssueNumber,
        artifact: Artifact,
    ) -> impl Future<Output = Result<String, Self::Error>> + Send;

    fn set_issue_type(
        &self,
        issue: IssueNumber,
        kind: IssueType,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send;

    /// Assign people to an item while it waits on them.
    fn assign(
        &self,
        issue: IssueNumber,
        who: Vec<Login>,
        reason: AssignReason,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send;

    /// Release the bot's own assignments of an item.
    fn release(&self, issue: IssueNumber) -> impl Future<Output = Result<(), Self::Error>> + Send;

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
