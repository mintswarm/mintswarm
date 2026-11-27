//! The `Subscribe` stream (design 5.1, 5.6, 5.7, 5.8): its limits, the task that reads the log
//! for one stream, and [`Outbound`], the response body that carries the task's messages and its
//! terminal status.
//!
//! The task reads batches after the stream's position and sends them into a bounded channel, so
//! a slow client stops it reading and never slows ingest. Once caught up, it waits on the head
//! watch that ingest advances after each committed append.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use mintswarm_proto::watch::v1::{Mark, MarkKind, SubscribeResponse, subscribe_response};
use tokio::sync::{OwnedSemaphorePermit, mpsc, oneshot, watch};
use tokio::time::{Instant, Sleep};
use tonic::Status;
use tonic::codegen::tokio_stream::Stream;

use super::service::{event_cursor, gap_mark, later, unavailable};
use super::{Cursor, Filter, WatchLog};

/// The most `Subscribe` streams open at once; the next one fails with RESOURCE_EXHAUSTED
/// (watch.proto, `rpc Subscribe`).
pub const SUBSCRIBE_STREAMS_MAX: usize = 64;

/// How long a stream waits on a client that reads nothing before it ends with UNAVAILABLE
/// (watch.proto, `rpc Subscribe`).
pub const SUBSCRIBE_UNREAD_TIMEOUT: Duration = Duration::from_secs(600);

/// The heartbeat period of a request whose `heartbeat_seconds` is zero (watch.proto,
/// `SubscribeRequest.heartbeat_seconds`).
pub const HEARTBEAT_DEFAULT_SECONDS: u32 = 30;

/// The shortest heartbeat period; a lower `heartbeat_seconds` is raised to it (watch.proto,
/// `SubscribeRequest.heartbeat_seconds`).
pub const HEARTBEAT_MIN_SECONDS: u32 = 5;

/// The most events one read of the log returns.
const SUBSCRIBE_BATCH_SIZE: usize = 100;

/// How many messages wait between a stream's task and its response body.
pub(super) const SUBSCRIBE_CHANNEL_CAPACITY: usize = 16;

/// The idle period `seconds` asks for: the default for zero, and at least the minimum.
pub(super) fn heartbeat_period(seconds: u32) -> Duration {
    let seconds = if seconds == 0 {
        HEARTBEAT_DEFAULT_SECONDS
    } else {
        seconds.max(HEARTBEAT_MIN_SECONDS)
    };
    Duration::from_secs(u64::from(seconds))
}

/// Where a stream starts.
#[derive(Debug, Clone, Copy)]
pub(super) enum Start {
    /// After this cursor.
    After(Cursor),
    /// At the head, with a `CAUGHT_UP` mark at once.
    Now,
    /// At the oldest stored event.
    Oldest,
}

/// What one stream reads and how: everything its task needs besides the channel ends.
pub(super) struct Subscription<S> {
    pub(super) store: Arc<S>,
    pub(super) filter: Filter,
    pub(super) start: Start,
    pub(super) include_payload: bool,
    /// The idle period after which a `HEARTBEAT` mark is sent.
    pub(super) period: Duration,
    /// The head watch, advanced by ingest after each committed append.
    pub(super) head: watch::Receiver<Option<Cursor>>,
}

/// Why a stream's task ended.
enum End {
    /// The client closed the stream.
    Closed,
    /// The stream ends with this status.
    Failed(Status),
}

/// Runs the stream `subscription` into `tx` until the client closes it or it fails; a failure's
/// status goes into `status` before `tx` is dropped. Dropping `permit` on return frees the slot.
pub(super) async fn run<S: WatchLog>(
    subscription: Subscription<S>,
    tx: mpsc::Sender<SubscribeResponse>,
    status: oneshot::Sender<Status>,
    permit: OwnedSemaphorePermit,
) {
    let Err(end) = serve(subscription, &tx).await;
    if let End::Failed(failure) = end {
        // The receiver is gone only when the client closed the stream.
        let _ = status.send(failure);
    }
    drop(tx);
    drop(permit);
}

/// A `MARK_KIND_CAUGHT_UP` or `MARK_KIND_HEARTBEAT` mark at `position`.
fn position_mark(kind: MarkKind, position: Option<Cursor>) -> SubscribeResponse {
    message(subscribe_response::Message::Mark(Mark {
        kind: kind.into(),
        cursor: position
            .map(|cursor| cursor.to_string())
            .unwrap_or_default(),
        ..Mark::default()
    }))
}

fn message(message: subscribe_response::Message) -> SubscribeResponse {
    SubscribeResponse {
        message: Some(message),
    }
}

/// Sends `message`, then restarts the idle timer `sleep`. A send that stays pending for
/// [`SUBSCRIBE_UNREAD_TIMEOUT`] fails the stream with UNAVAILABLE.
async fn send(
    tx: &mpsc::Sender<SubscribeResponse>,
    sleep: Pin<&mut Sleep>,
    period: Duration,
    message: SubscribeResponse,
) -> Result<(), End> {
    match tokio::time::timeout(SUBSCRIBE_UNREAD_TIMEOUT, tx.send(message)).await {
        Ok(Ok(())) => {
            sleep.reset(Instant::now() + period);
            Ok(())
        }
        Ok(Err(_)) => Err(End::Closed),
        Err(_) => Err(End::Failed(Status::unavailable(format!(
            "the client has not read from the stream for {} s",
            SUBSCRIBE_UNREAD_TIMEOUT.as_secs()
        )))),
    }
}

