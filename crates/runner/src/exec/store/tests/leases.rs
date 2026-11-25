//! Tests of leases on a manual clock: expiry at the deadline and renewal by the holder only.

use mintswarm_domain::ids::{AttemptId, JobId};

use super::super::{LeaseResource, OutboxMode, Store, StoreError};
use super::{ManualClock, fresh_state};

/// The time to live of the leases in these tests.
const TTL: i64 = 60_000;

/// The clock's starting time.
const START: i64 = 1_000_000;

#[test]
fn resources_render_as_in_section_12() {
    let rendered: Vec<String> = [
        LeaseResource::Job(JobId("j1".to_owned())),
        LeaseResource::Attempt(AttemptId("a1".to_owned())),
        LeaseResource::Worktree("w1".to_owned()),
        LeaseResource::Snapshot("s1".to_owned()),
        LeaseResource::StoreFetch,
    ]
    .iter()
    .map(ToString::to_string)
    .collect();
    assert_eq!(
        rendered,
        [
            "job:j1",
            "attempt:a1",
            "worktree:w1",
            "snapshot:s1",
            "store-fetch"
        ]
    );
}

#[test]
fn a_lease_expires_at_its_deadline() {
    let (_tmp, state) = fresh_state();
    let clock = ManualClock::at(START);
    let store = Store::open(&state, clock.clone(), OutboxMode::Live).unwrap();
    let job = LeaseResource::Job(JobId("j1".to_owned()));

    let lease = store.acquire(&job, "alpha", TTL).unwrap();
    assert_eq!(lease.expires_at, START + TTL);
    assert_eq!(store.lease(&job).unwrap(), Some(lease.clone()));

    let err = store.acquire(&job, "alpha", TTL).unwrap_err();
    assert!(matches!(err, StoreError::LeaseHeld(_)), "{err:?}");

    clock.set(START + TTL - 1);
    let err = store.acquire(&job, "beta", TTL).unwrap_err();
    assert!(matches!(err, StoreError::LeaseHeld(_)), "{err:?}");
    assert_eq!(store.lease(&job).unwrap(), Some(lease));

    clock.set(START + TTL);
    assert_eq!(store.lease(&job).unwrap(), None);
    let err = store.renew(&job, "alpha", TTL).unwrap_err();
    assert!(matches!(err, StoreError::LeaseNotHeld(_)), "{err:?}");
    let taken = store.acquire(&job, "beta", TTL).unwrap();
    assert_eq!(taken.holder, "beta");
    assert_eq!(taken.expires_at, START + 2 * TTL);
    assert_eq!(store.lease(&job).unwrap(), Some(taken));
}

#[test]
fn only_the_holder_renews_a_lease() {
    let (_tmp, state) = fresh_state();
    let clock = ManualClock::at(START);
    let store = Store::open(&state, clock.clone(), OutboxMode::Live).unwrap();
    let fetch = LeaseResource::StoreFetch;
    store.acquire(&fetch, "alpha", TTL).unwrap();

    clock.set(START + 10_000);
    let err = store.renew(&fetch, "beta", TTL).unwrap_err();
    assert!(matches!(err, StoreError::LeaseNotHeld(_)), "{err:?}");
    let lease = store.lease(&fetch).unwrap().unwrap();
    assert_eq!(
        (lease.holder.as_str(), lease.expires_at),
        ("alpha", START + TTL)
    );

    let renewed = store.renew(&fetch, "alpha", TTL).unwrap();
    assert_eq!(renewed.expires_at, START + 10_000 + TTL);
    assert_eq!(store.lease(&fetch).unwrap(), Some(renewed));
}

#[test]
fn only_the_holder_releases_a_lease() {
    let (_tmp, state) = fresh_state();
    let clock = ManualClock::at(START);
    let store = Store::open(&state, clock, OutboxMode::Live).unwrap();
    let worktree = LeaseResource::Worktree("w1".to_owned());
    store.acquire(&worktree, "alpha", TTL).unwrap();
    let err = store.release(&worktree, "beta").unwrap_err();
    assert!(matches!(err, StoreError::LeaseNotHeld(_)), "{err:?}");
    store.release(&worktree, "alpha").unwrap();
    assert_eq!(store.lease(&worktree).unwrap(), None);
    store.acquire(&worktree, "beta", TTL).unwrap();
}

#[test]
fn a_lease_needs_a_positive_time_to_live() {
    let (_tmp, state) = fresh_state();
    let store = Store::open(&state, ManualClock::at(START), OutboxMode::Live).unwrap();
    for ttl in [0, -1] {
        let err = store
            .acquire(&LeaseResource::StoreFetch, "alpha", ttl)
            .unwrap_err();
        assert!(matches!(err, StoreError::LeaseTtl(_)), "{err:?}");
    }
}
