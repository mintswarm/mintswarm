//! The watch log's store contract, [`WatchLog`], and the outcomes its methods return.

use std::future::Future;
use std::time::SystemTime;

use mintswarm_proto::watch::v1::WatchEvent;
use uuid::Uuid;

use super::{Cursor, Filter, Retention};

/// The result of [`WatchLog::append`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppendOutcome {
    /// The event was stored under this id.
    Appended(Uuid),
    /// A stored event already has the delivery; nothing was stored.
    Duplicate,
}

/// Why a read did not resume right after its cursor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gap {
    /// The cursor names another store epoch: the store was recreated since it was issued.
    StoreChanged,
    /// The cursor's id is below `pruned_through`: events after it were pruned. `pruned_before`
    /// is the oldest stored event's `at`, or `None` when the store holds no event.
    Expired { pruned_before: Option<SystemTime> },
}

/// The events a [`WatchLog::read_after`] returns, in id order, and the gap before them, if any.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Page {
    /// The matching events, each with `id`, `at` and `cursor` filled in.
    pub events: Vec<WatchEvent>,
    /// Set when the read started from the oldest event instead of after the cursor.
    pub gap: Option<Gap>,
}

/// The result of [`WatchLog::prune`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pruned {
    /// How many events were removed.
    pub removed: u64,
    /// The store's `pruned_through` after the prune: the id of the latest event any prune
    /// removed, or `None` while none has removed one.
    pub pruned_through: Option<Uuid>,
}

/// A watch log store: an append-only, id-ordered log of `WatchEvent`s with their raw payloads,
/// pruned from the oldest end (design 6.2, 5.7, B20).
///
/// - Ids are UUIDv7s the store assigns with [`next_id`](super::next_id) under its append lock,
///   so id order is commit order.
/// - The store epoch names one store's lifetime; a cursor of another epoch reads as
///   [`Gap::StoreChanged`].
/// - `pruned_through` is the id of the latest pruned event. A cursor strictly below it is
///   expired; a cursor at or above it resumes with no gap, even when its own event was pruned.
/// - The raw payload is kept apart from the event and goes with it when it is pruned.
/// - Cursors are never stored: reads build them from the epoch and the id.
pub trait WatchLog: Send + Sync {
    type Error: std::error::Error + Send + Sync + 'static;

    /// Stores `event`, recorded at `at`, with its raw `payload`.
    ///
    /// The store assigns the id and sets the event's `id` and `at`; it clears `cursor`, and
    /// clears a `github_webhook` body's `payload`, which it keeps apart. A `delivery` is unique
    /// among stored events: when a stored event has it, nothing is stored and the outcome is
    /// [`AppendOutcome::Duplicate`]. An event without a delivery is always stored.
    fn append(
        &self,
        event: WatchEvent,
        payload: Vec<u8>,
        delivery: Option<String>,
        at: SystemTime,
    ) -> impl Future<Output = Result<AppendOutcome, Self::Error>> + Send;

    /// The store epoch, fixed for the store's lifetime.
    fn epoch(&self) -> Uuid;

    /// Up to `limit` events that match `filter`, in id order, resolving `after` as design 5.7
    /// does:
    ///
    /// - no cursor: from the oldest event, with no gap;
    /// - a cursor of another epoch: [`Gap::StoreChanged`], from the oldest event;
    /// - an id strictly below `pruned_through`: [`Gap::Expired`], from the oldest event;
    /// - any other id: the events with an id strictly greater than it, with no gap.
    ///
    /// The filter is applied before the limit. Each event comes with `id`, `at` and `cursor`
    /// (`<epoch>.<id>`) filled in; with `include_payload`, a `github_webhook` body also carries
    /// the stored payload, and without it the body's `payload` is empty.
    fn read_after(
        &self,
        after: Option<Cursor>,
        filter: &Filter,
        limit: usize,
        include_payload: bool,
    ) -> impl Future<Output = Result<Page, Self::Error>> + Send;

    /// The cursor of the newest stored event, or `None` when the store is empty.
    fn head(&self) -> impl Future<Output = Result<Option<Cursor>, Self::Error>> + Send;

    /// The cursor of the oldest stored event, or `None` when the store is empty.
    fn oldest(&self) -> impl Future<Output = Result<Option<Cursor>, Self::Error>> + Send;

    /// Removes, with their payloads, the id-ordered prefix of events up to the newest one that
    /// breaks either bound of `retention`: an `at` before `now` minus the age bound, or a place
    /// outside the newest `max_events`. `pruned_through` becomes the id of the last event
    /// removed, and stays as it was when nothing is removed.
    ///
    /// Since the prefix is cut by id, an event that meets the age bound on its own is removed
    /// when an event with a greater id breaks the age bound; `at` normally rises with id.
    fn prune(
        &self,
        retention: &Retention,
        now: SystemTime,
    ) -> impl Future<Output = Result<Pruned, Self::Error>> + Send;
}
