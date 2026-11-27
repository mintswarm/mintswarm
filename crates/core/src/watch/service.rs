//! [`WatchService`], the unary RPCs of `mintswarm.watch.v1.WatchService` over a [`WatchLog`],
//! the limits they apply, and [`prune_periodically`], the task that keeps the log within its
//! [`Retention`] (design 5.6, 5.7, B20, A23).

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use mintswarm_proto::prost_types::Timestamp;
use mintswarm_proto::watch::v1::{
    self, GapReason, GetServerInfoRequest, GetServerInfoResponse, ListWatchEventsRequest,
    ListWatchEventsResponse, Mark, MarkKind, SubscribeRequest, SubscribeResponse, WatchEvent,
};
use tokio::time::MissedTickBehavior;
use tonic::{Request, Response, Status};

use super::{Cursor, Filter, Gap, Retention, WEBHOOK_KIND, WatchLog};

/// The most events one `ListWatchEvents` page returns; a larger `limit` is capped to it
/// (watch.proto, `ListWatchEventsRequest.limit`).
pub const LIST_LIMIT_MAX: u32 = 500;

/// The page size of a `ListWatchEvents` request whose `limit` is zero (watch.proto,
/// `ListWatchEventsRequest.limit`).
pub const LIST_LIMIT_DEFAULT: u32 = 100;

/// How often [`prune_periodically`] prunes, after its first prune at start (design A23).
pub const PRUNE_INTERVAL: Duration = Duration::from_secs(600);

/// The protocol's minor version within `v1`, raised by each minor change: a kind, field, enum
/// value, attribute, party role or mark kind added (watch.proto, the minor-version rule).
pub const PROTOCOL_MINOR: u32 = 0;

/// The watch service's unary RPCs, `ListWatchEvents` and `GetServerInfo`, over the store `S`
/// with the retention bounds it reports. `Subscribe` answers UNIMPLEMENTED for now.
#[derive(Debug)]
pub struct WatchService<S> {
    store: Arc<S>,
    retention: Retention,
}

impl<S> WatchService<S> {
    /// The service over `store`, reporting `retention`.
    pub fn new(store: Arc<S>, retention: Retention) -> Self {
        Self { store, retention }
    }
}

/// A store failure as the client sees it: the database is unreachable, so it retries.
fn unavailable(error: impl std::error::Error) -> Status {
    Status::unavailable(format!("the watch store failed: {error}"))
}

/// The cursor of an event the store returned.
fn event_cursor(event: &WatchEvent) -> Result<Cursor, Status> {
    Cursor::parse(&event.cursor)
        .map_err(|error| Status::internal(format!("the store returned a bad cursor: {error}")))
}

/// The later of `a` and `b` by id; either may be absent.
fn later(a: Option<Cursor>, b: Option<Cursor>) -> Option<Cursor> {
    match (a, b) {
        (Some(a), Some(b)) => Some(if b.id() > a.id() { b } else { a }),
        (a, b) => a.or(b),
    }
}

/// The page's `GAP` mark for `gap`.
fn gap_mark(gap: Gap) -> Mark {
    let (reason, pruned_before) = match gap {
        Gap::StoreChanged => (GapReason::StoreChanged, None),
        Gap::Expired { pruned_before } => {
            (GapReason::CursorExpired, pruned_before.map(Timestamp::from))
        }
    };
    Mark {
        kind: MarkKind::Gap.into(),
        cursor: String::new(),
        gap_reason: reason.into(),
        pruned_before,
    }
}

/// The page size `limit` asks for: the default for zero, and at most [`LIST_LIMIT_MAX`].
fn page_size(limit: u32) -> usize {
    let limit = if limit == 0 {
        LIST_LIMIT_DEFAULT
    } else {
        limit.min(LIST_LIMIT_MAX)
    };
    usize::try_from(limit).unwrap_or(usize::MAX)
}

#[tonic::async_trait]
impl<S: WatchLog + 'static> v1::watch_service_server::WatchService for WatchService<S> {
    async fn get_server_info(
        &self,
        _: Request<GetServerInfoRequest>,
    ) -> Result<Response<GetServerInfoResponse>, Status> {
        let oldest = self.store.oldest().await.map_err(unavailable)?;
        let oldest = oldest.map(|cursor| cursor.to_string()).unwrap_or_default();
        Ok(Response::new(GetServerInfoResponse {
            store_epoch: self.store.epoch().to_string(),
            kinds: vec![WEBHOOK_KIND.to_owned()],
            retention: Some(self.retention.to_proto(oldest)),
            protocol_minor: PROTOCOL_MINOR,
            server_version: env!("CARGO_PKG_VERSION").to_owned(),
        }))
    }

    type SubscribeStream = tonic::codegen::BoxStream<SubscribeResponse>;

    async fn subscribe(
        &self,
        _: Request<SubscribeRequest>,
    ) -> Result<Response<Self::SubscribeStream>, Status> {
        Err(Status::unimplemented("Subscribe is not served yet"))
    }

    async fn list_watch_events(
        &self,
        request: Request<ListWatchEventsRequest>,
    ) -> Result<Response<ListWatchEventsResponse>, Status> {
        let request = request.into_inner();
        let filter = Filter::new(request.filter.unwrap_or_default())?;
        let after = if request.after.is_empty() {
            None
        } else {
            Some(Cursor::parse(&request.after)?)
        };
        let limit = page_size(request.limit);

        // The head is read before the page, so an event appended between the two reads is
        // either on the page or after `next_cursor`, never skipped.
        let head = self.store.head().await.map_err(unavailable)?;
        let page = self
            .store
            .read_after(after, &filter, limit, request.include_payload)
            .await
            .map_err(unavailable)?;

        let last = page.events.last().map(event_cursor).transpose()?;
        let next = if page.events.len() >= limit {
            last
        } else {
            later(last, head)
        };
        // With no event and no head, the next page starts where this one did: after the same
        // cursor, or, past a gap, from the oldest event, so the gap is not reported again.
        let next_cursor = match next {
            Some(cursor) => cursor.to_string(),
            None if page.gap.is_some() => String::new(),
            None => request.after,
        };
        let head_cursor = later(head, last)
            .map(|cursor| cursor.to_string())
            .unwrap_or_default();

        Ok(Response::new(ListWatchEventsResponse {
            events: page.events,
            next_cursor,
            head_cursor,
            gap: page.gap.map(gap_mark),
        }))
    }
}

/// Prunes `store` to `retention` at once and then every [`PRUNE_INTERVAL`]; never returns.
///
/// The caller spawns it. A failed prune is logged and retried at the next tick.
pub async fn prune_periodically<S: WatchLog>(store: Arc<S>, retention: Retention) {
    let mut interval = tokio::time::interval(PRUNE_INTERVAL);
    interval.set_missed_tick_behavior(MissedTickBehavior::Delay);
    loop {
        interval.tick().await;
        match store.prune(&retention, SystemTime::now()).await {
            Ok(pruned) => tracing::debug!(removed = pruned.removed, "pruned the watch log"),
            Err(error) => tracing::warn!(%error, "pruning the watch log failed"),
        }
    }
}
