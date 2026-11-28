//! Keyed comments posted on issues as one App, design §9 "Reads and writes": each attempt of a
//! post is one content-creating request, budgeted by a [`ContentLimiter`], and a post is sent as
//! a create with a probe so an attempt that may have landed is never posted twice.
//!
//! A keyed comment's body carries a key line of its own, a `<!-- mintswarm: … -->` marker such
//! as a decision record's `<!-- mintswarm: answer <id> -->` line. Notes are one kind of keyed
//! comment: their body also carries [`NOTE_MARKER`]. After an ambiguous attempt the probe lists
//! the issue's comments since the call started, less [`PROBE_MARGIN`] for clock skew, and takes
//! the newest comment by the client's own login created since then that carries the key, whatever
//! its mark, as the landed attempt. A probe that cannot read the comments ends the call rather
//! than risk a duplicate.

use std::sync::Arc;
use std::time::Duration;

use reqwest::StatusCode;
use serde_json::Value;

use mintswarm_domain::ids::IssueNumber;
use mintswarm_domain::retry::{Exhausted, Failure, RetryPolicy};
use mintswarm_domain::time::format_utc_seconds;
use mintswarm_domain::tracker::{Comment, Login};

use crate::limit::ContentLimiter;
use crate::reads::{MAX_PAGES, NOTE_MARKER, PER_PAGE, STEER_MARKER, map_comment};
use crate::rest::{ClientError, Reply, Rest, check_names, path_segment};
use crate::retry::{Gate, Landing, Probe};
use crate::secret::Token;
use crate::token::{Clock, Permissions};

/// How far before the call's start the probe looks for a landed comment, for the skew between
/// this host's clock and GitHub's.
pub const PROBE_MARGIN: Duration = Duration::from_secs(10);

/// Why a comment could not be posted. No variant carries a token or a response body.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum CommentError {
    /// The body does not carry its key line, or the key is no marker line, or is a kind marker
    /// ([`NOTE_MARKER`] or [`STEER_MARKER`]) that every comment of its kind carries, so a probe
    /// could not tell the comment from others; or a note lacks [`NOTE_MARKER`]. Nothing was sent.
    #[error(
        "the body does not carry its key line, or the key is no marker line, or is a kind marker"
    )]
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
    /// tell whether an earlier attempt landed; the comment may have been posted.
    #[error(transparent)]
    Exhausted(#[from] Exhausted),
    /// The answer is not JSON of the expected shape.
    #[error("{endpoint}: not the expected JSON")]
    Unexpected {
        /// The method and path.
        endpoint: String,
    },
}

/// Posts keyed comments, notes among them, on the issues of one repository as `bot`.
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

    /// Posts `body` as a note on `issue` with `token`, once: a keyed comment, through
    /// [`post_keyed`](CommentClient::post_keyed), whose body also holds [`NOTE_MARKER`].
    ///
    /// `body` must hold a line equal to [`NOTE_MARKER`], lines split at LF and trimmed, and
    /// `key` must pass [`post_keyed`](CommentClient::post_keyed)'s checks, or the call is
    /// [`CommentError::Unmarked`] before any request.
    pub async fn post_note(
        &self,
        token: &Token,
        issue: IssueNumber,
        key: &str,
        body: &str,
    ) -> Result<Comment, CommentError> {
        if !has_line(body, NOTE_MARKER) {
            return Err(CommentError::Unmarked);
        }
        self.post_keyed(token, issue, key, body).await
    }

    /// Posts `body` as a comment keyed by `key` on `issue` with `token`, once.
    ///
    /// `key` must be a `<!-- mintswarm: … -->` marker line other than [`NOTE_MARKER`] and
    /// [`STEER_MARKER`], and `body` must hold a line equal to it, lines split at LF and trimmed,
    /// or the call is [`CommentError::Unmarked`] before any request. Each attempt of the create
    /// waits until the limiter grants it one request, a wait the create's total cuts off for
    /// every attempt after the first. After an attempt that may have landed, the probe's find is
    /// returned in place of a second post: the newest comment by `bot` that carries the key,
    /// whatever its mark. A rate limit that ends the call with a named wait also pauses the
    /// limiter for that long.
    pub async fn post_keyed(
        &self,
        token: &Token,
        issue: IssueNumber,
        key: &str,
        body: &str,
    ) -> Result<Comment, CommentError> {
        let key = key.trim();
        if !is_key(key) || !has_line(body, key) {
            return Err(CommentError::Unmarked);
        }
        self.grant().await;
        let since = format_utc_seconds(self.clock.now().saturating_sub(PROBE_MARGIN.as_secs()));
        let path = format!("{}/comments", self.issue_path(issue.0));
        let since = since.as_str();
        let probe: Probe<'_> = Box::new(move || Box::pin(self.probe(token, issue.0, key, since)));
        // Every attempt is a content-creating request, so each takes a grant of its own.
        let gate: Gate<'_> = Box::new(move || Box::pin(self.grant()));
        let created = self
            .rest
            .create(
                &path,
                token,
                &serde_json::json!({ "body": body }),
                probe,
                gate,
            )
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

    /// Waits until the limiter grants one request.
    async fn grant(&self) {
        while let Err(wait) = self.limiter.try_acquire() {
            tokio::time::sleep(wait).await;
        }
    }

    /// Whether an earlier attempt to post the comment keyed `key` on `issue` landed: the newest
    /// of the bot's comments created at or after `since` that carry the key line, whatever their
    /// mark, by creation time and then id. Any failed read is [`Landing::Unknown`].
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
                let matches = comment.author.0.eq_ignore_ascii_case(&self.bot.0)
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
        // More pages than a read takes: the comment may be on one not read.
        Landing::Unknown
    }

    fn issue_path(&self, issue: u64) -> String {
        format!("/repos/{}/{}/issues/{issue}", self.owner, self.repo)
    }
}

/// Whether `key` can key a comment: a `<!-- mintswarm: … -->` marker line that is not a kind
/// marker every comment of its kind carries.
fn is_key(key: &str) -> bool {
    key.starts_with("<!-- mintswarm:")
        && key.ends_with("-->")
        && key != NOTE_MARKER
        && key != STEER_MARKER
}

/// Whether one of `body`'s lines, split at LF and trimmed, is `line`, as
/// [`comment_mark`](crate::comment_mark) reads markers.
fn has_line(body: &str, line: &str) -> bool {
    body.split('\n').any(|candidate| candidate.trim() == line)
}
