//! `WatchService` over `MemoryWatchLog` through an in-process gRPC transport, and the pruning
//! task under paused tokio time.

#![cfg(test)]

use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, SystemTime};

use hyper_util::rt::TokioIo;
use mintswarm_core::watch::{
    AppendOutcome, Cursor, Filter, LIST_LIMIT_DEFAULT, LIST_LIMIT_MAX, MemoryWatchLog,
    PROTOCOL_MINOR, PRUNE_INTERVAL, Page, Pruned, Retention, WEBHOOK_KIND, WatchError, WatchLog,
    WatchService, prune_periodically,
};
use mintswarm_proto::prost_types::Timestamp;
use mintswarm_proto::watch::v1::watch_service_client::WatchServiceClient;
use mintswarm_proto::watch::v1::watch_service_server::WatchServiceServer;
use mintswarm_proto::watch::v1::{
    GapReason, GetServerInfoRequest, ListWatchEventsRequest, ListWatchEventsResponse, MarkKind,
    SubscribeRequest, WatchEvent, WatchFilter,
};
use tokio::sync::mpsc;
use tokio::time::Instant;
use tonic::Code;
use tonic::codegen::tokio_stream;
use tonic::transport::{Channel, Endpoint, Server, Uri};
use uuid::Uuid;

/// Serves `service` over an in-memory duplex pipe and returns a client connected to it.
async fn client<S: WatchLog + 'static>(service: WatchService<S>) -> WatchServiceClient<Channel> {
    let (client_io, server_io) = tokio::io::duplex(1 << 16);
    tokio::spawn(
        Server::builder()
            .add_service(WatchServiceServer::new(service))
            .serve_with_incoming(tokio_stream::once(Ok::<_, std::io::Error>(server_io))),
    );
    let mut client_io = Some(client_io);
    let channel = Endpoint::from_static("http://in-process")
        .connect_with_connector(tower::service_fn(move |_: Uri| {
            let io = client_io.take();
            async move {
                io.map(TokioIo::new)
                    .ok_or_else(|| std::io::Error::other("the pipe is already connected"))
            }
        }))
        .await
        .expect("connect");
    WatchServiceClient::new(channel)
}

/// Retention that keeps every event the tests append.
fn retention() -> Retention {
    Retention::new(Duration::from_secs(7 * 24 * 3600), 10_000).expect("valid retention")
}

/// A `github.webhook` event.
fn webhook() -> WatchEvent {
    WatchEvent {
        kind: WEBHOOK_KIND.to_owned(),
        summary: WEBHOOK_KIND.to_owned(),
        ..WatchEvent::default()
    }
}

/// An event of the made-up kind `plane.job`, which [`webhooks`] excludes.
fn plane_event() -> WatchEvent {
    WatchEvent {
        kind: "plane.job".to_owned(),
        ..WatchEvent::default()
    }
}

/// A filter of `github.webhook` events only.
fn webhooks() -> WatchFilter {
    WatchFilter {
        kinds: vec![WEBHOOK_KIND.to_owned()],
        ..WatchFilter::default()
    }
}

/// Appends `event` at the wall-clock now and returns its id.
async fn append<S: WatchLog>(store: &S, event: WatchEvent) -> Uuid {
    match store
        .append(event, Vec::new(), None, SystemTime::now())
        .await
        .expect("append")
    {
        AppendOutcome::Appended(id) => id,
        AppendOutcome::Duplicate => panic!("an append without a delivery is a duplicate"),
    }
}

/// Appends `count` webhook events and returns their ids.
async fn append_webhooks<S: WatchLog>(store: &S, count: usize) -> Vec<Uuid> {
    let mut ids = Vec::with_capacity(count);
    for _ in 0..count {
        ids.push(append(store, webhook()).await);
    }
    ids
}

/// The cursor of `id` in `store`, as a string.
fn cursor<S: WatchLog>(store: &S, id: Uuid) -> String {
    Cursor::new(store.epoch(), id)
        .expect("a valid cursor")
        .to_string()
}

/// The ids of a page's events.
fn ids(response: &ListWatchEventsResponse) -> Vec<Uuid> {
    response
        .events
        .iter()
        .map(|event| Uuid::parse_str(&event.id).expect("a UUID id"))
        .collect()
}

/// A `ListWatchEvents` request for webhooks after `after`, with `limit`.
fn list(after: &str, limit: u32) -> ListWatchEventsRequest {
    ListWatchEventsRequest {
        filter: Some(webhooks()),
        after: after.to_owned(),
        limit,
        include_payload: false,
    }
}

