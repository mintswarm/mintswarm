//! A receiver for the GitHub App's webhooks. `POST /github/webhook` checks each delivery's
//! `X-Hub-Signature-256` against the webhook secret in constant time, answers `202` and appends
//! the verified delivery to a log file as one JSON line. A missing or bad signature gets `401`
//! and one JSON line on stderr without the body; a body over [`BODY_LIMIT`] gets `413`. At most
//! [`IN_FLIGHT_LIMIT`] requests are handled at once; one more gets `503` before its body is
//! read. Nothing is filtered by event type.

use std::ffi::OsString;
use std::fmt;
use std::fs::OpenOptions;
use std::io::{self, Write};
use std::net::SocketAddr;
use std::os::unix::ffi::OsStringExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use axum::Router;
use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Request, State};
use axum::http::{HeaderMap, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use hmac::{Hmac, KeyInit, Mac};
use mintswarm_github::{KeyStore, KeyStoreEnv, KeyStoreError, SecretBytes, Tools, read_key};
use serde::Serialize;
use serde_json::Value;
use sha2::Sha256;
use tokio::net::TcpListener;
use tokio::sync::Semaphore;

/// The path GitHub posts deliveries to.
pub const PATH: &str = "/github/webhook";

/// The largest body accepted, in bytes: GitHub's cap of 25 MB, read as 25 MiB.
pub const BODY_LIMIT: usize = 25 * 1024 * 1024;

/// The most requests handled at once. Each may hold a body of up to [`BODY_LIMIT`] before its
/// signature is checked, so this bounds what unauthenticated callers can make the receiver hold.
pub const IN_FLIGHT_LIMIT: usize = 8;

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

/// The proleptic Gregorian date `days` days after 1970-01-01, after Howard Hinnant's
/// `civil_from_days`, for non-negative days only.
fn civil_from_days(days: u64) -> (u64, u64, u64) {
    let z = days + 719_468;
    let era = z / 146_097;
    let day_of_era = z % 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    };
    let year = year_of_era + era * 400 + u64::from(month <= 2);
    (year, month, day)
}

/// One line of the delivery log, in this key order.
#[derive(Serialize)]
struct Line<'a> {
    received_at: &'a str,
    delivery: Option<String>,
    event: Option<String>,
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
    let payload = serde_json::from_slice::<Value>(body)
        .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(body).into_owned()));
    let line = Line {
        received_at,
        delivery: header_text(headers, DELIVERY_HEADER),
        event: header_text(headers, EVENT_HEADER),
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
/// its own and the next delivery's line is whole.
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
    /// Opens `path` for appending, creating it if needed.
    pub fn open(path: &Path) -> Result<Self, LogError> {
        let file = OpenOptions::new()
            .append(true)
            .create(true)
            .open(path)
            .map_err(|source| LogError::Open {
                path: path.to_owned(),
                reason: io_reason(&source),
                source,
            })?;
        Ok(Self::with_writer(path.to_owned(), file))
    }

    /// A log that appends to `writer`, reported under `path`. [`DeliveryLog::open`] passes the
    /// file it opened; tests pass a writer that fails.
    pub fn with_writer(path: PathBuf, writer: impl Write + Send + 'static) -> Self {
        Self {
            path,
            sink: Mutex::new(Sink {
                writer: Box::new(writer),
                fragment: false,
            }),
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

/// What the receiver's handler shares: the secret, the log and the permits for requests in
/// flight. Clones share all three.
#[derive(Clone, Debug)]
pub struct Receiver {
    secret: Arc<Secret>,
    log: Arc<DeliveryLog>,
    in_flight: Arc<Semaphore>,
}

impl Receiver {
    /// A receiver that checks signatures with `secret`, appends to `log` and handles at most
    /// [`IN_FLIGHT_LIMIT`] requests at once.
    pub fn new(secret: Secret, log: DeliveryLog) -> Self {
        Self::with_limit(secret, log, IN_FLIGHT_LIMIT)
    }

    /// As [`Receiver::new`], handling at most `limit` requests at once.
    pub fn with_limit(secret: Secret, log: DeliveryLog, limit: usize) -> Self {
        Self {
            secret: Arc::new(secret),
            log: Arc::new(log),
            in_flight: Arc::new(Semaphore::new(limit)),
        }
    }
}

/// The router: `POST /github/webhook`, with bodies capped at [`BODY_LIMIT`] and requests in
/// flight capped by the receiver's limit.
pub fn router(receiver: Receiver) -> Router {
    let in_flight = Arc::clone(&receiver.in_flight);
    Router::new()
        .route(PATH, post(receive))
        .layer(DefaultBodyLimit::max(BODY_LIMIT))
        .layer(middleware::from_fn_with_state(in_flight, limit_in_flight))
        .with_state(receiver)
}

/// Answers `503` without reading the body when every permit is taken; otherwise holds a permit
/// until the response is ready.
async fn limit_in_flight(
    State(in_flight): State<Arc<Semaphore>>,
    request: Request,
    next: Next,
) -> Response {
    let Ok(_permit) = in_flight.try_acquire_owned() else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    next.run(request).await
}

async fn receive(State(receiver): State<Receiver>, headers: HeaderMap, body: Bytes) -> StatusCode {
    let received_at = format_rfc3339_utc(SystemTime::now());
    let signature = headers.get(SIGNATURE_HEADER).map(|value| value.as_bytes());
    let verdict = verify(&receiver.secret, &body, signature);
    if let Some(rejected) = verdict.rejection() {
        report(&received_at, &headers, Outcome::Rejected(rejected));
        return StatusCode::UNAUTHORIZED;
    }
    let line = log_line(&received_at, &headers, &body);
    let log = Arc::clone(&receiver.log);
    match tokio::task::spawn_blocking(move || log.append(&line)).await {
        Ok(Ok(())) => StatusCode::ACCEPTED,
        Ok(Err(error)) => {
            report(&received_at, &headers, Outcome::Error(&error.to_string()));
            StatusCode::INTERNAL_SERVER_ERROR
        }
        Err(_) => {
            report(
                &received_at,
                &headers,
                Outcome::Error("the log write did not finish"),
            );
            StatusCode::INTERNAL_SERVER_ERROR
        }
    }
}

/// Writes one JSON line to stderr about a delivery that was not logged, without its body.
fn report(received_at: &str, headers: &HeaderMap, outcome: Outcome<'_>) {
    let entry = Report {
        received_at,
        outcome,
        delivery: header_text(headers, DELIVERY_HEADER),
        event: header_text(headers, EVENT_HEADER),
    };
    let line = serde_json::to_string(&entry).expect("a struct of strings always serialises");
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

/// Serves [`router`] on `listener` until the process stops.
pub async fn serve(listener: TcpListener, receiver: Receiver) -> Result<(), ServeError> {
    axum::serve(listener, router(receiver))
        .await
        .map_err(|source| ServeError::Serve {
            reason: io_reason(&source),
            source,
        })
}
