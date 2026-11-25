//! Retrying `git fetch` after a transient failure, with the full-jitter backoff of
//! [`RetryPolicy`].
//!
//! Each failed attempt is sorted by its exit status and, for exit code 128, by our own reading
//! of git's standard error under `LC_ALL=C`:
//!
//! - any exit code but 0 and 128, and exit code 128 with a [`PERMANENT_MARKERS`] match, is
//!   permanent: it is returned at once, for the caller to turn into [`GitError::Git`];
//! - exit code 128 with a [`NETWORK_MARKERS`] match is [`FetchFailure::Network`], retried up to
//!   the policy's attempts;
//! - exit code 128 matching neither list is [`FetchFailure::Unknown`], and no exit code
//!   [`FetchFailure::Signal`]; both are retried, but only up to [`UNKNOWN_ATTEMPTS`].
//!
//! The waits block the calling thread. The policy's `total` bounds only the waits: a wait that
//! would end after it is not started, and a running fetch is never cut off.
//!
//! When an unknown or signal failure runs out its attempts, the loop logs a warning and the error
//! carries an [`UnrecognisedFailure`]: a redacted excerpt and its signature, for the caller to
//! record in the runner's store.

use std::fmt;
use std::hash::BuildHasher;
use std::process::Output;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use mintswarm_domain::retry::RetryPolicy;
use sha2::{Digest, Sha256};

use super::GitError;
use super::env::{status_text, stderr_start};

/// Phrases of git and curl, under `LC_ALL=C`, that make an exit code 128 permanent: failed
/// authentication, a repository that is not found, and a missing ref or id. A `…` stands for
/// any text within one line. They are checked before [`NETWORK_MARKERS`].
pub(super) const PERMANENT_MARKERS: &[&str] = &[
    // Authentication.
    "Authentication failed",
    "could not read Username",
    "could not read Password",
    "returned error: 401",
    "returned error: 403",
    // Not found.
    "Repository not found",
    "returned error: 404",
    "repository '…' not found",
    // Missing ref or id.
    "couldn't find remote ref",
    "not our ref",
];

/// Phrases of git and curl, under `LC_ALL=C`, that make an exit code 128 a network failure:
/// a connection that could not be made or broke, and a server error.
pub(super) const NETWORK_MARKERS: &[&str] = &[
    // Connection.
    "Could not resolve host",
    "Failed to connect",
    "Connection timed out",
    "Connection reset",
    "Operation timed out",
    "early EOF",
    "the remote end hung up unexpectedly",
    "RPC failed",
    // Server errors.
    "returned error: 500",
    "returned error: 502",
    "returned error: 503",
    "returned error: 504",
];

/// The attempts an unknown or signal failure gets at most, the first included; never more than
/// the policy's. Two is the smallest cap that still retries: one retry rides out a one-off blip
/// that git words in a way we do not know, and an unlisted permanent failure costs one extra
/// attempt and at most the policy's `base_delay` of waiting.
pub(super) const UNKNOWN_ATTEMPTS: u32 = 2;

/// The longest excerpt of an [`UnrecognisedFailure`], in characters.
const EXCERPT_LIMIT: usize = 200;

/// The shortest run of hexadecimal digits an excerpt reads as an id.
const ID_MIN: usize = 7;

/// How a fetch attempt that may be retried failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FetchFailure {
    /// Exit code 128 with a [`NETWORK_MARKERS`] phrase.
    Network,
    /// Exit code 128 with no phrase of either list.
    Unknown,
    /// No exit code: git was stopped by a signal.
    Signal,
}

impl fmt::Display for FetchFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Network => "a network failure",
            Self::Unknown => "an unrecognised failure",
            Self::Signal => "a signal",
        })
    }
}