/// Lists one page, panicking on an error.
async fn list_ok(
    client: &mut WatchServiceClient<Channel>,
    request: ListWatchEventsRequest,
) -> ListWatchEventsResponse {
    client
        .list_watch_events(request)
        .await
        .expect("list_watch_events")
        .into_inner()
}

/// A [`MemoryWatchLog`] with two hooks: an event that `read_after` appends once before it
/// reads, and a channel on which `prune` reports when it ran and what it removed.
struct Hooked {
    inner: MemoryWatchLog,
    append_on_read: Mutex<Option<WatchEvent>>,
    prunes: Option<mpsc::UnboundedSender<(Instant, Pruned)>>,
}

impl Hooked {
    fn new() -> Self {
        Self {
            inner: MemoryWatchLog::new(),
            append_on_read: Mutex::new(None),
            prunes: None,
        }
    }
}

impl WatchLog for Hooked {
    type Error = WatchError;

    async fn append(
        &self,
        event: WatchEvent,
        payload: Vec<u8>,
        delivery: Option<String>,
        at: SystemTime,
    ) -> Result<AppendOutcome, WatchError> {
        self.inner.append(event, payload, delivery, at).await
    }

    fn epoch(&self) -> Uuid {
        self.inner.epoch()
    }

    async fn read_after(
        &self,
        after: Option<Cursor>,
        filter: &Filter,
        limit: usize,
        include_payload: bool,
    ) -> Result<Page, WatchError> {
        let racing = self
            .append_on_read
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        if let Some(event) = racing {
            append(&self.inner, event).await;
        }
        self.inner
            .read_after(after, filter, limit, include_payload)
            .await
    }

    async fn head(&self) -> Result<Option<Cursor>, WatchError> {
        self.inner.head().await
    }

    async fn oldest(&self) -> Result<Option<Cursor>, WatchError> {
        self.inner.oldest().await
    }

    async fn prune(&self, retention: &Retention, now: SystemTime) -> Result<Pruned, WatchError> {
        let pruned = self.inner.prune(retention, now).await?;
        if let Some(prunes) = &self.prunes {
            prunes
                .send((Instant::now(), pruned))
                .expect("the test listens");
        }
        Ok(pruned)
    }
}

/// Stores three webhooks and prunes all but the newest; returns the store, the ids and the
/// prune's outcome.
async fn pruned_to_one() -> (Arc<MemoryWatchLog>, Vec<Uuid>, Pruned) {
    let store = Arc::new(MemoryWatchLog::new());
    let ids = append_webhooks(&*store, 3).await;
    let keep_one = Retention::new(Duration::from_secs(7 * 24 * 3600), 1).expect("valid");
    let pruned = store
        .prune(&keep_one, SystemTime::now())
        .await
        .expect("prune");
    assert_eq!(pruned.removed, 2);
    (store, ids, pruned)
}

#[tokio::test]
async fn limit_above_max_is_capped_with_next_cursor() {
    let store = Arc::new(MemoryWatchLog::new());
    let stored = append_webhooks(&*store, 600).await;
    let mut client = client(WatchService::new(Arc::clone(&store), retention())).await;

    let first = list_ok(&mut client, list("", 1000)).await;
    assert_eq!(ids(&first), stored[..500]);
    assert_eq!(first.next_cursor, cursor(&*store, stored[499]));

    let second = list_ok(&mut client, list(&first.next_cursor, 1000)).await;
    assert_eq!(ids(&second), stored[500..]);
}

#[tokio::test]
async fn zero_limit_uses_the_default() {
    let store = Arc::new(MemoryWatchLog::new());
    append_webhooks(&*store, 150).await;
    let mut client = client(WatchService::new(Arc::clone(&store), retention())).await;

    let page = list_ok(&mut client, list("", 0)).await;
    assert_eq!(page.events.len(), 100);
}

#[tokio::test]
async fn cursor_of_another_epoch_gets_store_changed() {
    let store = Arc::new(MemoryWatchLog::new());
    let stored = append_webhooks(&*store, 2).await;
    let elsewhere = MemoryWatchLog::new();
    let foreign = cursor(&elsewhere, append(&elsewhere, webhook()).await);
    let mut client = client(WatchService::new(Arc::clone(&store), retention())).await;

    let page = list_ok(&mut client, list(&foreign, 0)).await;
    let gap = page.gap.clone().expect("a gap mark");
    assert_eq!(gap.kind(), MarkKind::Gap);
    assert_eq!(gap.gap_reason(), GapReason::StoreChanged);
    assert_eq!(ids(&page), stored);
}