/// The stream's loop: catch-up, marks, gaps, heartbeats and live events, until it ends.
async fn serve<S: WatchLog>(
    subscription: Subscription<S>,
    tx: &mpsc::Sender<SubscribeResponse>,
) -> Result<std::convert::Infallible, End> {
    let Subscription {
        store,
        filter,
        start,
        include_payload,
        period,
        mut head,
    } = subscription;
    let failed = |error| End::Failed(unavailable(error));

    let sleep = tokio::time::sleep(period);
    tokio::pin!(sleep);
    let mut caught_up = false;
    // `None` with `Oldest`, after a gap on an empty store, and with `Now` on an empty store.
    // Such a stream has handed the client no cursor, so the next read starts at the oldest
    // stored event and reports no gap. Events pruned before that read are not reported, which
    // is intended: the client holds no cursor that they would follow.
    let mut position = match start {
        Start::After(cursor) => Some(cursor),
        Start::Oldest => None,
        Start::Now => {
            head.borrow_and_update();
            let now = store.head().await.map_err(failed)?;
            send(
                tx,
                sleep.as_mut(),
                period,
                position_mark(MarkKind::CaughtUp, now),
            )
            .await?;
            caught_up = true;
            now
        }
    };

    loop {
        // Marked seen before the head and the batch are read, so an append that lands after
        // these reads wakes the wait below.
        head.borrow_and_update();
        let newest = store.head().await.map_err(failed)?;
        let page = store
            .read_after(position, &filter, SUBSCRIBE_BATCH_SIZE, include_payload)
            .await
            .map_err(failed)?;

        let gapped = page.gap.is_some();
        if let Some(gap) = page.gap {
            let mark = subscribe_response::Message::Mark(gap_mark(gap));
            send(tx, sleep.as_mut(), period, message(mark)).await?;
        }
        let full = page.events.len() >= SUBSCRIBE_BATCH_SIZE;
        let last = page
            .events
            .last()
            .map(event_cursor)
            .transpose()
            .map_err(End::Failed)?;
        for event in page.events {
            let event = subscribe_response::Message::Event(event);
            send(tx, sleep.as_mut(), period, message(event)).await?;
        }
        if full {
            position = last;
            continue;
        }
        // As `ListWatchEvents` does: past the head read before the batch, so filtered-out
        // events are not read again and their pruning reads as no gap.
        position = match later(last, newest) {
            Some(cursor) => Some(cursor),
            None if gapped => None,
            None => position,
        };
        if !caught_up {
            let mark = position_mark(MarkKind::CaughtUp, position);
            send(tx, sleep.as_mut(), period, mark).await?;
            caught_up = true;
        }

        loop {
            tokio::select! {
                biased;
                () = tx.closed() => return Err(End::Closed),
                changed = head.changed() => match changed {
                    Ok(()) => break,
                    Err(_) => {
                        return Err(End::Failed(Status::unavailable(
                            "the server is shutting down",
                        )));
                    }
                },
                () = &mut sleep => {
                    let mark = position_mark(MarkKind::Heartbeat, position);
                    send(tx, sleep.as_mut(), period, mark).await?;
                }
            }
        }
    }
}

/// A `Subscribe` response body: the task's messages, with its terminal status put first.
pub(super) struct Outbound {
    messages: mpsc::Receiver<SubscribeResponse>,
    /// `None` once the status was taken or its sender dropped.
    status: Option<oneshot::Receiver<Status>>,
    ended: bool,
}

impl Outbound {
    pub(super) fn new(
        messages: mpsc::Receiver<SubscribeResponse>,
        status: oneshot::Receiver<Status>,
    ) -> Self {
        Self {
            messages,
            status: Some(status),
            ended: false,
        }
    }

    /// The task's terminal status, once it has sent one.
    fn poll_status(&mut self, cx: &mut Context<'_>) -> Option<Status> {
        let status = self.status.as_mut()?;
        match Pin::new(status).poll(cx) {
            Poll::Ready(Ok(status)) => {
                self.status = None;
                Some(status)
            }
            Poll::Ready(Err(_)) => {
                self.status = None;
                None
            }
            Poll::Pending => None,
        }
    }
}

impl Stream for Outbound {
    type Item = Result<SubscribeResponse, Status>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        if this.ended {
            return Poll::Ready(None);
        }
        if let Some(status) = this.poll_status(cx) {
            this.ended = true;
            return Poll::Ready(Some(Err(status)));
        }
        match this.messages.poll_recv(cx) {
            Poll::Ready(Some(message)) => Poll::Ready(Some(Ok(message))),
            // The task sends its status before it drops its sender, so a status sent after
            // the first check is there now.
            Poll::Ready(None) => {
                this.ended = true;
                Poll::Ready(this.poll_status(cx).map(Err))
            }
            Poll::Pending => Poll::Pending,
        }
    }
}