/// A failure the classifier did not recognise, redacted for logs and the runner's store.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnrecognisedFailure {
    /// The sha256 of `excerpt`, as 64 lowercase hexadecimal digits.
    pub signature: String,
    /// The start of git's standard error with the credential, URLs, ids and numbers replaced by
    /// `[redacted]`, `[url]`, `[id]` and `[n]`, cut to 200 characters.
    pub excerpt: String,
    /// The exit status, in our words: `exit code 128` or `no exit code`.
    pub status: String,
}

/// What one attempt's output means for the loop.
#[derive(Debug, PartialEq, Eq)]
enum Class {
    /// Success, or a permanent failure: the loop returns the output.
    Done,
    /// A failure that may be retried.
    Retry(FetchFailure),
}

/// Sort an attempt's output; see the module documentation.
fn classify(output: &Output) -> Class {
    match output.status.code() {
        None => Class::Retry(FetchFailure::Signal),
        Some(128) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            if matches_any(&stderr, PERMANENT_MARKERS) {
                Class::Done
            } else if matches_any(&stderr, NETWORK_MARKERS) {
                Class::Retry(FetchFailure::Network)
            } else {
                Class::Retry(FetchFailure::Unknown)
            }
        }
        Some(_) => Class::Done,
    }
}

/// Whether any line of `text` matches any of `markers`.
fn matches_any(text: &str, markers: &[&str]) -> bool {
    text.lines()
        .any(|line| markers.iter().any(|marker| matches_marker(line, marker)))
}

/// Whether `line` holds the parts of `marker`, split at `…`, in order.
fn matches_marker(line: &str, marker: &str) -> bool {
    let mut rest = line;
    for part in marker.split('…') {
        match rest.find(part) {
            Some(at) => rest = &rest[at + part.len()..],
            None => return false,
        }
    }
    true
}

/// The attempts `failure` gets at most under `policy`.
fn cap(policy: &RetryPolicy, failure: FetchFailure) -> u32 {
    let attempts = policy.attempts.max(1);
    match failure {
        FetchFailure::Network => attempts,
        FetchFailure::Unknown | FetchFailure::Signal => attempts.min(UNKNOWN_ATTEMPTS),
    }
}

/// Run `attempt` until it succeeds, fails permanently, or its class's attempts or the policy's
/// total run out.
///
/// A success or a permanent failure returns the attempt's output; an attempt that cannot run
/// returns its error at once. Before retry `n`, counted from 1, the loop sleeps
/// `policy.jittered(n, jitter())`, unless that wait would end after `policy.total`. `token` is
/// the credential the attempt passes to git, scrubbed from an unrecognised failure's excerpt.
pub(super) fn run(
    policy: &RetryPolicy,
    token: Option<&str>,
    mut attempt: impl FnMut() -> Result<Output, GitError>,
    mut jitter: impl FnMut() -> f64,
    mut sleep: impl FnMut(Duration),
) -> Result<Output, GitError> {
    let start = Instant::now();
    let mut slept = Duration::ZERO;
    let mut attempts = 0u32;
    loop {
        attempts += 1;
        let output = attempt()?;
        let failure = match classify(&output) {
            Class::Done => return Ok(output),
            Class::Retry(failure) => failure,
        };
        tracing::debug!(
            attempt = attempts,
            failure = %failure,
            status = %status_text(&output),
            "git fetch failed"
        );
        if attempts >= cap(policy, failure) {
            return Err(give_up(attempts, failure, &output, token));
        }
        let wait = policy.jittered(attempts, jitter());
        // The time spent so far: the real time, or the waits asked for when `sleep` does not
        // really sleep, whichever is longer.
        let elapsed = start.elapsed().max(slept);
        if elapsed.saturating_add(wait) > policy.total {
            return Err(give_up(attempts, failure, &output, token));
        }
        sleep(wait);
        slept = slept.saturating_add(wait);
    }
}

