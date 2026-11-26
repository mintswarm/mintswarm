//! A receiver for the GitHub App's webhooks. `POST /github/webhook` checks each delivery's
//! `X-Hub-Signature-256` against the webhook secret in constant time. A verified delivery is put
//! on a bounded queue as it came and answered `202` at once; a separate task builds its JSON
//! line and drains the queue to the log file. A missing or bad signature gets `401` and one JSON
//! line on stderr without the body; a body over [`BODY_LIMIT`] gets `413`, and a request not
//! answered within [`Limits::read_timeout`] gets `408`. When the queue is full, or
//! [`Limits::in_flight`] requests are already being read, the request gets `429`. Nothing is
//! filtered by event type.

use std::ffi::OsString;
use std::fmt;
use std::fs::{File, OpenOptions};
use std::future::{Future, IntoFuture};
use std::io::{self, Write};
use std::net::SocketAddr;
use std::os::unix::ffi::OsStringExt;
use std::os::unix::fs::FileExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::Router;
use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Request, State};
use axum::http::{HeaderMap, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use hmac::{Hmac, KeyInit, Mac};
use mintswarm_domain::time::civil_from_days;
use mintswarm_github::{KeyStore, KeyStoreEnv, KeyStoreError, SecretBytes, Tools, read_key};
use serde::Serialize;
use serde_json::Value;
use sha2::Sha256;
use tokio::net::TcpListener;
use tokio::sync::mpsc::{self, error::TrySendError};
use tokio::sync::{Semaphore, oneshot};
use tokio::task::JoinHandle;

/// The path GitHub posts deliveries to.
pub const PATH: &str = "/github/webhook";

/// The largest body accepted, in bytes: GitHub's cap of 25 MB, read as 25 MiB.
pub const BODY_LIMIT: usize = 25 * 1024 * 1024;

/// The default for [`Limits::in_flight`]. A request holds its body, up to [`BODY_LIMIT`], until
/// its signature is checked, so callers without the secret can make the receiver hold at most
/// this many bodies: 16 x 25 MiB = 400 MiB. GitHub sends each delivery as one request and the
/// receiver answers as soon as the signature is checked, so GitHub's own requests rarely overlap.
pub const DEFAULT_IN_FLIGHT: usize = 16;

/// The default for [`Limits::queued`]. Only verified deliveries are queued, and the writer only
/// falls behind while the disk stalls; at GitHub's typical few-KiB payloads 256 lines is under a
/// few MiB, and it covers a burst such as a push to many pull requests at once.
pub const DEFAULT_QUEUED: usize = 256;

/// The default for [`Limits::read_timeout`]: GitHub's own delivery timeout. GitHub ends a delivery
/// it has not had an answer to within 10 seconds and shows it as failed, so a request still being
/// read after that is either not from GitHub or one GitHub has already given up on. Without the
/// deadline, [`Limits::in_flight`] callers that send headers and then stall the body would hold
/// every permit, and GitHub's deliveries would get `429` for as long as they kept them open.
pub const DEFAULT_READ_TIMEOUT: Duration = Duration::from_secs(10);

/// The header that carries the HMAC-SHA256 of the body.
pub const SIGNATURE_HEADER: &str = "x-hub-signature-256";

/// The header that carries the delivery's GUID.
pub const DELIVERY_HEADER: &str = "x-github-delivery";

/// The header that carries the event name.
pub const EVENT_HEADER: &str = "x-github-event";

/// The variable the secret is read from by default.
pub const DEFAULT_SECRET_ENV: &str = "MSW_WEBHOOK_SECRET";

/// The webhook secret. Its `Debug` output is redacted, and it has no `Display`.
#[derive(Clone)]
pub struct Secret(SecretBytes);

impl Secret {
    /// Wraps `bytes`, without one trailing `\n` or `\r\n`. `None` when nothing is left.
    pub fn new(mut bytes: Vec<u8>) -> Option<Self> {
        if bytes.ends_with(b"\r\n") {
            bytes.truncate(bytes.len() - 2);
        } else if bytes.ends_with(b"\n") {
            bytes.truncate(bytes.len() - 1);
        }
        if bytes.is_empty() {
            None
        } else {
            Some(Self(SecretBytes::new(bytes)))
        }
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("Secret").field(&self.0).finish()
    }
}

/// Where the webhook secret is read from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SecretSource {
    /// An environment variable.
    Env {
        /// The variable's name.
        var: String,
    },
    /// A key store that `key-store.sh` writes to.
    KeyStore {
        /// The store.
        store: KeyStore,
        /// The service name.
        service: String,
        /// The account name.
        account: String,
    },
}

