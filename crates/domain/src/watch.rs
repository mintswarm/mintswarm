//! What the plane's watch sees on the tracker and the code host (design §9.3).
//!
//! The ETag poller and, later, webhooks both produce [`WatchEvent`] values, so the job loop
//! consumes one type whatever the source. Producers run the comment-command parser and the trust
//! rules before they emit an event: a command event always comes from a trusted author. Unknown
//! fields are ignored on read, so that older readers tolerate fields added later.

use serde::{Deserialize, Serialize};

use crate::ids::{IssueNumber, PrNumber, RevId};
use crate::tracker::{Login, Review};

/// One thing the watch observed.
///
/// Each variant maps to a [`job::Event`](crate::job::Event): `Approve` to `Approved`, `Changes`
/// to `ChangesRequested`, `Review` to `PrApproved` or `ReviewChangesRequested` by the review's
/// state, `Merged` to `Merged`, and every other command to the event of the same name.
/// `IssueChanged` feeds reconciliation rather than a single job event.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WatchEvent {
    /// The `approve` command.
    Approve(CommandEvent),
    /// The `changes` command.
    Changes(CommandEvent),
    /// The `reject` command.
    Reject(CommandEvent),
    /// The `veto` command.
    Veto(CommandEvent),
    /// The `steer` command.
    Steer(CommandEvent),
    /// The `continue` command.
    Continue(CommandEvent),
    /// The `accept` command.
    Accept(CommandEvent),
    /// The `replan` command.
    Replan(CommandEvent),
    /// The `cancel` command.
    Cancel(CommandEvent),
    /// A review was submitted on a pull request.
    Review(ReviewEvent),
    /// A pull request was merged.
    Merged(MergedEvent),
    /// An issue was closed, reopened, relabelled or edited.
    IssueChanged(IssueChangedEvent),
}

/// A comment command from a trusted author.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandEvent {
    /// The issue the command is about.
    pub issue: IssueNumber,
    /// The pull request the comment was made on, if any.
    pub pr: Option<PrNumber>,
    /// The comment carrying the command, which also keys deduplication.
    pub comment_id: u64,
    pub author: Login,
    /// RFC 3339.
    pub observed_at: String,
    /// The text after the command word, for example a `steer` instruction.
    pub text: Option<String>,
}

/// A review submitted on a pull request.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewEvent {
    pub pr: PrNumber,
    pub review: Review,
    /// RFC 3339.
    pub observed_at: String,
}

/// A merged pull request.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MergedEvent {
    pub pr: PrNumber,
    /// The head commit the pull request was merged at.
    pub head: RevId,
    /// RFC 3339.
    pub observed_at: String,
}

/// A change to an issue.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct IssueChangedEvent {
    pub issue: IssueNumber,
    pub change: IssueChange,
    /// RFC 3339.
    pub observed_at: String,
}

/// What changed on an issue.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "change", rename_all = "snake_case")]
pub enum IssueChange {
    Closed,
    Reopened,
    Labelled { label: String },
    Unlabelled { label: String },
    Edited,
}
