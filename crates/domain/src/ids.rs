//! Identifiers shared by jobs, records and the runner's traits.
//!
//! Each is a newtype that serialises as its inner value.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

macro_rules! id {
    ($(#[$doc:meta])* $name:ident($inner:ty)) => {
        $(#[$doc])*
        #[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub $inner);
    };
}

id!(
    /// A job: one issue's way through the pipeline.
    JobId(String)
);
id!(
    /// One attempt at a stage or step of a job.
    AttemptId(String)
);
id!(
    /// A worker slot of a runner.
    WorkerId(String)
);
id!(
    /// A project the plane serves: one repository with its config.
    ProjectId(String)
);
id!(
    /// A runner daemon.
    RunnerId(String)
);
id!(
    /// The position of an event or record in a runner's outbox.
    Seq(u64)
);
id!(
    /// A full hexadecimal commit id, never revision syntax.
    RevId(String)
);
id!(
    /// An issue number on the tracker.
    IssueNumber(u64)
);
id!(
    /// A pull request number on the code host.
    PrNumber(u64)
);
id!(
    /// A harness account, by the nickname the runner config gives it.
    AccountNickname(String)
);
id!(
    /// The fence of an attempt's lease: the plane raises it on every reassignment, so writes
    /// from an older holder are refused.
    LeaseEpoch(u64)
);
id!(
    /// A directive, so the runner can acknowledge it with a receipt.
    DirectiveId(String)
);
id!(
    /// A person's account in mintswarm, the identity work items are owned by.
    UserId(String)
);
id!(
    /// A work item: an issue, a plan, a decomposition or a pull request mintswarm tracks.
    WorkItemId(String)
);
id!(
    /// A person's identity in the plane, which login bindings, sessions and links point to.
    IdentityId(String)
);
id!(
    /// A signed-in session of an identity.
    SessionId(String)
);
id!(
    /// One write action an identity asked the plane to perform on the code host.
    ActionId(String)
);
id!(
    /// A login provider, by the id the server config gives it.
    ProviderId(String)
);
id!(
    /// A GitHub user's numeric id; `i64`, as auth.v1's `int64 github_user_id` and Postgres
    /// `bigint` hold it.
    GitHubUserId(i64)
);

/// The key a runner names a repository's store, snapshots and job refs by, such as
/// `mintswarm--mintswarm`.
///
/// It is one path component: never empty, never starting with `.`, and with no `/` or NUL. The
/// field is private, so every key held has been checked.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct RepoKey(String);

impl RepoKey {
    /// The key `key`; one that is not a single path component is rejected.
    pub fn new(key: impl Into<String>) -> Result<Self, RepoKeyError> {
        let key = key.into();
        let message = if key.is_empty() {
            "the key is empty"
        } else if key.starts_with('.') {
            "the key starts with `.`"
        } else if key.contains('/') {
            "the key holds `/`"
        } else if key.contains('\0') {
            "the key holds NUL"
        } else {
            return Ok(RepoKey(key));
        };
        Err(RepoKeyError {
            key,
            message: message.to_owned(),
        })
    }

    /// The key as text.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for RepoKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A string that is not a repo key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RepoKeyError {
    /// The string as given.
    pub key: String,
    /// What is wrong.
    pub message: String,
}

impl fmt::Display for RepoKeyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?} is not a repo key: {}", self.key, self.message)
    }
}

impl std::error::Error for RepoKeyError {}

impl FromStr for RepoKey {
    type Err = RepoKeyError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        RepoKey::new(text)
    }
}

impl TryFrom<String> for RepoKey {
    type Error = RepoKeyError;

    fn try_from(text: String) -> Result<Self, Self::Error> {
        RepoKey::new(text)
    }
}

impl From<RepoKey> for String {
    fn from(key: RepoKey) -> Self {
        key.0
    }
}
