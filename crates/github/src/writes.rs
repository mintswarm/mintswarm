//! Changes to existing issue content as an App's installation, design §6.3–§6.4: a `+1` reaction
//! on a comment, a comment's edited body and an artifact's updated body.
//!
//! Each write is safe to repeat: GitHub answers a repeated `+1` with the reaction it already
//! holds, and an edit or an update sets the body whatever it held. So every write is retried as
//! an idempotent request, and, as each attempt changes content, each attempt takes a grant from
//! a [`ContentLimiter`] first.
//!
//! A 403 that is not a rate limit is [`WriteError::Refused`]: the installation may not change
//! the item. It is told by its status alone; GitHub's wording is never read.

use std::sync::Arc;

use reqwest::StatusCode;
use serde_json::Value;

use mintswarm_domain::ids::IssueNumber;
use mintswarm_domain::retry::{Exhausted, Failure, RetryPolicy};
use mintswarm_domain::tracker::{Artifact, Comment};

use crate::limit::ContentLimiter;
use crate::reads::{map_comment, str_at, text_at};
use crate::rest::{ClientError, Reply, Rest, check_names};
use crate::retry::Gate;
use crate::secret::Token;
use crate::token::Permissions;

/// Why a write failed. No variant carries a token or a body.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum WriteError {
    /// GitHub has no such item, or does not show it to the token.
    #[error("{endpoint}: not found")]
    NotFound {
        /// The method and path.
        endpoint: String,
    },
    /// GitHub refused the change with a 403 that is not a rate limit: the installation may not
    /// change the item.
    #[error("{endpoint}: refused")]
    Refused {
        /// The method and path.
        endpoint: String,
    },
    /// GitHub answered with another status that is not a success.
    #[error("{endpoint}: status {status}")]
    Status {
        /// The method and path.
        endpoint: String,
        /// The HTTP status.
        status: u16,
    },
    /// The write kept failing transiently until its retries ran out; it may have landed.
    #[error(transparent)]
    Exhausted(#[from] Exhausted),
    /// The answer is not JSON of the expected shape.
    #[error("{endpoint}: not the expected JSON")]
    Unexpected {
        /// The method and path.
        endpoint: String,
    },
}

/// Reacts to, edits and updates the issue content of one repository with a token its caller
/// passes in.
#[derive(Debug)]
pub struct IssueWrites {
    rest: Rest,
    owner: String,
    repo: String,
    limiter: Arc<ContentLimiter>,
}

impl IssueWrites {
    /// Writes to `owner/repo` on the API at `api_base` (normally
    /// [`GITHUB_API`](crate::GITHUB_API)), budgeted by `limiter`. It retries with the default
    /// [`RetryPolicy`] and pauses every later call after a rate limit that names a wait.
    pub fn new(
        api_base: &str,
        owner: &str,
        repo: &str,
        limiter: Arc<ContentLimiter>,
    ) -> Result<Self, ClientError> {
        check_names(owner, repo)?;
        Ok(Self {
            rest: Rest::pausing(api_base, RetryPolicy::default())?,
            owner: owner.to_owned(),
            repo: repo.to_owned(),
            limiter,
        })
    }

    /// The writes retrying with `policy` in place of the default.
    #[must_use]
    pub fn with_retry(mut self, policy: RetryPolicy) -> Self {
        self.rest = self.rest.with_policy(policy);
        self
    }

    /// The permissions a token for these writes asks for, from its
    /// [`TokenClient`](crate::TokenClient): writing issues, and reading the repository's metadata.
    pub fn permissions() -> Permissions {
        Permissions::write(["issues"]).and(Permissions::read(["metadata"]))
    }