#[tokio::test]
async fn cursor_below_pruned_through_gets_cursor_expired() {
    let (store, stored, _) = pruned_to_one().await;
    let mut client = client(WatchService::new(Arc::clone(&store), retention())).await;

    let page = list_ok(&mut client, list(&cursor(&*store, stored[0]), 0)).await;
    let gap = page.gap.clone().expect("a gap mark");
    assert_eq!(gap.kind(), MarkKind::Gap);
    assert_eq!(gap.gap_reason(), GapReason::CursorExpired);
    assert_eq!(ids(&page), stored[2..]);
    let oldest_at: Option<Timestamp> = page.events[0].at;
    assert!(oldest_at.is_some());
    assert_eq!(gap.pruned_before, oldest_at);
}

#[tokio::test]
async fn cursor_at_pruned_through_resumes_without_gap() {
    let (store, stored, pruned) = pruned_to_one().await;
    let through = pruned.pruned_through.expect("a prune removed events");
    assert_eq!(through, stored[1]);
    let mut client = client(WatchService::new(Arc::clone(&store), retention())).await;

    let page = list_ok(&mut client, list(&cursor(&*store, through), 0)).await;
    assert_eq!(page.gap, None);
    assert_eq!(ids(&page), stored[2..]);
}

#[tokio::test]
async fn empty_kinds_is_invalid_argument() {
    let store = Arc::new(MemoryWatchLog::new());
    let mut client = client(WatchService::new(store, retention())).await;
    let request = ListWatchEventsRequest {
        filter: Some(WatchFilter::default()),
        ..list("", 0)
    };

    let status = client
        .list_watch_events(request)
        .await
        .expect_err("refused");
    assert_eq!(status.code(), Code::InvalidArgument);
}

#[tokio::test]
async fn missing_filter_is_invalid_argument() {
    let store = Arc::new(MemoryWatchLog::new());
    let mut client = client(WatchService::new(store, retention())).await;
    let request = ListWatchEventsRequest {
        filter: None,
        ..list("", 0)
    };

    let status = client
        .list_watch_events(request)
        .await
        .expect_err("refused");
    assert_eq!(status.code(), Code::InvalidArgument);
}

#[tokio::test]
async fn malformed_cursor_is_invalid_argument() {
    let store = Arc::new(MemoryWatchLog::new());
    let mut client = client(WatchService::new(store, retention())).await;

    let status = client
        .list_watch_events(list("not-a-cursor", 0))
        .await
        .expect_err("refused");
    assert_eq!(status.code(), Code::InvalidArgument);
}

#[tokio::test]
async fn server_info_reports_epoch_retention_and_oldest_cursor() {
    let store = Arc::new(MemoryWatchLog::new());
    let bounds = Retention::new(Duration::from_secs(3 * 24 * 3600), 250).expect("valid");
    let mut client = client(WatchService::new(Arc::clone(&store), bounds.clone())).await;

    let empty = client
        .get_server_info(GetServerInfoRequest {})
        .await
        .expect("get_server_info")
        .into_inner();
    assert_eq!(empty.store_epoch, store.epoch().to_string());
    assert_eq!(empty.kinds, [WEBHOOK_KIND]);
    assert_eq!(empty.protocol_minor, PROTOCOL_MINOR);
    assert_eq!(empty.server_version, env!("CARGO_PKG_VERSION"));
    let empty_retention = empty.retention.expect("retention is always set");
    assert_eq!(empty_retention.oldest_cursor, "");
    assert_eq!(
        empty_retention.max_age.map(Duration::try_from),
        Some(Ok(Duration::from_secs(3 * 24 * 3600)))
    );
    assert_eq!(empty_retention.max_events, 250);

    let stored = append_webhooks(&*store, 2).await;
    let filled = client
        .get_server_info(GetServerInfoRequest {})
        .await
        .expect("get_server_info")
        .into_inner();
    assert_eq!(
        filled.retention,
        Some(bounds.to_proto(cursor(&*store, stored[0])))
    );
}

#[tokio::test]
async fn subscribe_is_unimplemented() {
    let store = Arc::new(MemoryWatchLog::new());
    let mut client = client(WatchService::new(store, retention())).await;

    let status = client
        .subscribe(SubscribeRequest::default())
        .await
        .expect_err("not served");
    assert_eq!(status.code(), Code::Unimplemented);
}