/// Why the webhook secret could not be loaded. No variant carries the secret.
#[derive(Debug, thiserror::Error)]
pub enum SecretError {
    /// The variable is not set.
    #[error("the webhook secret variable {var} is not set")]
    Unset {
        /// The variable's name.
        var: String,
    },
    /// The source holds an empty value, or only a newline.
    #[error("the webhook secret from {from} is empty")]
    Empty {
        /// The variable's name, or `<store> key store <service>/<account>`.
        from: String,
    },
    /// The key store could not be read.
    #[error(
        "cannot read the webhook secret from the {store} key store {service}/{account}: {source}"
    )]
    KeyStore {
        /// The store's name.
        store: &'static str,
        /// The service name.
        service: String,
        /// The account name.
        account: String,
        /// Why.
        source: Box<KeyStoreError>,
    },
}

/// Loads the secret from `source`. `lookup` reads an environment variable, so that callers
/// choose the environment: the binary passes [`std::env::var_os`].
pub async fn load_secret(
    source: &SecretSource,
    lookup: impl Fn(&str) -> Option<OsString>,
) -> Result<Secret, SecretError> {
    match source {
        SecretSource::Env { var } => {
            let value = lookup(var).ok_or_else(|| SecretError::Unset { var: var.clone() })?;
            Secret::new(value.into_vec()).ok_or_else(|| SecretError::Empty { from: var.clone() })
        }
        SecretSource::KeyStore {
            store,
            service,
            account,
        } => {
            let failed = |source| SecretError::KeyStore {
                store: store.name(),
                service: service.clone(),
                account: account.clone(),
                source: Box::new(source),
            };
            let env = KeyStoreEnv::from_process().map_err(failed)?;
            let bytes = read_key(*store, service, account, &Tools::default(), &env)
                .await
                .map_err(failed)?;
            Secret::new(bytes.expose().to_vec()).ok_or_else(|| SecretError::Empty {
                from: format!("{} key store {service}/{account}", store.name()),
            })
        }
    }
}

/// The outcome of a signature check.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// The signature matches the body.
    Valid,
    /// There is no signature header.
    Missing,
    /// The header is malformed, or its signature does not match.
    Bad,
}

impl Verdict {
    /// The name a rejection is logged under, `None` for [`Verdict::Valid`].
    fn rejection(self) -> Option<&'static str> {
        match self {
            Self::Valid => None,
            Self::Missing => Some("missing_signature"),
            Self::Bad => Some("bad_signature"),
        }
    }
}

/// Parses `sha256=<64 hex digits>`, in either case, into the 32 signature bytes.
pub fn parse_signature(header: &[u8]) -> Option<[u8; 32]> {
    let hex = header.strip_prefix(b"sha256=")?;
    if hex.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (byte, pair) in out.iter_mut().zip(hex.chunks_exact(2)) {
        *byte = (hex_digit(pair[0])? << 4) | hex_digit(pair[1])?;
    }
    Some(out)
}

fn hex_digit(digit: u8) -> Option<u8> {
    match digit {
        b'0'..=b'9' => Some(digit - b'0'),
        b'a'..=b'f' => Some(digit - b'a' + 10),
        b'A'..=b'F' => Some(digit - b'A' + 10),
        _ => None,
    }
}

