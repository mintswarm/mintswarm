//! The content-creation limiter on a fake clock.
// Not redundant: clippy's `allow-unwrap-in-tests` covers a `tests/` crate only under `cfg(test)`.
#![cfg(test)]

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use mintswarm_github::{Clock, ContentLimiter};

/// A clock the test moves by hand, starting at `start`.
fn clock(start: u64) -> (Clock, Arc<AtomicU64>) {
    let now = Arc::new(AtomicU64::new(start));
    let read = Arc::clone(&now);
    (Clock::from_fn(move || read.load(Ordering::SeqCst)), now)
}

/// Acquires until the first refusal; returns how many were granted and the refusal's wait.
fn drain(limiter: &ContentLimiter) -> (u64, Duration) {
    let mut granted = 0;
    loop {
        match limiter.try_acquire() {
            Ok(()) => granted += 1,
            Err(wait) => return (granted, wait),
        }
        assert!(granted <= 1000, "the limiter never refuses");
    }
}

#[test]
fn eighty_a_minute_then_a_wait() {
    let (clock, now) = clock(1_000);
    let limiter = ContentLimiter::new(clock);
    let (granted, wait) = drain(&limiter);
    assert_eq!(granted, 80);
    // One token of the minute bucket refills in 60 / 80 s.
    assert_eq!(wait, Duration::from_millis(750));

    now.store(1_060, Ordering::SeqCst);
    let (granted, _) = drain(&limiter);
    assert_eq!(granted, 80);
}

#[test]
fn the_hour_bucket_refuses_once_spent() {
    let (clock, now) = clock(0);
    let limiter = ContentLimiter::new(clock);
    let mut total = 0;
    for minute in 0..6 {
        now.store(minute * 60, Ordering::SeqCst);
        let (granted, _) = drain(&limiter);
        assert_eq!(granted, 80, "minute {minute}");
        total += granted;
    }
    // By 360 s the hour bucket holds its 500 and 50 refilled, less the 480 taken: 70 more are
    // granted, though the minute bucket would allow 80, and the refusal waits for the hour
    // bucket's next token, 3600 / 500 s.
    now.store(360, Ordering::SeqCst);
    let (granted, wait) = drain(&limiter);
    assert_eq!(granted, 70);
    assert_eq!(total + granted, 550);
    assert_eq!(wait, Duration::from_millis(7_200));
}

#[test]
fn a_pause_refuses_until_it_ends() {
    let (clock, now) = clock(500);
    let limiter = ContentLimiter::new(clock);
    limiter.pause(Duration::from_secs(30));
    assert_eq!(limiter.try_acquire(), Err(Duration::from_secs(30)));
    now.store(529, Ordering::SeqCst);
    assert_eq!(limiter.try_acquire(), Err(Duration::from_secs(1)));
    // A shorter pause does not end the running one early.
    limiter.pause(Duration::from_secs(0));
    assert_eq!(limiter.try_acquire(), Err(Duration::from_secs(1)));
    now.store(530, Ordering::SeqCst);
    assert_eq!(limiter.try_acquire(), Ok(()));
}
