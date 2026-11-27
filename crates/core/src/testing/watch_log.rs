//! The watch store specification: the cases every [`WatchLog`] passes, whatever its backend.
//!
//! Each case is a `pub async fn` that takes a factory making an empty store, and panics when the
//! store breaks the contract. Every backend's test file runs every case, one test each:
//!
//! - [`prune_keeps_only_events_within_both_bounds`]
//! - [`cursor_below_pruned_through_is_expired`]
//! - [`cursor_at_pruned_through_resumes_without_gap`]
//! - [`cursor_of_another_epoch_reports_store_changed`]
//! - [`read_events_carry_epoch_and_id_cursors`]
//! - [`duplicate_delivery_stores_nothing`]
//! - [`payload_round_trips_and_is_pruned_with_its_event`]
//! - [`read_after_filters_before_the_limit_in_id_order`]
//! - [`pruned_delivery_is_stored_anew`]
//! - [`empty_store_has_no_head_or_oldest`]
//! - [`prune_removes_an_id_prefix_when_at_falls`]
//! - [`a_prune_that_removes_nothing_leaves_pruned_through_unset`]
//! - [`a_prune_that_removes_nothing_keeps_pruned_through`]
//! - [`emptied_store_expires_with_no_pruned_before`]
//! - [`append_after_an_emptying_prune_follows_pruned_through`]
//!
//! Every `at` is a whole second, so a backend that keeps microseconds compares exactly.

use std::future::Future;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use mintswarm_proto::prost_types::Timestamp;
use mintswarm_proto::watch::v1::watch_event::Body;
use mintswarm_proto::watch::v1::{GitHubWebhook, WatchEvent, WatchFilter};
use uuid::Uuid;

use crate::watch::{AppendOutcome, Cursor, Filter, Gap, Page, Pruned, Retention, WatchLog};

/// The prune time of every case: 2027-01-15T08:00:00Z.
fn now() -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(1_800_000_000)
}

const HOUR: Duration = Duration::from_secs(3600);

/// `now()` less `minutes`.
fn minutes_ago(minutes: u64) -> SystemTime {
    now() - Duration::from_secs(minutes * 60)
}

/// A `github.webhook` event with an empty body.
fn webhook() -> WatchEvent {
    WatchEvent {
        kind: "github.webhook".to_owned(),
        summary: "github.webhook".to_owned(),
        body: Some(Body::GithubWebhook(GitHubWebhook::default())),
        ..WatchEvent::default()
    }
}

/// An event of the made-up kind `plane.job`, with no body.
fn plane_event() -> WatchEvent {
    WatchEvent {
        kind: "plane.job".to_owned(),
        ..WatchEvent::default()
    }
}

/// A filter that matches every event the cases append.
fn everything() -> Filter {
    Filter::new(WatchFilter {
        kinds: vec!["github.*".to_owned(), "plane.*".to_owned()],
        ..WatchFilter::default()
    })
    .expect("a valid filter")
}

/// Appends `event` and returns its id; panics on a duplicate.
async fn append<S: WatchLog>(
    store: &S,
    event: WatchEvent,
    payload: &[u8],
    delivery: Option<&str>,
    at: SystemTime,
) -> Uuid {
    let outcome = store
        .append(event, payload.to_vec(), delivery.map(str::to_owned), at)
        .await
        .expect("append");
    match outcome {
        AppendOutcome::Appended(id) => id,
        AppendOutcome::Duplicate => panic!("an append without a stored delivery is a duplicate"),
    }
}

/// Appends a webhook at each of `ats`, in order, and returns their ids.
async fn append_at<S: WatchLog>(store: &S, ats: &[SystemTime]) -> Vec<Uuid> {
    let mut ids = Vec::new();
    for &at in ats {
        ids.push(append(store, webhook(), b"{}", None, at).await);
    }
    ids
}

/// Every event after `after`, without payloads.
async fn read_all<S: WatchLog>(store: &S, after: Option<Cursor>) -> Page {
    store
        .read_after(after, &everything(), usize::MAX, false)
        .await
        .expect("read")
}