/// Checks `header`, the `X-Hub-Signature-256` value if there is one, against the HMAC-SHA256
/// of `body` under `secret`. The comparison is `Mac::verify_slice`, which runs in constant time.
pub fn verify(secret: &Secret, body: &[u8], header: Option<&[u8]>) -> Verdict {
    let Some(header) = header else {
        return Verdict::Missing;
    };
    let Some(signature) = parse_signature(header) else {
        return Verdict::Bad;
    };
    let Ok(mut mac) = Hmac::<Sha256>::new_from_slice(secret.0.expose()) else {
        // HMAC takes keys of any length, so this does not happen; refuse rather than accept.
        return Verdict::Bad;
    };
    mac.update(body);
    match mac.verify_slice(&signature) {
        Ok(()) => Verdict::Valid,
        Err(_) => Verdict::Bad,
    }
}

/// Formats `time` as RFC 3339 in UTC with milliseconds, such as `2024-02-29T12:34:56.789Z`.
/// A time before the Unix epoch is given as the epoch.
pub fn format_rfc3339_utc(time: SystemTime) -> String {
    let since = time.duration_since(UNIX_EPOCH).unwrap_or_default();
    let secs = since.as_secs();
    let (year, month, day) = civil_from_days(secs / 86_400);
    let of_day = secs % 86_400;
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:03}Z",
        of_day / 3600,
        of_day % 3600 / 60,
        of_day % 60,
        since.subsec_millis()
    )
}

/// One line of the delivery log, in this key order.
#[derive(Serialize)]
struct Line<'a> {
    received_at: &'a str,
    delivery: Option<&'a str>,
    event: Option<&'a str>,
    action: &'a Value,
    installation: &'a Value,
    repository: &'a Value,
    payload: &'a Value,
}

/// The log line for a verified delivery: one compact JSON object ending in `\n`, with the keys
/// `received_at`, `delivery` (`X-GitHub-Delivery`), `event` (`X-GitHub-Event`), `action`
/// (`payload.action`), `installation` (`payload.installation.id`), `repository`
/// (`payload.repository.full_name`) and `payload`. A missing header or payload key is `null`.
/// A body that is not JSON is kept in `payload` as a string.
pub fn log_line(received_at: &str, headers: &HeaderMap, body: &[u8]) -> Vec<u8> {
    delivery_line(
        received_at,
        header_text(headers, DELIVERY_HEADER).as_deref(),
        header_text(headers, EVENT_HEADER).as_deref(),
        body,
    )
}

/// [`log_line`] from the header strings already taken from the request: `delivery` and `event`
/// are the `X-GitHub-Delivery` and `X-GitHub-Event` values, if any. The writer task calls it, so
/// the body is parsed after the delivery has been answered.
pub fn delivery_line(
    received_at: &str,
    delivery: Option<&str>,
    event: Option<&str>,
    body: &[u8],
) -> Vec<u8> {
    let payload = serde_json::from_slice::<Value>(body)
        .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(body).into_owned()));
    let line = Line {
        received_at,
        delivery,
        event,
        action: payload.pointer("/action").unwrap_or(&Value::Null),
        installation: payload.pointer("/installation/id").unwrap_or(&Value::Null),
        repository: payload
            .pointer("/repository/full_name")
            .unwrap_or(&Value::Null),
        payload: &payload,
    };
    let mut out =
        serde_json::to_vec(&line).expect("a struct of strings and JSON values always serialises");
    out.push(b'\n');
    out
}

fn header_text(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .map(|value| String::from_utf8_lossy(value.as_bytes()).into_owned())
}

/// Why the delivery log could not be opened or written.
#[derive(Debug, thiserror::Error)]
pub enum LogError {
    /// The file could not be opened for appending.
    #[error("cannot open the delivery log {}: {reason}", .path.display())]
    Open {
        /// The log's path.
        path: PathBuf,
        /// Why, in our words.
        reason: &'static str,
        /// The underlying error.
        source: io::Error,
    },
    /// A line could not be written whole.
    #[error("cannot append to the delivery log {}: {reason}", .path.display())]
    Write {
        /// The log's path.
        path: PathBuf,
        /// Why, in our words.
        reason: &'static str,
        /// The underlying error.
        source: io::Error,
    },
    /// A previous write panicked while holding the log.
    #[error("the delivery log {} is unusable after an earlier failure", .path.display())]
    Poisoned {
        /// The log's path.
        path: PathBuf,
    },
}

