//! [`MemoryWatchLog`], the in-memory [`WatchLog`] the service's and server's tests use.
//!
//! Each method does its work when its future is first polled, as a database store does, so a
//! future dropped unpolled changes nothing. The lock is taken and released within that poll.

use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::SystemTime;

use mintswarm_proto::prost_types::Timestamp;
use mintswarm_proto::watch::v1::WatchEvent;
use mintswarm_proto::watch::v1::watch_event::Body;
use uuid::Uuid;

use super::store::{AppendOutcome, Gap, Page, Pruned, WatchLog};
use super::{Cursor, Filter, Retention, WatchError, next_id};

/// One stored event.
#[derive(Debug)]
struct Entry {
    id: Uuid,
    at: SystemTime,
    delivery: Option<String>,
    /// The event as appended, with `id` and `at` set and `cursor` and the payload cleared.
    event: WatchEvent,
    payload: Vec<u8>,
}

#[derive(Debug, Default)]
struct State {
    /// In id order: ids are assigned in append order and only rise.
    entries: Vec<Entry>,
    pruned_through: Option<Uuid>,
}

/// A [`WatchLog`] kept in memory, with a fresh UUIDv7 epoch made when it is created.
#[derive(Debug)]
pub struct MemoryWatchLog {
    epoch: Uuid,
    state: Mutex<State>,
}

impl MemoryWatchLog {
    /// An empty store with a fresh epoch.
    pub fn new() -> Self {
        Self {
            epoch: Uuid::now_v7(),
            state: Mutex::new(State::default()),
        }
    }

    fn state(&self) -> MutexGuard<'_, State> {
        // Every change is made in full before the guard drops, so a poisoned state is whole.
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn cursor(&self, id: Uuid) -> Result<Cursor, WatchError> {
        Cursor::new(self.epoch, id)
    }

    fn append_now(
        &self,
        mut event: WatchEvent,
        payload: Vec<u8>,
        delivery: Option<String>,
        at: SystemTime,
    ) -> Result<AppendOutcome, WatchError> {
        let mut state = self.state();
        if delivery.is_some() && state.entries.iter().any(|entry| entry.delivery == delivery) {
            return Ok(AppendOutcome::Duplicate);
        }
        // Above every id ever assigned, stored or pruned, so a cursor at `pruned_through` never
        // skips a later event, whatever the clock does after a prune empties the store.
        let latest = state
            .entries
            .last()
            .map(|entry| entry.id)
            .max(state.pruned_through);
        let id = next_id(latest)?;
        event.id = id.to_string();
        event.at = Some(Timestamp::from(at));
        event.cursor.clear();
        if let Some(Body::GithubWebhook(webhook)) = &mut event.body {
            webhook.payload.clear();
        }
        state.entries.push(Entry {
            id,
            at,
            delivery,
            event,
            payload,
        });
        Ok(AppendOutcome::Appended(id))
    }

    fn read_now(
        &self,
        after: Option<Cursor>,
        filter: &Filter,
        limit: usize,
        include_payload: bool,
    ) -> Result<Page, WatchError> {
        let state = self.state();
        let (start, gap) = match after {
            None => (0, None),
            Some(cursor) if cursor.epoch() != self.epoch => (0, Some(Gap::StoreChanged)),
            Some(cursor) if state.pruned_through.is_some_and(|p| cursor.id() < p) => (
                0,
                Some(Gap::Expired {
                    pruned_before: state.entries.first().map(|entry| entry.at),
                }),
            ),
            Some(cursor) => (
                state
                    .entries
                    .partition_point(|entry| entry.id <= cursor.id()),
                None,
            ),
        };
        let events = state.entries[start..]
            .iter()
            .filter(|entry| filter.matches(&entry.event))
            .take(limit)
            .map(|entry| self.read_entry(entry, include_payload))
            .collect::<Result<_, _>>()?;
        Ok(Page { events, gap })
    }

    /// The stored event of `entry` as a read returns it.
    fn read_entry(&self, entry: &Entry, include_payload: bool) -> Result<WatchEvent, WatchError> {
        let mut event = entry.event.clone();
        event.cursor = self.cursor(entry.id)?.to_string();
        if include_payload && let Some(Body::GithubWebhook(webhook)) = &mut event.body {
            webhook.payload.clone_from(&entry.payload);
        }
        Ok(event)
    }

    fn end_cursor(&self, newest: bool) -> Result<Option<Cursor>, WatchError> {
        let state = self.state();
        let entry = if newest {
            state.entries.last()
        } else {
            state.entries.first()
        };
        entry.map(|entry| self.cursor(entry.id)).transpose()
    }

    fn prune_now(&self, retention: &Retention, now: SystemTime) -> Pruned {
        let mut state = self.state();
        let len = state.entries.len();
        let max_events = usize::try_from(retention.max_events()).unwrap_or(usize::MAX);
        let by_count = len.saturating_sub(max_events);
        let by_age = now
            .checked_sub(retention.max_age())
            .and_then(|limit| state.entries.iter().rposition(|entry| entry.at < limit))
            .map_or(0, |last| last + 1);
        let cut = by_count.max(by_age);
        if cut > 0 {
            state.pruned_through = Some(state.entries[cut - 1].id);
            state.entries.drain(..cut);
        }
        Pruned {
            removed: cut as u64,
            pruned_through: state.pruned_through,
        }
    }
}

impl Default for MemoryWatchLog {
    fn default() -> Self {
        Self::new()
    }
}

impl WatchLog for MemoryWatchLog {
    type Error = WatchError;

    async fn append(
        &self,
        event: WatchEvent,
        payload: Vec<u8>,
        delivery: Option<String>,
        at: SystemTime,
    ) -> Result<AppendOutcome, WatchError> {
        self.append_now(event, payload, delivery, at)
    }

    fn epoch(&self) -> Uuid {
        self.epoch
    }

    async fn read_after(
        &self,
        after: Option<Cursor>,
        filter: &Filter,
        limit: usize,
        include_payload: bool,
    ) -> Result<Page, WatchError> {
        self.read_now(after, filter, limit, include_payload)
    }

    async fn head(&self) -> Result<Option<Cursor>, WatchError> {
        self.end_cursor(true)
    }

    async fn oldest(&self) -> Result<Option<Cursor>, WatchError> {
        self.end_cursor(false)
    }

    async fn prune(&self, retention: &Retention, now: SystemTime) -> Result<Pruned, WatchError> {
        Ok(self.prune_now(retention, now))
    }
}