#[tokio::test]
async fn excluded_events_give_an_empty_page_at_the_head() {
    let store = Arc::new(MemoryWatchLog::new());
    append(&*store, plane_event()).await;
    let last = append(&*store, plane_event()).await;
    let mut client = client(WatchService::new(Arc::clone(&store), retention())).await;

    let page = list_ok(&mut client, list("", 0)).await;
    assert!(page.events.is_empty());
    assert_eq!(page.next_cursor, cursor(&*store, last));
    assert_eq!(page.head_cursor, cursor(&*store, last));
}

#[tokio::test]
async fn empty_store_echoes_after() {
    let store = Arc::new(MemoryWatchLog::new());
    let after = cursor(&*store, Uuid::now_v7());
    let mut client = client(WatchService::new(Arc::clone(&store), retention())).await;

    let page = list_ok(&mut client, list(&after, 0)).await;
    assert!(page.events.is_empty());
    assert_eq!(page.next_cursor, after);
    assert_eq!(page.head_cursor, "");
}

#[tokio::test]
async fn racing_append_that_matches_moves_both_cursors_to_it() {
    let store = Arc::new(Hooked::new());
    let first = append(&*store, webhook()).await;
    *store.append_on_read.lock().expect("unpoisoned") = Some(webhook());
    let mut client = client(WatchService::new(Arc::clone(&store), retention())).await;

    let page = list_ok(&mut client, list(&cursor(&*store, first), 0)).await;
    let racing = ids(&page);
    assert_eq!(racing.len(), 1);
    assert_eq!(page.next_cursor, cursor(&*store, racing[0]));
    assert_eq!(page.head_cursor, cursor(&*store, racing[0]));
}

#[tokio::test]
async fn racing_append_that_is_excluded_keeps_both_cursors_at_the_earlier_head() {
    let store = Arc::new(Hooked::new());
    let first = append(&*store, webhook()).await;
    *store.append_on_read.lock().expect("unpoisoned") = Some(plane_event());
    let mut client = client(WatchService::new(Arc::clone(&store), retention())).await;

    let page = list_ok(&mut client, list(&cursor(&*store, first), 0)).await;
    assert!(page.events.is_empty());
    assert_eq!(page.next_cursor, cursor(&*store, first));
    assert_eq!(page.head_cursor, cursor(&*store, first));
}

#[tokio::test(start_paused = true)]
async fn prunes_at_start_and_every_ten_minutes() {
    assert_eq!(PRUNE_INTERVAL, Duration::from_secs(600));
    let (prunes, mut reports) = mpsc::unbounded_channel();
    let store = Arc::new(Hooked {
        prunes: Some(prunes),
        ..Hooked::new()
    });
    append_webhooks(&*store, 3).await;
    let keep_one = Retention::new(Duration::from_secs(7 * 24 * 3600), 1).expect("valid");
    let start = Instant::now();
    tokio::spawn(prune_periodically(Arc::clone(&store), keep_one));

    let (first_at, first) = reports.recv().await.expect("the first prune");
    assert_eq!(first_at, start);
    assert_eq!(first.removed, 2);

    append_webhooks(&*store, 2).await;
    let (second_at, second) = reports.recv().await.expect("the second prune");
    assert_eq!(second_at - first_at, Duration::from_secs(600));
    assert_eq!(second.removed, 2);
}

#[test]
fn limit_constants_match_watch_proto() {
    let proto = include_str!("../../../api/proto/mintswarm/watch/v1/watch.proto");
    let lines: Vec<&str> = proto.lines().collect();
    let field = lines
        .iter()
        .position(|line| line.trim() == "uint32 limit = 3;")
        .expect("watch.proto declares `uint32 limit = 3;`");
    let mut comment: Vec<&str> = lines[..field]
        .iter()
        .rev()
        .map(|line| line.trim())
        .take_while(|line| line.starts_with("//"))
        .map(|line| line.trim_start_matches("//").trim())
        .collect();
    comment.reverse();
    let comment = comment.join(" ");

    for stated in [
        format!("at most {LIST_LIMIT_MAX}"),
        format!("default of {LIST_LIMIT_DEFAULT}"),
        format!("capped at {LIST_LIMIT_MAX}"),
    ] {
        assert!(
            comment.contains(&stated),
            "the `limit` comment `{comment}` does not state `{stated}`"
        );
    }
}