/// The ids of `page`'s events.
fn ids(page: &Page) -> Vec<Uuid> {
    page.events
        .iter()
        .map(|event| Uuid::parse_str(&event.id).expect("an event id is a UUID"))
        .collect()
}

/// The cursor of `id` in `store`.
fn cursor<S: WatchLog>(store: &S, id: Uuid) -> Cursor {
    Cursor::new(store.epoch(), id).expect("a valid cursor")
}

async fn prune<S: WatchLog>(store: &S, max_age: Duration, max_events: u64) -> Pruned {
    let retention = Retention::new(max_age, max_events).expect("valid bounds");
    store.prune(&retention, now()).await.expect("prune")
}

/// The fixture of criteria 1 to 3: six events, two at 2 hours old and four at 30 minutes,
/// pruned with bounds of 1 hour and 3 events. The count bound removes the first three, the age
/// bound the first two, so the last three remain. Returns the store and all six ids.
async fn pruned_six<S, F, Fut>(make: F) -> (S, Vec<Uuid>)
where
    S: WatchLog,
    F: FnOnce() -> Fut,
    Fut: Future<Output = S>,
{
    let store = make().await;
    let old = minutes_ago(120);
    let recent = minutes_ago(30);
    let ids = append_at(&store, &[old, old, recent, recent, recent, recent]).await;
    let pruned = prune(&store, HOUR, 3).await;
    assert_eq!(
        pruned,
        Pruned {
            removed: 3,
            pruned_through: Some(ids[2]),
        }
    );
    (store, ids)
}

/// A prune keeps only the events that meet both bounds and sets `pruned_through` to the latest
/// removed id, whether the count bound (six events) or the age bound (four events, three older
/// than the age bound) removes more.
pub async fn prune_keeps_only_events_within_both_bounds<S, F, Fut>(make: F)
where
    S: WatchLog,
    F: Fn() -> Fut,
    Fut: Future<Output = S>,
{
    let (store, all) = pruned_six(&make).await;
    let page = read_all(&store, None).await;
    assert_eq!(ids(&page), all[3..]);
    let limit = Timestamp::from(now() - HOUR);
    for event in &page.events {
        let at = event.at.expect("a read event has `at`");
        assert!((at.seconds, at.nanos) >= (limit.seconds, limit.nanos));
    }
    assert_eq!(
        store.oldest().await.expect("oldest"),
        Some(cursor(&store, all[3]))
    );
    assert_eq!(
        store.head().await.expect("head"),
        Some(cursor(&store, all[5]))
    );

    let store = make().await;
    let old = minutes_ago(120);
    let all = append_at(&store, &[old, old, old, minutes_ago(30)]).await;
    let pruned = prune(&store, HOUR, 3).await;
    assert_eq!(
        pruned,
        Pruned {
            removed: 3,
            pruned_through: Some(all[2]),
        }
    );
    assert_eq!(ids(&read_all(&store, None).await), all[3..]);
}

/// After the prune, a cursor strictly below `pruned_through` is expired, with the oldest stored
/// event's `at`, and the read starts from the oldest event.
pub async fn cursor_below_pruned_through_is_expired<S, F, Fut>(make: F)
where
    S: WatchLog,
    F: FnOnce() -> Fut,
    Fut: Future<Output = S>,
{
    let (store, all) = pruned_six(make).await;
    for &below in &all[..2] {
        let page = read_all(&store, Some(cursor(&store, below))).await;
        assert_eq!(
            page.gap,
            Some(Gap::Expired {
                pruned_before: Some(minutes_ago(30)),
            })
        );
        assert_eq!(ids(&page), all[3..]);
    }
}

/// After the prune, a cursor at `pruned_through` resumes with no gap from the oldest event, and
/// one at a stored id resumes after it.
pub async fn cursor_at_pruned_through_resumes_without_gap<S, F, Fut>(make: F)
where
    S: WatchLog,
    F: FnOnce() -> Fut,
    Fut: Future<Output = S>,
{
    let (store, all) = pruned_six(make).await;
    let page = read_all(&store, Some(cursor(&store, all[2]))).await;
    assert_eq!(page.gap, None);
    assert_eq!(ids(&page), all[3..]);

    let page = read_all(&store, Some(cursor(&store, all[3]))).await;
    assert_eq!(page.gap, None);
    assert_eq!(ids(&page), all[4..]);
}

