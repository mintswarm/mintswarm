//! How often and how long a call to an outside service is retried after a transient failure,
//! and the error a call gives when its retries run out.
//!
//! Each wait is full jitter over an exponential ceiling, as the AWS SDKs and Tools reference
//! guide, "Retry behavior"
//! (<https://docs.aws.amazon.com/sdkref/latest/guide/feature-retry-behavior.html>), gives it:
//! `random(0, 1) × min(max_delay, base_delay × 2^retry)`, with `retry` counted from 0 for the
//! first retry. [`RetryPolicy::ceiling`] counts retries from 1, so its `n` is that `retry` plus 1.

use std::fmt;
use std::time::Duration;

/// How many attempts a call makes at most, the first included: Octokit `plugin-retry`
/// (<https://github.com/octokit/plugin-retry.js>, `src/index.ts`) retries 3 times by default
/// (`retries: 3`), plus the first attempt.
pub const DEFAULT_ATTEMPTS: u32 = 4;

/// The ceiling of the first retry's wait, doubled for each later one: Octokit `plugin-retry`'s
/// `retryAfterBaseValue: 1000` (`src/index.ts`), read as milliseconds, which is how the plugin
/// uses it; the source states no unit.
pub const DEFAULT_BASE_DELAY: Duration = Duration::from_secs(1);

/// The cap on any one wait's ceiling: the AWS SDKs and Tools reference guide, "Retry behavior"
/// (<https://docs.aws.amazon.com/sdkref/latest/guide/feature-retry-behavior.html>): "The maximum
/// backoff cap is 20 seconds". That page describes the retry behaviour the SDKs offer as an
/// opt-in in 2026.
pub const DEFAULT_MAX_DELAY: Duration = Duration::from_secs(20);

/// How long a call may take in all, its attempts and waits together; an attempt still running
/// when it ends is cut off as a [`Failure::Timeout`]. It is [`DEFAULT_ATTEMPTS`] (4) times the
/// 30 s that one GitHub request may take (`REQUEST_TIMEOUT` in `mintswarm-github`, whose tests
/// check this sum against it), plus the capped ceilings of the three waits between them,
/// 1 + 2 + 4 s. It also leaves room for one 60 s secondary rate-limit wait after a first
/// refusal that came back fast.
pub const DEFAULT_TOTAL: Duration = Duration::from_secs(127);

/// The bounds of a call's retries: at most `attempts` attempts within `total`, each wait drawn
/// from the full-jitter backoff that starts at `base_delay` and is capped at `max_delay`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RetryPolicy {
    /// How many attempts a call makes at most, the first included; 1 or more.
    pub attempts: u32,
    /// The ceiling of the first retry's wait.
    pub base_delay: Duration,
    /// The cap on any wait's ceiling; not below `base_delay`.
    pub max_delay: Duration,
    /// How long a call may take in all: an attempt still running when it ends is cut off as a
    /// [`Failure::Timeout`], and a wait that would end after it ends the call instead.
    pub total: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            attempts: DEFAULT_ATTEMPTS,
            base_delay: DEFAULT_BASE_DELAY,
            max_delay: DEFAULT_MAX_DELAY,
            total: DEFAULT_TOTAL,
        }
    }
}

impl RetryPolicy {
    /// The un-jittered wait before retry `n`, counted from 1: `min(max_delay, base_delay ×
    /// 2^(n-1))`. Retry 0 has the same ceiling as retry 1.
    pub fn ceiling(&self, n: u32) -> Duration {
        1u32.checked_shl(n.saturating_sub(1))
            .and_then(|factor| self.base_delay.checked_mul(factor))
            .map_or(self.max_delay, |delay| delay.min(self.max_delay))
    }

    /// The wait before retry `n` for a draw `unit` from `[0, 1]`: [`ceiling`](Self::ceiling)
    /// scaled by `unit`, the full jitter of the module's formula. A `unit` outside `[0, 1]` is
    /// clamped into it.
    pub fn jittered(&self, n: u32, unit: f64) -> Duration {
        let unit = if unit.is_nan() {
            0.0
        } else {
            unit.clamp(0.0, 1.0)
        };
        self.ceiling(n).mul_f64(unit)
    }
}

