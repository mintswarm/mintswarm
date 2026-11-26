//! Notes posted on issues as the runner App, design §9 "Reads and writes": each post is one
//! content-creating request, budgeted by a [`ContentLimiter`], and sent as a create with a probe
//! so an attempt that may have landed is never posted twice.
//!
//! A note's body carries [`NOTE_MARKER`] and a key line of its own. After an ambiguous attempt
//! the probe lists the issue's comments since the call started, less [`PROBE_MARGIN`] for clock
//! skew, and takes the newest bot note created since then that carries the key as the landed
//! attempt. A probe that cannot read the comments ends the call rather than risk a duplicate.

use std::sync::Arc;
use std::time::Duration;

use reqwest::StatusCode;
use serde_json::Value;

use mintswarm_domain::ids::IssueNumber;
use mintswarm_domain::retry::{Exhausted, Failure, RetryPolicy};
use mintswarm_domain::time::format_utc_seconds;
use mintswarm_domain::tracker::{Comment, CommentMark, Login};

use crate::limit::ContentLimiter;
use crate::reads::{MAX_PAGES, NOTE_MARKER, PER_PAGE, STEER_MARKER, comment_mark, map_comment};
use crate::rest::{ClientError, Reply, Rest, check_names, path_segment};
use crate::retry::{Landing, Probe};
use crate::secret::Token;
use crate::token::{Clock, Permissions};

/// How far before the call's start the probe looks for a landed note, for the skew between this
/// host's clock and GitHub's.
pub const PROBE_MARGIN: Duration = Duration::from_secs(10);

/// Why a note could not be posted. No variant carries a token or a response body.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum CommentError {
    /// The body lacks the note marker line or the key line, or the key is empty or a marker
    /// every note of its kind carries, so a probe could not tell the note from others; nothing
    /// was sent.
    #[error("the note's body does not carry its marker and key lines")]
    Unmarked,
    /// GitHub has no such issue, or does not show it to the token.
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
    /// The create kept failing transiently until its retries ran out, or its probe could not
    /// tell whether an earlier attempt landed; the note may have been posted.
    #[error(transparent)]
    Exhausted(#[from] Exhausted),
    /// The answer is not JSON of the expected shape.
    #[error("{endpoint}: not the expected JSON")]
    Unexpected {
        /// The method and path.
        endpoint: String,
    },
}

/// Posts notes on the issues of one repository as `bot`.
#[derive(Debug)]
pub struct CommentClient {
    rest: Rest,
    owner: String,
    repo: String,
    bot: Login,
    limiter: Arc<ContentLimiter>,
    clock: Clock,
}

impl CommentClient {
    /// A client for `owner/repo` on the API at `api_base` (normally
    /// [`GITHUB_API`](crate::GITHUB_API)), posting as `bot`, budgeted by `limiter` and timing
    /// its probes by `clock`. It retries with the default [`RetryPolicy`] and pauses every later
    /// call after a rate limit that names a wait.
    pub fn new(
        api_base: &str,
        owner: &str,
        repo: &str,
        bot: Login,
        limiter: Arc<ContentLimiter>,
        clock: Clock,
    ) -> Result<Self, ClientError> {
        check_names(owner, repo)?;
        Ok(Self {
            rest: Rest::pausing(api_base, RetryPolicy::default())?,
            owner: owner.to_owned(),
            repo: repo.to_owned(),
            bot,
            limiter,
            clock,
        })
    }

    /// The client retrying with `policy` in place of the default.
    #[must_use]
    pub fn with_retry(mut self, policy: RetryPolicy) -> Self {
        self.rest = self.rest.with_policy(policy);
        self
    }

    /// The permissions a token for its posts asks for, from its
    /// [`TokenClient`](crate::TokenClient): writing issues, and reading the repository's metadata.
    pub fn permissions() -> Permissions {
        Permissions::write(["issues"]).and(Permissions::read(["metadata"]))
    }