/// Our own words for an I/O failure.
fn io_reason(error: &io::Error) -> &'static str {
    match error.kind() {
        io::ErrorKind::NotFound => "a directory on its path does not exist",
        io::ErrorKind::PermissionDenied => "permission denied",
        io::ErrorKind::IsADirectory => "it is a directory",
        io::ErrorKind::StorageFull => "the disk is full",
        _ => "an I/O error",
    }
}

/// The delivery log: a file opened in append mode, created if absent. Each line is written
/// under a mutex, so lines never interleave. When a write fails after part of a line was
/// written, the next append first ends that fragment with `\n`, so the fragment stays a line of
/// its own and the next delivery's line is whole. The same holds for a fragment an earlier
/// process left when it was killed part-way through a line: [`DeliveryLog::open`] finds it.
pub struct DeliveryLog {
    path: PathBuf,
    sink: Mutex<Sink>,
}

impl fmt::Debug for DeliveryLog {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DeliveryLog")
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

/// The writer and whether it ends in a fragment of a line.
struct Sink {
    writer: Box<dyn Write + Send>,
    fragment: bool,
}

impl Sink {
    /// Writes all of `bytes`, setting `fragment` when it stops after writing only part of them.
    fn write_whole(&mut self, bytes: &[u8]) -> io::Result<()> {
        let mut written = 0;
        while written < bytes.len() {
            match self.writer.write(&bytes[written..]) {
                Ok(0) => {
                    self.fragment |= written > 0;
                    return Err(io::ErrorKind::WriteZero.into());
                }
                Ok(count) => written += count,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(error) => {
                    self.fragment |= written > 0;
                    return Err(error);
                }
            }
        }
        Ok(())
    }

    /// Ends a fragment left by a failed write, then writes `line`.
    fn append(&mut self, line: &[u8]) -> io::Result<()> {
        if self.fragment {
            self.write_whole(b"\n")?;
            self.fragment = false;
        }
        self.write_whole(line)?;
        self.writer.flush()
    }
}

impl DeliveryLog {
    /// Opens `path` for appending, creating it if needed. When the file is not empty and does
    /// not end in `\n`, an earlier process stopped part-way through a line, and the first append
    /// ends that fragment first.
    pub fn open(path: &Path) -> Result<Self, LogError> {
        let failed = |source: io::Error| LogError::Open {
            path: path.to_owned(),
            reason: io_reason(&source),
            source,
        };
        let file = OpenOptions::new()
            .read(true)
            .append(true)
            .create(true)
            .open(path)
            .map_err(failed)?;
        let fragment = ends_in_fragment(&file).map_err(failed)?;
        Ok(Self::with_sink(path.to_owned(), Box::new(file), fragment))
    }

    /// A log that appends to `writer`, reported under `path`. [`DeliveryLog::open`] passes the
    /// file it opened; tests pass a writer that fails.
    pub fn with_writer(path: PathBuf, writer: impl Write + Send + 'static) -> Self {
        Self::with_sink(path, Box::new(writer), false)
    }

    fn with_sink(path: PathBuf, writer: Box<dyn Write + Send>, fragment: bool) -> Self {
        Self {
            path,
            sink: Mutex::new(Sink { writer, fragment }),
        }
    }

    /// The log's path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Appends `line`, which ends in `\n`, first ending any fragment a failed write left.
    pub fn append(&self, line: &[u8]) -> Result<(), LogError> {
        let mut sink = self.sink.lock().map_err(|_| LogError::Poisoned {
            path: self.path.clone(),
        })?;
        sink.append(line).map_err(|source| LogError::Write {
            path: self.path.clone(),
            reason: io_reason(&source),
            source,
        })
    }
}

/// Whether `file` is not empty and its last byte is not `\n`.
fn ends_in_fragment(file: &File) -> io::Result<bool> {
    let len = file.metadata()?.len();
    if len == 0 {
        return Ok(false);
    }
    let mut last = [0u8; 1];
    file.read_exact_at(&mut last, len - 1)?;
    Ok(last[0] != b'\n')
}

/// The receiver's bounds. The counts are at least 1; a 0 is read as 1.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    /// The most requests whose bodies are read and checked at once.
    pub in_flight: usize,
    /// The most verified deliveries waiting for the log.
    pub queued: usize,
    /// How long a request may take from taking its permit to its answer, which is mostly
    /// reading the body; past it the request gets `408`.
    pub read_timeout: Duration,
}