/// A cursor of another epoch reports the store as changed, and the read starts from the oldest
/// event.
pub async fn cursor_of_another_epoch_reports_store_changed<S, F, Fut>(make: F)
where
    S: WatchLog,
    F: FnOnce() -> Fut,
    Fut: Future<Output = S>,
{
    let store = make().await;
    let all = append_at(&store, &[minutes_ago(3), minutes_ago(2), minutes_ago(1)]).await;
    let epoch = Uuid::now_v7();
    assert_ne!(epoch, store.epoch());
    let other = Cursor::new(epoch, all[1]).expect("a valid cursor");
    let page = read_all(&store, Some(other)).await;
    assert_eq!(page.gap, Some(Gap::StoreChanged));
    assert_eq!(ids(&page), all);
}

/// Every read event's `cursor` is `<store epoch>.<id>`, on a plain read, a resumed read and a
/// read after a gap.
pub async fn read_events_carry_epoch_and_id_cursors<S, F, Fut>(make: F)
where
    S: WatchLog,
    F: FnOnce() -> Fut,
    Fut: Future<Output = S>,
{
    let store = make().await;
    let all = append_at(&store, &[minutes_ago(3), minutes_ago(2), minutes_ago(1)]).await;
    let other = Cursor::new(Uuid::now_v7(), all[0]).expect("a valid cursor");
    let pages = [
        read_all(&store, None).await,
        read_all(&store, Some(cursor(&store, all[0]))).await,
        read_all(&store, Some(other)).await,
    ];
    assert_eq!(pages.iter().map(|page| page.events.len()).sum::<usize>(), 8);
    for event in pages.iter().flat_map(|page| &page.events) {
        assert_eq!(event.cursor, format!("{}.{}", store.epoch(), event.id));
        let parsed = Cursor::parse(&event.cursor).expect("a read cursor parses");
        assert_eq!(parsed.epoch(), store.epoch());
        assert_eq!(parsed.id().to_string(), event.id);
    }
}

/// A second event with a stored delivery stores nothing and reports a duplicate; events
/// without a delivery are never duplicates.
pub async fn duplicate_delivery_stores_nothing<S, F, Fut>(make: F)
where
    S: WatchLog,
    F: FnOnce() -> Fut,
    Fut: Future<Output = S>,
{
    let store = make().await;
    let first = append(&store, webhook(), b"{}", Some("d-1"), minutes_ago(2)).await;
    let outcome = store
        .append(
            webhook(),
            b"{}".to_vec(),
            Some("d-1".to_owned()),
            minutes_ago(1),
        )
        .await
        .expect("append");
    assert_eq!(outcome, AppendOutcome::Duplicate);
    assert_eq!(
        store.head().await.expect("head"),
        Some(cursor(&store, first))
    );
    assert_eq!(ids(&read_all(&store, None).await), [first]);

    let second = append(&store, plane_event(), b"", None, minutes_ago(1)).await;
    let third = append(&store, plane_event(), b"", None, minutes_ago(1)).await;
    assert_eq!(ids(&read_all(&store, None).await), [first, second, third]);
}

/// A raw payload, not UTF-8, reads back byte for byte with `include_payload` and is absent
/// without it; once its event is pruned, no read returns it.
pub async fn payload_round_trips_and_is_pruned_with_its_event<S, F, Fut>(make: F)
where
    S: WatchLog,
    F: FnOnce() -> Fut,
    Fut: Future<Output = S>,
{
    let store = make().await;
    let raw: &[u8] = &[0xff, 0x00, 0xfe, b'{', 0x80];
    let mut event = webhook();
    if let Some(Body::GithubWebhook(webhook)) = &mut event.body {
        // The store keeps the payload it is given apart, never one inside the event.
        webhook.payload = b"inside".to_vec();
    }
    append(&store, event, raw, Some("d-raw"), minutes_ago(2)).await;
    let payloads = |page: &Page| -> Vec<Vec<u8>> {
        page.events
            .iter()
            .map(|event| match &event.body {
                Some(Body::GithubWebhook(webhook)) => webhook.payload.clone(),
                _ => panic!("a webhook reads back without its body"),
            })
            .collect()
    };
    let with = store
        .read_after(None, &everything(), usize::MAX, true)
        .await
        .expect("read");
    assert_eq!(payloads(&with), [raw.to_vec()]);
    assert_eq!(payloads(&read_all(&store, None).await), [Vec::<u8>::new()]);

    append(
        &store,
        webhook(),
        b"{\"later\":1}",
        Some("d-later"),
        minutes_ago(1),
    )
    .await;
    assert_eq!(prune(&store, HOUR, 1).await.removed, 1);
    let with = store
        .read_after(None, &everything(), usize::MAX, true)
        .await
        .expect("read");
    assert_eq!(payloads(&with), [b"{\"later\":1}".to_vec()]);
}

