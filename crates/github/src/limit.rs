//! The budget for content-creating requests (comments, issues, artifacts and the like), design
//! §9: GitHub's secondary limits allow about 80 a minute and 500 an hour.
//! [`CommentClient::post_note`](crate::CommentClient::post_note) asks it before each attempt of
//! a note, and [`IssueWrites`](crate::IssueWrites) before each attempt of a reaction, an edit or
//! an update.

use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use crate::token::Clock;

/// At most this many content-creating requests a minute.
pub const CONTENT_PER_MINUTE: u64 = 80;
/// At most this many content-creating requests an hour.
pub const CONTENT_PER_HOUR: u64 = 500;

/// A token bucket of `capacity` tokens refilled linearly over `period` seconds.
///
/// The level is kept in units of `1 / period` token, so a second refills exactly `capacity`
/// units and the arithmetic stays in integers.
#[derive(Debug)]
struct Bucket {
    capacity: u64,
    period: u64,
    /// The level, in units of `1 / period` token.
    level: u64,
    /// The second the level was last brought up to date.
    at: u64,
}

impl Bucket {
    /// A full bucket at `now`.
    fn full(capacity: u64, period: u64, now: u64) -> Self {
        Self {
            capacity,
            period,
            level: capacity * period,
            at: now,
        }
    }

    fn refill(&mut self, now: u64) {
        let elapsed = now.saturating_sub(self.at);
        self.level = self
            .level
            .saturating_add(elapsed.saturating_mul(self.capacity))
            .min(self.capacity * self.period);
        self.at = self.at.max(now);
    }

    /// How long until the bucket holds a whole token; zero when it does.
    fn wait(&self) -> Duration {
        let deficit = self.period.saturating_sub(self.level);
        // Whole milliseconds, rounded up, so a wait never ends before the token is there.
        Duration::from_millis((deficit * 1000).div_ceil(self.capacity))
    }

    fn take(&mut self) {
        self.level -= self.period;
    }
}

#[derive(Debug)]
struct State {
    minute: Bucket,
    hour: Bucket,
    /// The second before which every request is refused, after [`ContentLimiter::pause`].
    paused_until: u64,
}

/// Two token buckets for content-creating requests: [`CONTENT_PER_MINUTE`] a minute and
/// [`CONTENT_PER_HOUR`] an hour, both full at the start and refilled linearly by its clock.
#[derive(Debug)]
pub struct ContentLimiter {
    clock: Clock,
    state: Mutex<State>,
}

impl ContentLimiter {
    /// A limiter with full buckets, timed by `clock`.
    pub fn new(clock: Clock) -> Self {
        let now = clock.now();
        Self {
            state: Mutex::new(State {
                minute: Bucket::full(CONTENT_PER_MINUTE, 60, now),
                hour: Bucket::full(CONTENT_PER_HOUR, 3600, now),
                paused_until: now,
            }),
            clock,
        }
    }

    /// Takes one request from both buckets, or refuses it with how long to wait: the longer of
    /// the two buckets' waits, or what is left of a pause. A refused request takes nothing.
    pub fn try_acquire(&self) -> Result<(), Duration> {
        let now = self.clock.now();
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if now < state.paused_until {
            return Err(Duration::from_secs(state.paused_until - now));
        }
        state.minute.refill(now);
        state.hour.refill(now);
        let wait = state.minute.wait().max(state.hour.wait());
        if !wait.is_zero() {
            return Err(wait);
        }
        state.minute.take();
        state.hour.take();
        Ok(())
    }

    /// Refuses every request until `wait` has passed, for example a `retry-after`. A pause
    /// never ends one already running earlier.
    pub fn pause(&self, wait: Duration) {
        let now = self.clock.now();
        let secs = wait.as_secs() + u64::from(wait.subsec_nanos() > 0);
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        state.paused_until = state.paused_until.max(now.saturating_add(secs));
    }
}