impl Limits {
    /// How long a stop waits for the requests in progress: twice [`Limits::read_timeout`], one
    /// for a request head still arriving when the stop came and one for the body read after it,
    /// which the read timeout bounds. Past it the stop ends without them; see [`serve`].
    pub fn stop_deadline(&self) -> Duration {
        self.read_timeout.saturating_mul(2)
    }
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            in_flight: DEFAULT_IN_FLIGHT,
            queued: DEFAULT_QUEUED,
            read_timeout: DEFAULT_READ_TIMEOUT,
        }
    }
}

/// A verified delivery waiting for the log, as it came: the writer task builds its line.
struct Queued {
    received_at: String,
    delivery: Option<String>,
    event: Option<String>,
    body: Bytes,
}

/// What goes on the queue: a delivery, or the stop, after which the writer takes no more
/// deliveries but still writes the ones already queued.
#[derive(Debug)]
enum Item {
    Delivery(Queued),
    Stop,
}

/// What the receiver's handler shares: the secret, the queue to the log and the gate on
/// requests in flight. Clones share all three.
#[derive(Clone, Debug)]
pub struct Receiver {
    secret: Arc<Secret>,
    queue: mpsc::Sender<Item>,
    gate: Gate,
    limits: Limits,
}

/// The permits for requests in flight, and how long each may hold one.
#[derive(Clone, Debug)]
struct Gate {
    permits: Arc<Semaphore>,
    read_timeout: Duration,
}

impl fmt::Debug for Queued {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Queued")
            .field("delivery", &self.delivery)
            .finish_non_exhaustive()
    }
}

/// The task that drains the queue to the log. It ends once the queue is empty and either every
/// [`Receiver`] clone is dropped or [`serve`] has closed the queue. If it panics, the queue
/// closes with it: what it still held is lost, and later verified deliveries get `500` and a
/// stderr line; [`Writer::finished`] then fails.
#[derive(Debug)]
pub struct Writer(JoinHandle<()>);

impl Writer {
    /// Waits until the queue is drained and the writer has ended. Fails with
    /// [`ServeError::Writer`] when the writer ended without draining it, as when it panicked:
    /// the deliveries it still held were answered `202` but may not be in the log.
    pub async fn finished(self) -> Result<(), ServeError> {
        self.0.await.map_err(|error| ServeError::Writer {
            reason: if error.is_panic() {
                "it panicked"
            } else {
                "it was cancelled"
            },
        })
    }
}

impl Receiver {
    /// Starts the writer that drains the queue to `log`, and returns a receiver that checks
    /// signatures with `secret` within `limits`. Call it inside a Tokio runtime.
    pub fn start(secret: Secret, log: DeliveryLog, limits: Limits) -> (Self, Writer) {
        let (queue, mut drain) = mpsc::channel::<Item>(limits.queued.max(1));
        let writer = tokio::task::spawn_blocking(move || {
            while let Some(item) = drain.blocking_recv() {
                let queued = match item {
                    Item::Delivery(queued) => queued,
                    Item::Stop => {
                        // Deliveries already queued are still received; new ones are refused.
                        drain.close();
                        continue;
                    }
                };
                let line = delivery_line(
                    &queued.received_at,
                    queued.delivery.as_deref(),
                    queued.event.as_deref(),
                    &queued.body,
                );
                if let Err(error) = log.append(&line) {
                    report_queued(&queued, Outcome::Error(&error.to_string()));
                }
            }
        });
        let receiver = Self {
            secret: Arc::new(secret),
            queue,
            gate: Gate {
                permits: Arc::new(Semaphore::new(limits.in_flight.max(1))),
                read_timeout: limits.read_timeout,
            },
            limits,
        };
        (receiver, Writer(writer))
    }
}