    /// Posts `body` as a note on `issue` with `token`, once.
    ///
    /// `body` must hold a line equal to [`NOTE_MARKER`] and a line equal to `key`, lines split
    /// at LF and trimmed, and `key` must be neither [`NOTE_MARKER`] nor [`STEER_MARKER`], or the
    /// call is [`CommentError::Unmarked`] before any request. The call
    /// then waits until the limiter grants one request, and sends the create. After an attempt
    /// that may have landed, the probe's find is returned in place of a second post. A rate
    /// limit that ends the call with a named wait also pauses the limiter for that long.
    pub async fn post_note(
        &self,
        token: &Token,
        issue: IssueNumber,
        key: &str,
        body: &str,
    ) -> Result<Comment, CommentError> {
        let key = key.trim();
        if key.is_empty()
            || key == NOTE_MARKER
            || key == STEER_MARKER
            || !has_line(body, NOTE_MARKER)
            || !has_line(body, key)
        {
            return Err(CommentError::Unmarked);
        }
        while let Err(wait) = self.limiter.try_acquire() {
            tokio::time::sleep(wait).await;
        }
        let since = format_utc_seconds(self.clock.now().saturating_sub(PROBE_MARGIN.as_secs()));
        let path = format!("{}/comments", self.issue_path(issue.0));
        let since = since.as_str();
        let probe: Probe<'_> = Box::new(move || Box::pin(self.probe(token, issue.0, key, since)));
        let created = self
            .rest
            .create(&path, token, &serde_json::json!({ "body": body }), probe)
            .await;
        let reply = match created {
            Ok(reply) => reply,
            Err(exhausted) => {
                if let Failure::RateLimited { wait: Some(wait) } = exhausted.last {
                    self.limiter.pause(wait);
                }
                return Err(exhausted.into());
            }
        };
        if !reply.status.is_success() {
            return Err(if reply.status == StatusCode::NOT_FOUND {
                CommentError::NotFound {
                    endpoint: reply.endpoint,
                }
            } else {
                CommentError::Status {
                    endpoint: reply.endpoint,
                    status: reply.status.as_u16(),
                }
            });
        }
        let unexpected = || CommentError::Unexpected {
            endpoint: reply.endpoint.clone(),
        };
        let comment = serde_json::from_slice::<Value>(&reply.body).map_err(|_| unexpected())?;
        map_comment(&comment, &reply.endpoint).map_err(|_| unexpected())
    }

    /// Whether an earlier attempt to post the note keyed `key` on `issue` landed: the newest of
    /// the bot's notes created at or after `since` that carry the key line, by creation time and
    /// then id. Any failed read is [`Landing::Unknown`].
    async fn probe(&self, token: &Token, issue: u64, key: &str, since: &str) -> Landing {
        let base = format!(
            "{}/comments?since={}",
            self.issue_path(issue),
            path_segment(since)
        );
        let mut newest: Option<(Comment, Value, Reply)> = None;
        for number in 1..=MAX_PAGES {
            let path = format!("{base}&per_page={PER_PAGE}&page={number}");
            let Ok(reply) = self.rest.get(&path, token, None).await else {
                return Landing::Unknown;
            };
            if !reply.status.is_success() {
                return Landing::Unknown;
            }
            let Ok(Value::Array(items)) = serde_json::from_slice::<Value>(&reply.body) else {
                return Landing::Unknown;
            };
            for item in &items {
                let Ok(comment) = map_comment(item, &reply.endpoint) else {
                    return Landing::Unknown;
                };
                let matches = comment_mark(&comment, &[], &self.bot) == Some(CommentMark::Note)
                    && comment.created_at.as_str() >= since
                    && has_line(&comment.body, key);
                let newer = newest.as_ref().is_none_or(|(best, _, _)| {
                    (comment.created_at.as_str(), comment.id) > (best.created_at.as_str(), best.id)
                });
                if matches && newer {
                    newest = Some((comment, item.clone(), reply.clone()));
                }
            }
            if !reply.next_page.unwrap_or(items.len() >= PER_PAGE) {
                return match newest {
                    Some((_, item, reply)) => match serde_json::to_vec(&item) {
                        Ok(body) => Landing::Landed(Reply { body, ..reply }),
                        Err(_) => Landing::Unknown,
                    },
                    None => Landing::NotLanded,
                };
            }
        }
        // More pages than a read takes: the note may be on one not read.
        Landing::Unknown
    }

    fn issue_path(&self, issue: u64) -> String {
        format!("/repos/{}/{}/issues/{issue}", self.owner, self.repo)
    }
}

/// Whether one of `body`'s lines, split at LF and trimmed, is `line`, as [`comment_mark`] reads
/// markers.
fn has_line(body: &str, line: &str) -> bool {
    body.split('\n').any(|candidate| candidate.trim() == line)
}
