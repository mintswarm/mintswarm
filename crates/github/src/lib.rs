//! GitHub access for the runner: it acts as the runner App with an RS256 App JWT and
//! installation tokens narrowed to one repository and the permissions each use asks for, each at
//! read or write level.
//! The App's private key is read from the key store its configuration names, for each JWT, and
//! dropped after signing. [`FieldClient`] and [`GitHubFieldCatalogue`] read and write a role's
//! issue field or labels with a token their caller passes in.
//!
//! [`GitHubReads`] is the read-only view of one repository, design §9 "Reads and writes":
//! issues with their comments since a cursor, an issue's parent and its sub-issue links, a
//! login's role on the repository as a [`RepoRole`](mintswarm_domain::tracker::RepoRole), the
//! issues that block an issue or that it blocks, a listing of issues since a time with GitHub's
//! clock at its first reply, issue artifacts, and pull requests with their reviews, review
//! comments, review threads (a read-only GraphQL query), requested reviewers and changed files,
//! and the check runs, check suites and statuses on a commit, paged, revalidated with ETags and
//! paused by `retry-after`. [`comment_mark`] tells steers and notes from other comments.
//! [`GitHubRepoReads`] reads the default branch's head, a file's bytes or text and the
//! CODEOWNERS file in force at a commit through the same GET path, so the plane needs no clone.
//!
//! [`MergeClient`] squash-merges a pull request pinned to a head sha with a token its caller
//! passes in; its PUT is never sent again after an attempt that may have landed, and GitHub's
//! refusals come back as [`MergeError`] classes.
//!
//! [`StatusClient`] reads an issue's status name and sets it by name through GraphQL, querying
//! the repository's statuses once per [`StatusPass`]; a GraphQL reply with `errors` is a failure,
//! and one with a `RATE_LIMITED` error a rate limit. [`CommentClient`] posts notes once each: a
//! create whose attempt may have landed is answered by a probe for the note's key, and
//! [`ContentLimiter`] budgets every [`CommentClient::post_note`]. [`IssueWrites`] reacts `+1` to
//! a comment, edits a comment and updates an artifact, each attempt budgeted by the same limiter,
//! and a 403 that is not a rate limit is [`WriteError::Refused`].
//!
//! Every request goes through one retry layer: a transient failure is retried within a
//! [`RetryPolicy`], with bounded, jittered backoff and the waits a rate limit names, and a call
//! whose retries run out ends in [`Exhausted`].

mod catalogue;
mod comments;
mod config;
mod etag;
mod fields;
mod jwt;
mod key_store;
mod limit;
mod merges;
mod pulls;
mod reads;
mod repo;
mod rest;
mod retry;
mod secret;
mod status;
mod token;
mod writes;

pub use catalogue::GitHubFieldCatalogue;
pub use comments::{CommentClient, CommentError, PROBE_MARGIN};
pub use config::{AppConfig, ConfigError, KeyStore};
pub use fields::{FieldClient, FieldPass};
pub use jwt::{JWT_BACKDATE_SECS, JWT_LIFETIME_SECS, JwtError, app_jwt};
pub use key_store::{DEFAULT_TIMEOUT, KeyStoreEnv, KeyStoreError, Tools, read_key};
pub use limit::{CONTENT_PER_HOUR, CONTENT_PER_MINUTE, ContentLimiter};
pub use merges::{MergeClient, MergeError};
pub use mintswarm_domain::retry::{Exhausted, Failure, RetryPolicy};
pub use reads::{
    GitHubReads, IssueList, IssueQuery, IssueState, KindSource, Kinds, MAX_PAGES, NOTE_MARKER,
    PER_PAGE, ReadError, STEER_MARKER, comment_mark,
};
pub use repo::GitHubRepoReads;
pub use rest::ClientError;
pub use retry::SECONDARY_LIMIT_WAIT;
pub use secret::{SecretBytes, Token};
pub use status::{StatusClient, StatusError, StatusPass};
pub use token::{
    Access, CONNECT_TIMEOUT, Clock, GITHUB_API, Permissions, REFRESH_MARGIN_SECS, REQUEST_TIMEOUT,
    TokenClient, TokenError,
};
pub use writes::{IssueWrites, WriteError};