/// The error for a fetch that gave up after `attempts`, the last failing as `last` with
/// `output`; an unknown or signal failure is logged as a warning and carried, redacted.
fn give_up(attempts: u32, last: FetchFailure, output: &Output, token: Option<&str>) -> GitError {
    let unrecognised = match last {
        FetchFailure::Network => None,
        FetchFailure::Unknown | FetchFailure::Signal => {
            let record = unrecognised(output, token);
            tracing::warn!(
                attempts,
                status = %record.status,
                signature = %record.signature,
                excerpt = %record.excerpt,
                "git fetch gave up on a failure it does not recognise"
            );
            Some(Box::new(record))
        }
    };
    GitError::FetchExhausted {
        attempts,
        last,
        status: status_text(output),
        stderr: stderr_start(output),
        unrecognised,
    }
}

/// The redacted record of an unrecognised failure; see [`UnrecognisedFailure`].
///
/// The whole of standard error is cleaned before it is cut, so a cut never leaves part of a
/// credential or URL behind.
pub(super) fn unrecognised(output: &Output, token: Option<&str>) -> UnrecognisedFailure {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let mut text = stderr.trim().to_owned();
    if let Some(token) = token.filter(|token| !token.is_empty()) {
        text = text.replace(token, "[redacted]");
    }
    let text = replace_urls(&text);
    let text = replace_runs(&text, |c| c.is_ascii_hexdigit(), ID_MIN, "[id]");
    let text = replace_runs(&text, |c| c.is_ascii_digit(), 1, "[n]");
    let excerpt: String = text.chars().take(EXCERPT_LIMIT).collect();
    UnrecognisedFailure {
        signature: hex_sha256(excerpt.as_bytes()),
        excerpt,
        status: status_text(output),
    }
}

/// `text` with every `scheme://…` run, up to whitespace or a quote, replaced by `[url]`.
fn replace_urls(text: &str) -> String {
    let is_scheme = |c: char| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.');
    let ends_url = |c: char| c.is_whitespace() || matches!(c, '\'' | '"' | '`' | '<' | '>');
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find("://") {
        let before = &rest[..at];
        let scheme_start = before
            .char_indices()
            .rev()
            .take_while(|&(_, c)| is_scheme(c))
            .last()
            .map_or(at, |(index, _)| index);
        let after = &rest[at + 3..];
        if scheme_start == at {
            // No scheme before `://`: keep it and look further on.
            out.push_str(&rest[..at + 3]);
            rest = after;
            continue;
        }
        let end = after.find(ends_url).unwrap_or(after.len());
        out.push_str(&rest[..scheme_start]);
        out.push_str("[url]");
        rest = &after[end..];
    }
    out.push_str(rest);
    out
}

/// `text` with every run of at least `min` characters that `keep` accepts replaced by `with`.
fn replace_runs(text: &str, keep: impl Fn(char) -> bool, min: usize, with: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut run = String::new();
    let flush = |run: &mut String, out: &mut String| {
        if run.chars().count() >= min {
            out.push_str(with);
        } else {
            out.push_str(run);
        }
        run.clear();
    };
    for c in text.chars() {
        if keep(c) {
            run.push(c);
        } else {
            flush(&mut run, &mut out);
            out.push(c);
        }
    }
    flush(&mut run, &mut out);
    out
}

/// The sha256 of `bytes`, as 64 lowercase hexadecimal digits.
fn hex_sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// A draw from `[0, 1)`: a hash of a process-wide counter under a randomly keyed
/// [`RandomState`](std::hash::RandomState). It only spreads retries apart; it is not for
/// secrets. A copy of the GitHub client's, which this crate does not depend on.
pub(super) fn random_unit() -> f64 {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    static STATE: OnceLock<std::hash::RandomState> = OnceLock::new();
    let draw = STATE
        .get_or_init(std::hash::RandomState::new)
        .hash_one(COUNTER.fetch_add(1, Ordering::Relaxed));
    // The top 53 bits, which an f64 holds exactly, over 2^53.
    (draw >> 11) as f64 / (1u64 << 53) as f64
}