/// The filter is applied before the limit, and events come in id order.
pub async fn read_after_filters_before_the_limit_in_id_order<S, F, Fut>(make: F)
where
    S: WatchLog,
    F: FnOnce() -> Fut,
    Fut: Future<Output = S>,
{
    let store = make().await;
    let mut webhooks = Vec::new();
    for minutes in [5, 4, 3, 2, 1] {
        append(&store, plane_event(), b"", None, minutes_ago(minutes)).await;
        webhooks.push(append(&store, webhook(), b"{}", None, minutes_ago(minutes)).await);
    }
    let github = Filter::new(WatchFilter {
        kinds: vec!["github.*".to_owned()],
        ..WatchFilter::default()
    })
    .expect("a valid filter");
    let page = store
        .read_after(None, &github, 2, false)
        .await
        .expect("read");
    assert_eq!(page.gap, None);
    assert_eq!(ids(&page), webhooks[..2]);

    let after = cursor(&store, webhooks[1]);
    let page = store
        .read_after(Some(after), &github, 10, false)
        .await
        .expect("read");
    assert_eq!(ids(&page), webhooks[2..]);

    let all = ids(&read_all(&store, None).await);
    assert_eq!(all.len(), 10);
    assert!(all.windows(2).all(|pair| pair[0] < pair[1]));
}

/// A delivery whose event was pruned is stored again: deliveries are unique among stored
/// events only.
pub async fn pruned_delivery_is_stored_anew<S, F, Fut>(make: F)
where
    S: WatchLog,
    F: FnOnce() -> Fut,
    Fut: Future<Output = S>,
{
    let store = make().await;
    append(&store, webhook(), b"{}", Some("d-1"), minutes_ago(3)).await;
    let kept = append(&store, webhook(), b"{}", Some("d-2"), minutes_ago(2)).await;
    assert_eq!(prune(&store, HOUR, 1).await.removed, 1);
    let again = append(&store, webhook(), b"{}", Some("d-1"), minutes_ago(1)).await;
    assert_eq!(ids(&read_all(&store, None).await), [kept, again]);
}

/// An empty store has no head or oldest event, reads nothing with no gap, and prunes nothing.
pub async fn empty_store_has_no_head_or_oldest<S, F, Fut>(make: F)
where
    S: WatchLog,
    F: FnOnce() -> Fut,
    Fut: Future<Output = S>,
{
    let store = make().await;
    assert_eq!(store.head().await.expect("head"), None);
    assert_eq!(store.oldest().await.expect("oldest"), None);
    assert_eq!(read_all(&store, None).await, Page::default());
    assert_eq!(
        prune(&store, HOUR, 1).await,
        Pruned {
            removed: 0,
            pruned_through: None,
        }
    );
}

/// A prune removes an id-ordered prefix up to the newest event that breaks a bound, even when
/// `at` does not rise with id: of events at 30, 120 and 30 minutes old, pruned with bounds of
/// 1 hour and 10 events, the first two go and the third stays.
pub async fn prune_removes_an_id_prefix_when_at_falls<S, F, Fut>(make: F)
where
    S: WatchLog,
    F: FnOnce() -> Fut,
    Fut: Future<Output = S>,
{
    let store = make().await;
    let all = append_at(
        &store,
        &[minutes_ago(30), minutes_ago(120), minutes_ago(30)],
    )
    .await;
    assert_eq!(
        prune(&store, HOUR, 10).await,
        Pruned {
            removed: 2,
            pruned_through: Some(all[1]),
        }
    );
    assert_eq!(ids(&read_all(&store, None).await), all[2..]);
    let page = read_all(&store, Some(cursor(&store, all[0]))).await;
    assert_eq!(
        page.gap,
        Some(Gap::Expired {
            pruned_before: Some(minutes_ago(30)),
        })
    );
    assert_eq!(ids(&page), all[2..]);
}