    /// Reacts `+1` to the issue comment `comment_id`. A comment that already carries this
    /// installation's `+1` keeps that one reaction, and the call succeeds.
    pub async fn react_plus_one(&self, token: &Token, comment_id: u64) -> Result<(), WriteError> {
        let path = format!(
            "{}/issues/comments/{comment_id}/reactions",
            self.repo_path()
        );
        self.grant().await;
        let gate: Gate<'_> = Box::new(move || Box::pin(self.grant()));
        // Idempotent: GitHub answers a repeated `+1` by the same installation with 200 and the
        // reaction it already holds, so a retried attempt adds no second one.
        let sent = self
            .rest
            .post_idempotent_gated(&path, token, &serde_json::json!({ "content": "+1" }), gate)
            .await;
        self.success(sent)?;
        Ok(())
    }

    /// Sets the body of the issue comment `comment_id` to `body`, and returns the comment as
    /// GitHub then holds it.
    pub async fn edit_comment(
        &self,
        token: &Token,
        comment_id: u64,
        body: &str,
    ) -> Result<Comment, WriteError> {
        let path = format!("{}/issues/comments/{comment_id}", self.repo_path());
        let reply = self.patch(token, &path, body).await?;
        let value = json(&reply)?;
        map_comment(&value, &reply.endpoint).map_err(|_| unexpected(&reply))
    }

    /// Sets the body of the artifact numbered `number` on `issue` (its per-issue number, not
    /// its id) to `body`, in place, and returns the artifact as GitHub then holds it.
    pub async fn update_artifact(
        &self,
        token: &Token,
        issue: IssueNumber,
        number: u64,
        body: &str,
    ) -> Result<Artifact, WriteError> {
        let path = format!("{}/issues/{}/artifacts/{number}", self.repo_path(), issue.0);
        let reply = self.patch(token, &path, body).await?;
        let value = json(&reply)?;
        let bad = |_| unexpected(&reply);
        Ok(Artifact {
            name: str_at(&value, "name", &reply.endpoint).map_err(bad)?,
            body: text_at(&value, "body", &reply.endpoint).map_err(bad)?,
        })
    }

    /// A PATCH of `{"body": body}` to `path`, each attempt behind a grant.
    async fn patch(&self, token: &Token, path: &str, body: &str) -> Result<Reply, WriteError> {
        self.grant().await;
        let gate: Gate<'_> = Box::new(move || Box::pin(self.grant()));
        let sent = self
            .rest
            .patch(path, token, &serde_json::json!({ "body": body }), gate)
            .await;
        self.success(sent)
    }

    /// The reply when it is a success, or the write's error. A rate limit that ended the call
    /// with a named wait pauses the limiter for that long.
    fn success(&self, sent: Result<Reply, Exhausted>) -> Result<Reply, WriteError> {
        let reply = match sent {
            Ok(reply) => reply,
            Err(exhausted) => {
                if let Failure::RateLimited { wait: Some(wait) } = exhausted.last {
                    self.limiter.pause(wait);
                }
                return Err(exhausted.into());
            }
        };
        if reply.status.is_success() {
            return Ok(reply);
        }
        Err(match reply.status {
            StatusCode::NOT_FOUND => WriteError::NotFound {
                endpoint: reply.endpoint,
            },
            StatusCode::FORBIDDEN if !reply.rate_limited => WriteError::Refused {
                endpoint: reply.endpoint,
            },
            status => WriteError::Status {
                endpoint: reply.endpoint,
                status: status.as_u16(),
            },
        })
    }

    /// Waits until the limiter grants one request.
    async fn grant(&self) {
        while let Err(wait) = self.limiter.try_acquire() {
            tokio::time::sleep(wait).await;
        }
    }

    fn repo_path(&self) -> String {
        format!("/repos/{}/{}", self.owner, self.repo)
    }
}

fn json(reply: &Reply) -> Result<Value, WriteError> {
    serde_json::from_slice(&reply.body).map_err(|_| unexpected(reply))
}

fn unexpected(reply: &Reply) -> WriteError {
    WriteError::Unexpected {
        endpoint: reply.endpoint.clone(),
    }
}