/// The router: `POST /github/webhook`, with bodies capped at [`BODY_LIMIT`], requests in
/// flight capped by the receiver's [`Limits::in_flight`], and each request limited to its
/// [`Limits::read_timeout`].
pub fn router(receiver: Receiver) -> Router {
    let gate = receiver.gate.clone();
    Router::new()
        .route(PATH, post(receive))
        .layer(DefaultBodyLimit::max(BODY_LIMIT))
        .layer(middleware::from_fn_with_state(gate, limit_in_flight))
        .with_state(receiver)
}

/// Answers `429` without reading the body when every permit is taken. Otherwise holds a permit
/// until the response is ready, and answers `408`, with a stderr line, when that takes longer
/// than the read timeout. Once the body is read the handler runs to its answer without waiting
/// on anything, so a delivery that was queued is never answered `408`.
async fn limit_in_flight(State(gate): State<Gate>, request: Request, next: Next) -> Response {
    let Ok(_permit) = gate.permits.try_acquire_owned() else {
        return StatusCode::TOO_MANY_REQUESTS.into_response();
    };
    let received_at = format_rfc3339_utc(SystemTime::now());
    let delivery = header_text(request.headers(), DELIVERY_HEADER);
    let event = header_text(request.headers(), EVENT_HEADER);
    match tokio::time::timeout(gate.read_timeout, next.run(request)).await {
        Ok(response) => response,
        Err(_) => {
            write_report(&Report {
                received_at: &received_at,
                outcome: Outcome::Rejected("read_timeout"),
                delivery,
                event,
            });
            StatusCode::REQUEST_TIMEOUT.into_response()
        }
    }
}

/// Checks the signature, queues a verified delivery for the log, and answers at once: `202`
/// when queued, `401` for a missing or bad signature, `429` when the queue is full. Only the
/// HMAC runs here; the writer task parses the body and builds the line.
async fn receive(State(receiver): State<Receiver>, headers: HeaderMap, body: Bytes) -> StatusCode {
    let received_at = format_rfc3339_utc(SystemTime::now());
    let signature = headers.get(SIGNATURE_HEADER).map(|value| value.as_bytes());
    let verdict = verify(&receiver.secret, &body, signature);
    if let Some(rejected) = verdict.rejection() {
        report(&received_at, &headers, Outcome::Rejected(rejected));
        return StatusCode::UNAUTHORIZED;
    }
    let queued = Queued {
        delivery: header_text(&headers, DELIVERY_HEADER),
        event: header_text(&headers, EVENT_HEADER),
        received_at,
        body,
    };
    let (item, outcome, status) = match receiver.queue.try_send(Item::Delivery(queued)) {
        Ok(()) => return StatusCode::ACCEPTED,
        Err(TrySendError::Full(item)) => (
            item,
            Outcome::Rejected("queue_full"),
            StatusCode::TOO_MANY_REQUESTS,
        ),
        Err(TrySendError::Closed(item)) => (
            item,
            Outcome::Error("the log writer has stopped"),
            StatusCode::INTERNAL_SERVER_ERROR,
        ),
    };
    if let Item::Delivery(queued) = &item {
        report_queued(queued, outcome);
    }
    status
}

/// Writes one JSON line to stderr about a queued delivery that was not logged, without its line.
fn report_queued(queued: &Queued, outcome: Outcome<'_>) {
    write_report(&Report {
        received_at: &queued.received_at,
        outcome,
        delivery: queued.delivery.clone(),
        event: queued.event.clone(),
    });
}

/// Writes one JSON line to stderr about a delivery that was not logged, without its body.
fn report(received_at: &str, headers: &HeaderMap, outcome: Outcome<'_>) {
    write_report(&Report {
        received_at,
        outcome,
        delivery: header_text(headers, DELIVERY_HEADER),
        event: header_text(headers, EVENT_HEADER),
    });
}

fn write_report(report: &Report<'_>) {
    let line = serde_json::to_string(report).expect("a struct of strings always serialises");
    eprintln!("{line}");
}

/// A stderr line about a delivery that was not logged, in this key order.
#[derive(Serialize)]
struct Report<'a> {
    received_at: &'a str,
    #[serde(flatten)]
    outcome: Outcome<'a>,
    delivery: Option<String>,
    event: Option<String>,
}

