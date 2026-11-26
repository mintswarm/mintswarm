//! The merge of a pull request: one squash `PUT pulls/{n}/merge` pinned to the head it was
//! judged at, with a token its caller passes in and this module never mints.
//!
//! A merge that landed answers a repeated attempt with a refusal, so the PUT is sent again only
//! after an attempt that was not carried out (see `retry.rs`). An attempt that may have been
//! ends the call in [`MergeError::Exhausted`], and [`MergeError::may_have_landed`] says so: the
//! caller reads the pull request back to learn whether it merged. GitHub's refusals come back
//! as classes, never with GitHub's text: 409 is [`MergeError::HeadMoved`] and 405
//! [`MergeError::NotMergeable`].

use reqwest::StatusCode;
use serde_json::Value;

use mintswarm_domain::ids::{PrNumber, RevId};
use mintswarm_domain::retry::{Exhausted, Failure, RetryPolicy};
use mintswarm_domain::tracker::Merged;

use crate::repo::is_commit_id;
use crate::rest::{ClientError, Rest, check_names};
use crate::secret::Token;
use crate::token::Permissions;

/// Why a merge failed. No variant carries a token or GitHub's text.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum MergeError {
    /// The sha is not a full commit id; nothing was sent.
    #[error("{sha:?} is not a full commit id")]
    InvalidSha {
        /// The sha given.
        sha: String,
    },
    /// GitHub refused the merge with 409: the head is no longer the sha given.
    #[error("{endpoint}: the head moved")]
    HeadMoved {
        /// The method and path.
        endpoint: String,
    },
    /// GitHub refused the merge with 405: the pull request cannot be merged as it stands.
    #[error("{endpoint}: not mergeable")]
    NotMergeable {
        /// The method and path.
        endpoint: String,
    },
    /// GitHub has no such pull request, or does not show it to the token.
    #[error("{endpoint}: not found")]
    NotFound {
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
    /// GitHub answered with success but not with a merge of the expected shape.
    #[error("{endpoint}: not the expected JSON")]
    Unexpected {
        /// The method and path.
        endpoint: String,
    },
    /// The merge failed transiently until its retries ran out, or after an attempt that may
    /// have been carried out; see [`MergeError::may_have_landed`].
    #[error(transparent)]
    Exhausted(#[from] Exhausted),
}

impl MergeError {
    /// Whether the merge may have landed in spite of the error: the call ended on a failure
    /// that may have been carried out, such as a 5xx, a timeout or a broken connection. Every
    /// earlier attempt was not carried out, or the call would have ended on it.
    pub fn may_have_landed(&self) -> bool {
        match self {
            Self::Exhausted(exhausted) => !matches!(
                exhausted.last,
                Failure::Connect | Failure::RateLimited { .. } | Failure::Request
            ),
            _ => false,
        }
    }
}

/// Merges pull requests in one repository.
#[derive(Debug)]
pub struct MergeClient {
    rest: Rest,
    owner: String,
    repo: String,
}

impl MergeClient {
    /// A client for `owner/repo` on the API at `api_base` (normally
    /// [`GITHUB_API`](crate::GITHUB_API)), retrying with the default [`RetryPolicy`].
    pub fn new(api_base: &str, owner: &str, repo: &str) -> Result<Self, ClientError> {
        check_names(owner, repo)?;
        Ok(Self {
            rest: Rest::new(api_base, RetryPolicy::default())?,
            owner: owner.to_owned(),
            repo: repo.to_owned(),
        })
    }

    /// The client retrying with `policy` in place of the default.
    #[must_use]
    pub fn with_retry(mut self, policy: RetryPolicy) -> Self {
        self.rest = self.rest.with_policy(policy);
        self
    }

    /// The permissions a token for a merge asks for: [`Permissions::merge`].
    pub fn permissions() -> Permissions {
        Permissions::merge()
    }

    /// Squash-merges pull request `pr` if its head is still `sha`, with `title` as the commit
    /// title, verbatim. A `sha` that is not a full commit id is refused before anything is
    /// sent.
    pub async fn merge(
        &self,
        token: &Token,
        pr: PrNumber,
        sha: &RevId,
        title: &str,
    ) -> Result<Merged, MergeError> {
        if !is_commit_id(&sha.0) {
            return Err(MergeError::InvalidSha { sha: sha.0.clone() });
        }
        let path = format!("/repos/{}/{}/pulls/{}/merge", self.owner, self.repo, pr.0);
        let body = serde_json::json!({
            "merge_method": "squash",
            "sha": sha.0,
            "commit_title": title,
        });
        let reply = self.rest.put(&path, token, &body).await?;
        let endpoint = reply.endpoint;
        match reply.status {
            StatusCode::OK => {}
            StatusCode::CONFLICT => return Err(MergeError::HeadMoved { endpoint }),
            StatusCode::METHOD_NOT_ALLOWED => return Err(MergeError::NotMergeable { endpoint }),
            StatusCode::NOT_FOUND => return Err(MergeError::NotFound { endpoint }),
            status => {
                return Err(MergeError::Status {
                    endpoint,
                    status: status.as_u16(),
                });
            }
        }
        let merged = serde_json::from_slice::<Value>(&reply.body)
            .ok()
            .filter(|body| body.get("merged").and_then(Value::as_bool) == Some(true))
            .and_then(|body| body.get("sha").and_then(Value::as_str).map(str::to_owned))
            .filter(|sha| is_commit_id(sha));
        match merged {
            Some(sha) => Ok(Merged { sha: RevId(sha) }),
            None => Err(MergeError::Unexpected { endpoint }),
        }
    }
}