/// A prune that removes nothing from a store never pruned leaves `pruned_through` unset.
pub async fn a_prune_that_removes_nothing_leaves_pruned_through_unset<S, F, Fut>(make: F)
where
    S: WatchLog,
    F: FnOnce() -> Fut,
    Fut: Future<Output = S>,
{
    let store = make().await;
    let all = append_at(&store, &[minutes_ago(2), minutes_ago(1)]).await;
    assert_eq!(
        prune(&store, HOUR, 10).await,
        Pruned {
            removed: 0,
            pruned_through: None,
        }
    );
    assert_eq!(ids(&read_all(&store, None).await), all);
}

/// A prune that removes nothing after one that removed events keeps `pruned_through`, and a
/// cursor below it is still expired.
pub async fn a_prune_that_removes_nothing_keeps_pruned_through<S, F, Fut>(make: F)
where
    S: WatchLog,
    F: FnOnce() -> Fut,
    Fut: Future<Output = S>,
{
    let (store, all) = pruned_six(make).await;
    assert_eq!(
        prune(&store, HOUR, 3).await,
        Pruned {
            removed: 0,
            pruned_through: Some(all[2]),
        }
    );
    let page = read_all(&store, Some(cursor(&store, all[0]))).await;
    assert_eq!(
        page.gap,
        Some(Gap::Expired {
            pruned_before: Some(minutes_ago(30)),
        })
    );
    assert_eq!(ids(&page), all[3..]);
    let page = read_all(&store, Some(cursor(&store, all[2]))).await;
    assert_eq!(page.gap, None);
    assert_eq!(ids(&page), all[3..]);
}

/// An age prune that removes every event sets `pruned_through` to the last of them; a cursor
/// below it is then expired with no `pruned_before`, and one at it reads nothing with no gap.
pub async fn emptied_store_expires_with_no_pruned_before<S, F, Fut>(make: F)
where
    S: WatchLog,
    F: FnOnce() -> Fut,
    Fut: Future<Output = S>,
{
    let store = make().await;
    let all = append_at(&store, &[minutes_ago(120), minutes_ago(90)]).await;
    assert_eq!(
        prune(&store, HOUR, 10).await,
        Pruned {
            removed: 2,
            pruned_through: Some(all[1]),
        }
    );
    assert_eq!(store.head().await.expect("head"), None);
    assert_eq!(store.oldest().await.expect("oldest"), None);
    let page = read_all(&store, Some(cursor(&store, all[0]))).await;
    assert_eq!(
        page,
        Page {
            events: Vec::new(),
            gap: Some(Gap::Expired {
                pruned_before: None,
            }),
        }
    );
    assert_eq!(
        read_all(&store, Some(cursor(&store, all[1]))).await,
        Page::default()
    );
}

/// An event appended after a prune empties the store gets an id above `pruned_through`, so a
/// read from a cursor at `pruned_through` returns it with no gap.
pub async fn append_after_an_emptying_prune_follows_pruned_through<S, F, Fut>(make: F)
where
    S: WatchLog,
    F: FnOnce() -> Fut,
    Fut: Future<Output = S>,
{
    let store = make().await;
    let all = append_at(&store, &[minutes_ago(120), minutes_ago(90)]).await;
    let pruned = prune(&store, HOUR, 10).await;
    assert_eq!(pruned.removed, 2);
    let through = pruned
        .pruned_through
        .expect("an emptying prune sets `pruned_through`");
    assert_eq!(through, all[1]);

    let added = append(&store, webhook(), b"{}", None, minutes_ago(1)).await;
    assert!(added > through);
    let page = read_all(&store, Some(cursor(&store, through))).await;
    assert_eq!(page.gap, None);
    assert_eq!(ids(&page), [added]);
}