/// How one attempt failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Failure {
    /// The service answered with this server-error status.
    Status(u16),
    /// The attempt took longer than it may.
    Timeout,
    /// No connection could be made, so nothing was sent.
    Connect,
    /// The connection broke after the request may have been sent.
    Reset,
    /// The answer's body could not be read.
    Body,
    /// The request could not be built, so nothing was sent, and every attempt would fail the
    /// same way; it is never retried.
    Request,
    /// The service refused the request for a rate limit.
    RateLimited {
        /// The wait the service asked for, if it named one.
        wait: Option<Duration>,
    },
}

impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Status(status) => write!(f, "status {status}"),
            Self::Timeout => f.write_str("a timeout"),
            Self::Connect => f.write_str("no connection"),
            Self::Reset => f.write_str("a broken connection"),
            Self::Body => f.write_str("an unreadable answer"),
            Self::Request => f.write_str("a request that could not be built"),
            Self::RateLimited { wait: Some(wait) } => {
                write!(f, "a rate limit asking for {}s", wait.as_secs())
            }
            Self::RateLimited { wait: None } => f.write_str("a rate limit"),
        }
    }
}

/// A call that gave up: its attempts or its time ran out while it kept failing transiently, or
/// an attempt's request could not be built ([`Failure::Request`]), which ends the call at that
/// attempt, whichever it is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Exhausted {
    /// The method and path of the call; never a credential.
    pub endpoint: String,
    /// How many attempts were made; one that ended in [`Failure::Request`] is counted though
    /// nothing was sent, and 0 when a rate-limit pause already running did not fit the call's
    /// time.
    pub attempts: u32,
    /// How the last attempt failed.
    pub last: Failure,
}

impl fmt::Display for Exhausted {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}: gave up after {} attempts, the last ending in {}",
            self.endpoint, self.attempts, self.last
        )
    }
}

impl std::error::Error for Exhausted {}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{
        DEFAULT_ATTEMPTS, DEFAULT_BASE_DELAY, DEFAULT_MAX_DELAY, DEFAULT_TOTAL, RetryPolicy,
    };

    #[test]
    fn delays_double_up_to_the_cap() {
        let policy = RetryPolicy::default();
        let seconds: Vec<u64> = (1..=7).map(|n| policy.ceiling(n).as_secs()).collect();
        assert_eq!(seconds, [1, 2, 4, 8, 16, 20, 20]);
        assert_eq!(policy.ceiling(0), DEFAULT_BASE_DELAY);
        // Past the range of the doubling, the cap still holds.
        assert_eq!(policy.ceiling(40), DEFAULT_MAX_DELAY);
        assert_eq!(policy.ceiling(u32::MAX), DEFAULT_MAX_DELAY);
        assert_eq!(policy.jittered(3, 1.0), Duration::from_secs(4));
        assert_eq!(policy.jittered(3, 0.5), Duration::from_secs(2));
        assert_eq!(policy.jittered(3, 0.0), Duration::ZERO);
        assert_eq!(policy.jittered(3, 7.0), Duration::from_secs(4));
        assert_eq!(policy.jittered(3, f64::NAN), Duration::ZERO);
    }

    #[test]
    fn the_default_total_adds_up() {
        // `REQUEST_TIMEOUT` in `mintswarm-github`, which this crate cannot name.
        let request_timeout = Duration::from_secs(30);
        let policy = RetryPolicy::default();
        let waits: Duration = (1..DEFAULT_ATTEMPTS).map(|n| policy.ceiling(n)).sum();
        assert_eq!(waits, Duration::from_secs(1 + 2 + 4));
        assert_eq!(DEFAULT_TOTAL, request_timeout * DEFAULT_ATTEMPTS + waits);
        // Room for one 60 s secondary-limit wait after a fast first refusal.
        assert!(DEFAULT_TOTAL > Duration::from_secs(60) + request_timeout * 2);
    }
}