/// Why a delivery was not logged: `{"rejected": …}` or `{"error": …}`.
#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
enum Outcome<'a> {
    Rejected(&'static str),
    Error(&'a str),
}

/// Why the receiver could not listen or stopped.
#[derive(Debug, thiserror::Error)]
pub enum ServeError {
    /// The address could not be bound.
    #[error("cannot listen on {addr}: {reason}")]
    Bind {
        /// The address.
        addr: SocketAddr,
        /// Why, in our words.
        reason: &'static str,
        /// The underlying error.
        source: io::Error,
    },
    /// The server stopped with an error.
    #[error("the webhook receiver stopped: {reason}")]
    Serve {
        /// Why, in our words.
        reason: &'static str,
        /// The underlying error.
        source: io::Error,
    },
    /// The writer ended without draining the queue. Deliveries it still held were answered
    /// `202` but may not be in the log, and requests after it ended got `500`.
    #[error(
        "the log writer failed ({reason}); deliveries answered 202 may be missing from the log"
    )]
    Writer {
        /// Why, in our words. The panic's own message is not carried.
        reason: &'static str,
    },
}

/// Binds `addr`.
pub async fn bind(addr: SocketAddr) -> Result<TcpListener, ServeError> {
    TcpListener::bind(addr)
        .await
        .map_err(|source| ServeError::Bind {
            addr,
            reason: match source.kind() {
                io::ErrorKind::AddrInUse => "the address is in use",
                io::ErrorKind::AddrNotAvailable => "the address is not available here",
                io::ErrorKind::PermissionDenied => "permission denied",
                _ => "an I/O error",
            },
            source,
        })
}

/// How a stop ended. In each case the deliveries already queued were drained to the log, or
/// reported on stderr, before [`serve`] returned.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stopped {
    /// Every request in progress was answered.
    Drained,
    /// [`Limits::stop_deadline`] passed with requests still open, such as a connection that
    /// stalled before its request head was complete; they were left unanswered.
    DeadlinePassed,
    /// `force` resolved with requests still open; they were left unanswered.
    Forced,
}

/// Serves [`router`] on `listener` until `shutdown` resolves. It then stops accepting and lets
/// the requests in progress finish, until they all have, [`Limits::stop_deadline`] has passed,
/// or `force` resolves, whichever comes first; `force` is only awaited after `shutdown`. It
/// then closes the queue, so a request still open that later passes its signature check gets
/// `500` and a stderr line, and waits for `writer` to drain what was queued, so every delivery
/// answered `202` is written, or reported on stderr, before it returns. When the writer failed
/// instead, it returns [`ServeError::Writer`], even if the server also stopped with an error:
/// deliveries answered `202` may have been lost, which matters most.
pub async fn serve(
    listener: TcpListener,
    receiver: Receiver,
    writer: Writer,
    shutdown: impl Future<Output = ()> + Send + 'static,
    force: impl Future<Output = ()> + Send,
) -> Result<Stopped, ServeError> {
    let deadline = receiver.limits.stop_deadline();
    let queue = receiver.queue.clone();
    let (stopping, stop_started) = oneshot::channel::<()>();
    let server = axum::serve(listener, router(receiver))
        .with_graceful_shutdown(async move {
            shutdown.await;
            let _ = stopping.send(());
        })
        .into_future();
    let bound = async move {
        if stop_started.await.is_err() {
            // The server ended without a stop; its own result decides.
            return std::future::pending().await;
        }
        tokio::select! {
            () = tokio::time::sleep(deadline) => Stopped::DeadlinePassed,
            () = force => Stopped::Forced,
        }
    };
    let (served, stopped) = tokio::select! {
        served = server => (served, Stopped::Drained),
        stopped = bound => (Ok(()), stopped),
    };
    // Connections left open still hold receivers, so the writer is told to stop taking
    // deliveries; it ends once what is queued is written. If the writer has already ended
    // there is nothing to stop. It is awaited after an error too, to write what was answered.
    let _ = queue.send(Item::Stop).await;
    drop(queue);
    writer.finished().await?;
    served
        .map(|()| stopped)
        .map_err(|source| ServeError::Serve {
            reason: io_reason(&source),
            source,
        })
}
