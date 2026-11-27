//! The watch store specification, `mintswarm_core::testing::watch_log`, run against
//! `MemoryWatchLog`, and the memory store's own guarantees.

#![cfg(test)]

use std::time::{Duration, UNIX_EPOCH};

use mintswarm_core::testing::watch_log as spec;
use mintswarm_core::watch::{AppendOutcome, MemoryWatchLog, Retention, WatchLog};
use mintswarm_proto::watch::v1::WatchEvent;

async fn memory() -> MemoryWatchLog {
    MemoryWatchLog::new()
}

#[tokio::test]
async fn prune_keeps_only_events_within_both_bounds() {
    spec::prune_keeps_only_events_within_both_bounds(memory).await;
}

#[tokio::test]
async fn cursor_below_pruned_through_is_expired() {
    spec::cursor_below_pruned_through_is_expired(memory).await;
}

#[tokio::test]
async fn cursor_at_pruned_through_resumes_without_gap() {
    spec::cursor_at_pruned_through_resumes_without_gap(memory).await;
}

#[tokio::test]
async fn cursor_of_another_epoch_reports_store_changed() {
    spec::cursor_of_another_epoch_reports_store_changed(memory).await;
}

#[tokio::test]
async fn read_events_carry_epoch_and_id_cursors() {
    spec::read_events_carry_epoch_and_id_cursors(memory).await;
}

#[tokio::test]
async fn duplicate_delivery_stores_nothing() {
    spec::duplicate_delivery_stores_nothing(memory).await;
}

#[tokio::test]
async fn payload_round_trips_and_is_pruned_with_its_event() {
    spec::payload_round_trips_and_is_pruned_with_its_event(memory).await;
}

#[tokio::test]
async fn read_after_filters_before_the_limit_in_id_order() {
    spec::read_after_filters_before_the_limit_in_id_order(memory).await;
}

#[tokio::test]
async fn pruned_delivery_is_stored_anew() {
    spec::pruned_delivery_is_stored_anew(memory).await;
}

#[tokio::test]
async fn empty_store_has_no_head_or_oldest() {
    spec::empty_store_has_no_head_or_oldest(memory).await;
}

#[tokio::test]
async fn prune_removes_an_id_prefix_when_at_falls() {
    spec::prune_removes_an_id_prefix_when_at_falls(memory).await;
}

#[tokio::test]
async fn a_prune_that_removes_nothing_leaves_pruned_through_unset() {
    spec::a_prune_that_removes_nothing_leaves_pruned_through_unset(memory).await;
}

#[tokio::test]
async fn a_prune_that_removes_nothing_keeps_pruned_through() {
    spec::a_prune_that_removes_nothing_keeps_pruned_through(memory).await;
}

#[tokio::test]
async fn emptied_store_expires_with_no_pruned_before() {
    spec::emptied_store_expires_with_no_pruned_before(memory).await;
}

#[tokio::test]
async fn append_after_an_emptying_prune_follows_pruned_through() {
    spec::append_after_an_emptying_prune_follows_pruned_through(memory).await;
}

#[test]
fn the_epoch_is_a_fixed_uuid_v7_per_store() {
    let store = MemoryWatchLog::new();
    assert_eq!(store.epoch().get_version_num(), 7);
    assert_eq!(store.epoch(), store.epoch());
    assert_ne!(store.epoch(), MemoryWatchLog::default().epoch());
}

#[tokio::test]
async fn a_future_dropped_unpolled_changes_nothing() {
    let store = MemoryWatchLog::new();
    let at = UNIX_EPOCH + Duration::from_secs(1_800_000_000);
    drop(store.append(WatchEvent::default(), Vec::new(), None, at));
    assert_eq!(store.head().await.expect("head"), None);

    let id = match store
        .append(WatchEvent::default(), Vec::new(), None, at)
        .await
        .expect("append")
    {
        AppendOutcome::Appended(id) => id,
        AppendOutcome::Duplicate => panic!("an append without a delivery is a duplicate"),
    };
    let retention = Retention::new(Duration::from_secs(3600), 1).expect("valid bounds");
    drop(store.prune(&retention, at + Duration::from_secs(7200)));
    let head = store.head().await.expect("head").expect("a stored event");
    assert_eq!(head.id(), id);
}
