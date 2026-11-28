//! The one retry layer every GitHub request goes through.
//!
//! [`Retrying`] owns a single-attempt [`Transport`] and offers only calls typed by whether they
//! are safe to repeat: [`get`](Retrying::get), [`delete`](Retrying::delete) and
//! [`post_idempotent`](Retrying::post_idempotent) are retried as they are;
//! [`patch`](Retrying::patch) and [`post_idempotent_gated`](Retrying::post_idempotent_gated) are
//! retried as they are behind a gate, as a create is; [`put`](Retrying::put) is never sent again
//! after an attempt that may have been carried out; [`create`](Retrying::create) cannot be called
//! without a probe that tells whether an earlier attempt landed. The transport trait is sealed and
//! its only real implementation cannot be built outside `rest.rs`, so no request reaches GitHub
//! around the layer.
//!
//! A success, a 304 or a 4xx that is not a rate limit is returned at once, and a request that
//! could not be built ([`Failure::Request`]) ends the call after its one attempt. A 5xx, a
//! timeout, a broken connection or an unreadable body may have been carried out, so an
//! idempotent call waits and tries again, a create first asks its probe, and a PUT ends in
//! [`Exhausted`] with that failure as its last. A refused
//! connection and a rate-limited reply were not carried out, so they are retried without a
//! probe. Each wait is [`RetryPolicy::jittered`], raised to the wait a rate-limited reply names
//! in `retry-after` or `x-ratelimit-reset`, or to [`SECONDARY_LIMIT_WAIT`], doubled for each
//! further such refusal, when it names none. A pausing layer shares a rate limit's wait with
//! every later call as a deadline, which a retry's own wait never adds to.
//!
//! The policy's total bounds the call: an attempt still running when it ends is cut off as a
//! [`Failure::Timeout`], and a wait that would end past it is not started. When the attempts or
//! the total run out, the call gives up with [`Exhausted`]. A create's probe is the caller's own
//! call, not bounded by the create's total; its gate is, and a gate still waiting when the total
//! ends is cut off and ends the call. A gated idempotent call's gate is bounded the same way.

use std::fmt;
use std::future::Future;
use std::hash::BuildHasher;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::Duration;

use reqwest::Method;
use serde_json::Value;
use tokio::time::Instant;

use mintswarm_domain::retry::{Exhausted, Failure, RetryPolicy};

use crate::rest::Reply;
use crate::secret::Token;

/// How long to wait after a rate-limited reply that names no wait: GitHub's "Rate limits for the
/// REST API" page (<https://docs.github.com/en/rest/using-the-rest-api/rate-limits-for-the-rest-api>),
/// "Otherwise, wait for at least one minute before retrying". The page also asks for waits
/// growing exponentially on repeated secondary limits, so each further such refusal within one
/// call doubles it.
pub const SECONDARY_LIMIT_WAIT: Duration = Duration::from_secs(60);

mod sealed {
    /// Only the types this module names can be transports.
    pub trait Sealed {}
}

impl sealed::Sealed for crate::rest::Http {}

/// One request, as the layer hands it to its transport for each attempt.
#[derive(Debug)]
pub(crate) struct Request<'a> {
    pub method: Method,
    pub path: &'a str,
    pub token: &'a Token,
    pub body: Option<&'a Value>,
    pub if_none_match: Option<&'a str>,
}

impl Request<'_> {
    /// The method and path, for errors and logs; never the token.
    pub fn endpoint(&self) -> String {
        format!("{} {}", self.method, self.path)
    }
}

/// The outcome of one attempt: GitHub's reply, whatever its status, or a [`Failure`] of
/// [`Connect`](Failure::Connect), [`Timeout`](Failure::Timeout), [`Reset`](Failure::Reset),
/// [`Body`](Failure::Body) or [`Request`](Failure::Request).
pub(crate) type Attempt = Result<Reply, Failure>;

/// Sends one attempt of a request and nothing more. Sealed: only [`Retrying`] calls it.
pub(crate) trait Transport: sealed::Sealed + Send + Sync {
    /// One attempt at `request`.
    fn send(&self, request: &Request<'_>) -> impl Future<Output = Attempt> + Send;
}

/// What a create's probe found out about an earlier attempt.
pub(crate) enum Landing {
    /// It landed; this is what GitHub holds for it.
    Landed(Reply),
    /// It did not land, so sending again cannot make a duplicate.
    NotLanded,
    /// The probe could not tell; the call gives up rather than risk a duplicate.
    Unknown,
}

/// Asks GitHub whether an earlier attempt of a create landed, for example by looking up a
/// marker the create's body carries.
pub(crate) type Probe<'a> =
    Box<dyn Fn() -> Pin<Box<dyn Future<Output = Landing> + Send + 'a>> + Send + Sync + 'a>;

/// Waits before each attempt of a create or a gated idempotent call after the first, for
/// example for a grant from a budget of content-creating requests.
pub(crate) type Gate<'a> =
    Box<dyn Fn() -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> + Send + Sync + 'a>;

/// Whether a request is safe to repeat.
enum Class<'a> {
    /// Repeating it leaves GitHub as one success would.
    Idempotent,
    /// Repeating it leaves GitHub as one success would, but each attempt after the first waits
    /// on its gate.
    Gated(Gate<'a>),
    /// Repeating it after it landed would make a second one; its probe and its gate.
    Create(Probe<'a>, Gate<'a>),
    /// Repeating it after it landed would answer as a refusal of the first, so it is sent
    /// again only after a failure that was not carried out.
    Once,
}

/// A draw from `[0, 1]` for the jitter of a wait.
pub(crate) type Jitter = Arc<dyn Fn() -> f64 + Send + Sync>;

/// A draw from `[0, 1)`: a hash of a process-wide counter under a randomly keyed
/// [`RandomState`](std::hash::RandomState). It only spreads retries apart; it is not for
/// secrets.
fn random_unit() -> f64 {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    static STATE: OnceLock<std::hash::RandomState> = OnceLock::new();
    let draw = STATE
        .get_or_init(std::hash::RandomState::new)
        .hash_one(COUNTER.fetch_add(1, Ordering::Relaxed));
    // The top 53 bits, which an f64 holds exactly, over 2^53.
    (draw >> 11) as f64 / (1u64 << 53) as f64
}

/// The retry layer over a single-attempt transport `T`; see the module documentation.
pub(crate) struct Retrying<T> {
    transport: T,
    policy: RetryPolicy,
    jitter: Jitter,
    /// For a pausing layer, the instant before which no request is sent; `None` inside when no
    /// pause was asked for yet.
    pause: Option<Mutex<Option<Instant>>>,
}

impl<T: fmt::Debug> fmt::Debug for Retrying<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Retrying")
            .field("transport", &self.transport)
            .field("policy", &self.policy)
            .field("pausing", &self.pause.is_some())
            .finish_non_exhaustive()
    }
}

impl<T: Transport> Retrying<T> {
    /// The layer over `transport` with `policy`; a `pausing` layer makes every later call wait
    /// out a rate limit's named wait too.
    pub fn wrap(transport: T, policy: RetryPolicy, pausing: bool) -> Self {
        Self {
            transport,
            policy,
            jitter: Arc::new(random_unit),
            pause: pausing.then(|| Mutex::new(None)),
        }
    }

    /// The layer with `policy` in place of its own.
    pub fn with_policy(mut self, policy: RetryPolicy) -> Self {
        self.policy = policy;
        self
    }

    /// The layer drawing its jitter from `jitter`.
    #[cfg(test)]
    fn with_jitter(mut self, jitter: impl Fn() -> f64 + Send + Sync + 'static) -> Self {
        self.jitter = Arc::new(jitter);
        self
    }

    /// A GET, with `If-None-Match` when an ETag is given.
    pub async fn get(
        &self,
        path: &str,
        token: &Token,
        if_none_match: Option<&str>,
    ) -> Result<Reply, Exhausted> {
        let request = Request {
            method: Method::GET,
            path,
            token,
            body: None,
            if_none_match,
        };
        self.run(&request, Class::Idempotent).await
    }

    /// A PUT, such as a merge with its `sha` pinned. A merge that landed answers a second
    /// attempt with a refusal, so the PUT is sent again only after a refused connection or a
    /// rate limit; after any failure that may have been carried out the call ends in
    /// [`Exhausted`] with that failure as its last.
    pub async fn put(&self, path: &str, token: &Token, body: &Value) -> Result<Reply, Exhausted> {
        let request = Request {
            method: Method::PUT,
            path,
            token,
            body: Some(body),
            if_none_match: None,
        };
        self.run(&request, Class::Once).await
    }

    /// A DELETE. A 404 after an attempt that may have landed is the resource that attempt
    /// removed; the reply says so in [`Reply::after_ambiguous`].
    pub async fn delete(&self, path: &str, token: &Token) -> Result<Reply, Exhausted> {
        let request = Request {
            method: Method::DELETE,
            path,
            token,
            body: None,
            if_none_match: None,
        };
        self.run(&request, Class::Idempotent).await
    }

    /// A DELETE with a JSON body that changes content and leaves GitHub as one success would
    /// however often it is repeated, such as removing assignees. `gate` is awaited before every
    /// attempt after the first, as a create's is. Each caller says next to the call why
    /// repeating is safe.
    pub async fn delete_gated<'a>(
        &self,
        path: &str,
        token: &Token,
        body: &Value,
        gate: Gate<'a>,
    ) -> Result<Reply, Exhausted> {
        let request = Request {
            method: Method::DELETE,
            path,
            token,
            body: Some(body),
            if_none_match: None,
        };
        self.run(&request, Class::Gated(gate)).await
    }

    /// A POST that leaves GitHub as one success would however often it is repeated, such as
    /// setting a value or adding a label. Each caller says next to the call why that holds.
    pub async fn post_idempotent(
        &self,
        path: &str,
        token: &Token,
        body: &Value,
    ) -> Result<Reply, Exhausted> {
        let request = Request {
            method: Method::POST,
            path,
            token,
            body: Some(body),
            if_none_match: None,
        };
        self.run(&request, Class::Idempotent).await
    }

    /// A PATCH that sets fields of a resource to `body` whatever they held, such as a comment's
    /// body. `gate` is awaited before every attempt after the first, as a create's is.
    pub async fn patch<'a>(
        &self,
        path: &str,
        token: &Token,
        body: &Value,
        gate: Gate<'a>,
    ) -> Result<Reply, Exhausted> {
        let request = Request {
            method: Method::PATCH,
            path,
            token,
            body: Some(body),
            if_none_match: None,
        };
        self.run(&request, Class::Gated(gate)).await
    }

    /// A POST that is safe to repeat, as [`post_idempotent`](Retrying::post_idempotent) is, but
    /// that changes content, such as a reaction. `gate` is awaited before every attempt after
    /// the first, as a create's is. Each caller says next to the call why repeating is safe.
    pub async fn post_idempotent_gated<'a>(
        &self,
        path: &str,
        token: &Token,
        body: &Value,
        gate: Gate<'a>,
    ) -> Result<Reply, Exhausted> {
        let request = Request {
            method: Method::POST,
            path,
            token,
            body: Some(body),
            if_none_match: None,
        };
        self.run(&request, Class::Gated(gate)).await
    }

    /// A POST that creates something, such as a comment, an artefact or a pull request. After
    /// an attempt that may have landed, `probe` is asked before the next one is sent: a landed
    /// attempt's reply is returned as the call's, and a probe that cannot tell ends the call.
    /// When the attempts or the time run out after such an attempt, the call ends in
    /// [`Exhausted`] without asking, so the create may still have landed. `gate` is awaited
    /// before every attempt after the first, whatever ended the one before; the caller makes
    /// the first attempt's wait itself. A gate still waiting when the total ends is cut off and
    /// ends the call, so it must take nothing until it completes.
    pub async fn create<'a>(
        &self,
        path: &str,
        token: &Token,
        body: &Value,
        probe: Probe<'a>,
        gate: Gate<'a>,
    ) -> Result<Reply, Exhausted> {
        let request = Request {
            method: Method::POST,
            path,
            token,
            body: Some(body),
            if_none_match: None,
        };
        self.run(&request, Class::Create(probe, gate)).await
    }

    /// The instant before which a pausing layer sends nothing, if it is still ahead.
    fn deadline(&self) -> Option<Instant> {
        let pause = self.pause.as_ref()?;
        let deadline = *pause.lock().unwrap_or_else(PoisonError::into_inner);
        deadline.filter(|deadline| *deadline > Instant::now())
    }

    /// What is left of a pausing layer's pause; zero when there is none.
    pub fn pause_remaining(&self) -> Duration {
        self.deadline()
            .map_or(Duration::ZERO, |deadline| deadline - Instant::now())
    }

    /// Moves a pausing layer's deadline to `wait` from now, never earlier than it is.
    fn pause_for(&self, wait: Duration) {
        let Some(pause) = &self.pause else {
            return;
        };
        // A wait too long for the clock is no pause anyone could wait out; the call that met it
        // gives up on it.
        let Some(until) = Instant::now().checked_add(wait) else {
            return;
        };
        let mut deadline = pause.lock().unwrap_or_else(PoisonError::into_inner);
        if deadline.is_none_or(|deadline| deadline < until) {
            *deadline = Some(until);
        }
    }

    async fn run(&self, request: &Request<'_>, class: Class<'_>) -> Result<Reply, Exhausted> {
        // `None` when the total is too long for the clock to reach: no bound in practice.
        let end = Instant::now().checked_add(self.policy.total);
        let past_end = |instant: Instant| end.is_some_and(|end| instant > end);
        let give_up = |attempts, last| Exhausted {
            endpoint: request.endpoint(),
            attempts,
            last,
        };
        if let Some(deadline) = self.deadline() {
            if past_end(deadline) {
                let wait = deadline - Instant::now();
                return Err(give_up(0, Failure::RateLimited { wait: Some(wait) }));
            }
            tokio::time::sleep_until(deadline).await;
        }
        let mut attempts = 0;
        let mut ambiguous = false;
        let mut unnamed_limits = 0;
        loop {
            attempts += 1;
            let attempt = match end {
                // Whatever is left of the total bounds the attempt; one cut off may have landed.
                Some(end) => tokio::time::timeout_at(end, self.transport.send(request))
                    .await
                    .unwrap_or(Err(Failure::Timeout)),
                None => self.transport.send(request).await,
            };
            let (failure, floor) = match attempt {
                Ok(reply) if reply.rate_limited => {
                    let named = reply.retry_after.or(reply.reset_wait);
                    let floor = match named {
                        Some(wait) => {
                            self.pause_for(wait);
                            wait
                        }
                        None => {
                            unnamed_limits += 1;
                            1u32.checked_shl(unnamed_limits - 1)
                                .and_then(|factor| SECONDARY_LIMIT_WAIT.checked_mul(factor))
                                .unwrap_or(Duration::MAX)
                        }
                    };
                    (Failure::RateLimited { wait: named }, floor)
                }
                Ok(reply) if reply.status.is_server_error() => {
                    (Failure::Status(reply.status.as_u16()), Duration::ZERO)
                }
                Ok(mut reply) => {
                    reply.after_ambiguous = ambiguous;
                    return Ok(reply);
                }
                // Nothing was sent, and another attempt would fail the same way.
                Err(Failure::Request) => return Err(give_up(attempts, Failure::Request)),
                Err(failure) => (failure, Duration::ZERO),
            };
            let definite = matches!(failure, Failure::Connect | Failure::RateLimited { .. });
            tracing::debug!(
                endpoint = %request.endpoint(),
                attempts,
                %failure,
                "a GitHub request failed transiently"
            );
            if attempts >= self.policy.attempts || (!definite && matches!(class, Class::Once)) {
                return Err(give_up(attempts, failure));
            }
            let wait = self.policy.jittered(attempts, (self.jitter)()).max(floor);
            let Some(mut until) = Instant::now().checked_add(wait) else {
                return Err(give_up(attempts, failure));
            };
            if let Some(deadline) = self.deadline() {
                until = until.max(deadline);
            }
            if past_end(until) {
                return Err(give_up(attempts, failure));
            }
            tokio::time::sleep_until(until).await;
            if !definite {
                ambiguous = true;
                if let Class::Create(probe, _) = &class {
                    match probe().await {
                        Landing::Landed(mut reply) => {
                            reply.after_ambiguous = true;
                            return Ok(reply);
                        }
                        Landing::NotLanded => {}
                        Landing::Unknown => return Err(give_up(attempts, failure)),
                    }
                }
            }
            if let Class::Create(_, gate) | Class::Gated(gate) = &class {
                // A gate refused at the end of the total takes nothing, so cutting it off there
                // spends no grant on an attempt that is never sent.
                match end {
                    Some(end) => {
                        if tokio::time::timeout_at(end, gate()).await.is_err() {
                            return Err(give_up(attempts, failure));
                        }
                    }
                    None => gate().await,
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Mutex, PoisonError};
    use std::time::Duration;

    use reqwest::{Method, StatusCode};
    use tokio::time::Instant;

    use mintswarm_domain::retry::{Exhausted, Failure, RetryPolicy};

    use super::{
        Attempt, Gate, Landing, Request, Retrying, SECONDARY_LIMIT_WAIT, Transport, random_unit,
        sealed,
    };
    use crate::rest::Reply;
    use crate::secret::Token;

    /// A transport answering from a script, one entry per attempt, after `took` each.
    #[derive(Debug, Default)]
    struct Script {
        answers: Mutex<VecDeque<Attempt>>,
        sent: Mutex<Vec<Instant>>,
        took: Duration,
    }

    impl sealed::Sealed for Script {}

    impl Transport for Script {
        async fn send(&self, _: &Request<'_>) -> Attempt {
            self.sent
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(Instant::now());
            tokio::time::sleep(self.took).await;
            self.answers
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .pop_front()
                .expect("the script ran out")
        }
    }

    fn reply(status: u16) -> Reply {
        Reply {
            endpoint: String::from("GET /x"),
            status: StatusCode::from_u16(status).expect("a status"),
            etag: None,
            rate_limited: status == 429,
            retry_after: None,
            reset_wait: None,
            next_page: None,
            date: None,
            body: Vec::new(),
            after_ambiguous: false,
        }
    }

    fn limited(retry_after: Option<u64>, reset_wait: Option<u64>) -> Reply {
        Reply {
            retry_after: retry_after.map(Duration::from_secs),
            reset_wait: reset_wait.map(Duration::from_secs),
            ..reply(429)
        }
    }

    fn scripted(answers: impl IntoIterator<Item = Attempt>) -> Retrying<Script> {
        layer_with(answers, RetryPolicy::default(), Duration::ZERO)
    }

    /// A layer over a script, with the jitter fixed at its ceiling.
    fn layer_with(
        answers: impl IntoIterator<Item = Attempt>,
        policy: RetryPolicy,
        took: Duration,
    ) -> Retrying<Script> {
        let script = Script {
            answers: Mutex::new(answers.into_iter().collect()),
            took,
            ..Script::default()
        };
        Retrying::wrap(script, policy, false).with_jitter(|| 1.0)
    }

    fn token() -> Token {
        Token::new(String::from("ghs_t0ken"), 0)
    }

    /// A gate that lets every attempt through at once.
    fn no_gate<'a>() -> Gate<'a> {
        Box::new(|| Box::pin(async {}))
    }

    /// The time from the first attempt to each later one.
    fn offsets(layer: &Retrying<Script>) -> Vec<Duration> {
        let sent = layer
            .transport
            .sent
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        sent.iter().map(|at| *at - sent[0]).collect()
    }

    fn secs(seconds: &[u64]) -> Vec<Duration> {
        seconds.iter().copied().map(Duration::from_secs).collect()
    }

    #[tokio::test(start_paused = true)]
    async fn server_errors_are_retried_with_backoff() {
        let layer = scripted([Ok(reply(500)), Ok(reply(500)), Ok(reply(200))]);
        let got = layer.get("/x", &token(), None).await.expect("a reply");
        assert_eq!(got.status, StatusCode::OK);
        assert!(got.after_ambiguous);
        assert_eq!(offsets(&layer), secs(&[0, 1, 3]));
    }

    #[tokio::test(start_paused = true)]
    async fn bad_gateway_reset_and_timeout_are_retried() {
        let layer = scripted([
            Ok(reply(502)),
            Err(Failure::Reset),
            Err(Failure::Timeout),
            Ok(reply(200)),
        ]);
        let got = layer.get("/x", &token(), None).await.expect("a reply");
        assert_eq!(got.status, StatusCode::OK);
        assert_eq!(offsets(&layer), secs(&[0, 1, 3, 7]));
    }

    #[tokio::test(start_paused = true)]
    async fn attempts_run_out_with_the_count() {
        let layer = scripted([
            Ok(reply(500)),
            Ok(reply(500)),
            Ok(reply(500)),
            Ok(reply(500)),
        ]);
        let error = layer.get("/x", &token(), None).await.unwrap_err();
        assert!(
            matches!(
                error,
                Exhausted {
                    attempts: 4,
                    last: Failure::Status(500),
                    ..
                }
            ),
            "{error:?}"
        );
        assert_eq!(offsets(&layer).len(), 4);
    }

    #[tokio::test(start_paused = true)]
    async fn retry_after_is_honoured() {
        let layer = scripted([Ok(limited(Some(5), None)), Ok(reply(200))]);
        layer.get("/x", &token(), None).await.expect("a reply");
        assert_eq!(offsets(&layer), secs(&[0, 5]));
    }

    #[tokio::test(start_paused = true)]
    async fn ratelimit_reset_is_honoured() {
        let layer = scripted([Ok(limited(None, Some(7))), Ok(reply(200))]);
        layer.get("/x", &token(), None).await.expect("a reply");
        assert_eq!(offsets(&layer), secs(&[0, 7]));
    }

    #[tokio::test(start_paused = true)]
    async fn a_rate_limit_without_headers_waits_a_minute() {
        let layer = scripted([Ok(limited(None, None)), Ok(reply(200))]);
        layer.get("/x", &token(), None).await.expect("a reply");
        assert_eq!(offsets(&layer), vec![Duration::ZERO, SECONDARY_LIMIT_WAIT]);

        // The second header-less refusal asks for 120 s, past the default total.
        let layer = scripted([Ok(limited(None, None)), Ok(limited(None, None))]);
        let started = Instant::now();
        let error = layer.get("/x", &token(), None).await.unwrap_err();
        assert!(
            matches!(
                error,
                Exhausted {
                    attempts: 2,
                    last: Failure::RateLimited { wait: None },
                    ..
                }
            ),
            "{error:?}"
        );
        assert_eq!(started.elapsed(), SECONDARY_LIMIT_WAIT);
    }

    #[tokio::test(start_paused = true)]
    async fn a_rate_limit_beyond_the_budget_gives_up_at_once() {
        let layer = scripted([Ok(limited(Some(200), None))]);
        let started = Instant::now();
        let error = layer.get("/x", &token(), None).await.unwrap_err();
        assert!(
            matches!(
                error,
                Exhausted {
                    attempts: 1,
                    last: Failure::RateLimited { wait: Some(wait) },
                    ..
                } if wait == Duration::from_secs(200)
            ),
            "{error:?}"
        );
        assert_eq!(started.elapsed(), Duration::ZERO);
    }

    #[tokio::test(start_paused = true)]
    async fn the_total_budget_bounds_retries() {
        // Each attempt takes 4 s: attempts at 0 and 5 s, and the next would start at 11 s.
        let policy = RetryPolicy {
            total: Duration::from_secs(10),
            ..RetryPolicy::default()
        };
        let layer = layer_with(
            [Err(Failure::Timeout), Err(Failure::Timeout)],
            policy,
            Duration::from_secs(4),
        );
        let started = Instant::now();
        let error = layer.get("/x", &token(), None).await.unwrap_err();
        assert!(
            matches!(
                error,
                Exhausted {
                    attempts: 2,
                    last: Failure::Timeout,
                    ..
                }
            ),
            "{error:?}"
        );
        assert_eq!(offsets(&layer), secs(&[0, 5]));
        assert_eq!(started.elapsed(), Duration::from_secs(9));
    }

    #[tokio::test(start_paused = true)]
    async fn the_total_cuts_off_a_running_attempt() {
        // The attempt would take 30 s; the total ends it at 10 s, and no wait fits after it.
        let policy = RetryPolicy {
            total: Duration::from_secs(10),
            ..RetryPolicy::default()
        };
        let layer = layer_with([Ok(reply(200))], policy, Duration::from_secs(30));
        let started = Instant::now();
        let error = layer.get("/x", &token(), None).await.unwrap_err();
        assert!(
            matches!(
                error,
                Exhausted {
                    attempts: 1,
                    last: Failure::Timeout,
                    ..
                }
            ),
            "{error:?}"
        );
        assert_eq!(started.elapsed(), Duration::from_secs(10));
    }

    #[tokio::test(start_paused = true)]
    async fn a_request_that_cannot_be_built_is_not_retried() {
        let layer = scripted([Err(Failure::Request)]);
        let started = Instant::now();
        let error = layer.get("/x", &token(), None).await.unwrap_err();
        assert!(
            matches!(
                error,
                Exhausted {
                    attempts: 1,
                    last: Failure::Request,
                    ..
                }
            ),
            "{error:?}"
        );
        assert_eq!(started.elapsed(), Duration::ZERO);

        // A create gives up the same way, without asking its probe.
        let layer = scripted([Ok(reply(500)), Err(Failure::Request)]);
        let asked = AtomicUsize::new(0);
        let error = layer
            .create(
                "/x",
                &token(),
                &serde_json::json!({}),
                Box::new(|| {
                    asked.fetch_add(1, Ordering::Relaxed);
                    Box::pin(async { Landing::NotLanded })
                }),
                no_gate(),
            )
            .await
            .unwrap_err();
        assert!(
            matches!(
                error,
                Exhausted {
                    attempts: 2,
                    last: Failure::Request,
                    ..
                }
            ),
            "{error:?}"
        );
        // Asked once, after the 500, and not after the request that could not be built.
        assert_eq!(asked.load(Ordering::Relaxed), 1);
        assert_eq!(offsets(&layer), secs(&[0, 1]));
    }

    #[tokio::test(start_paused = true)]
    async fn a_pause_shared_with_later_calls_is_never_added_to() {
        let script = Script {
            answers: Mutex::new(
                [
                    Ok(limited(Some(5), None)),
                    Ok(reply(500)),
                    Ok(reply(200)),
                    Ok(reply(200)),
                ]
                .into_iter()
                .collect(),
            ),
            ..Script::default()
        };
        let layer = Retrying::wrap(script, RetryPolicy::default(), true).with_jitter(|| 1.0);
        let started = Instant::now();
        layer.get("/x", &token(), None).await.expect("a reply");
        assert_eq!(layer.pause_remaining(), Duration::ZERO);
        layer.get("/x", &token(), None).await.expect("a reply");
        // The 500's own 2 s wait runs from its attempt, not from the end of the pause.
        assert_eq!(offsets(&layer), secs(&[0, 5, 7, 7]));
        assert_eq!(started.elapsed(), Duration::from_secs(7));
    }

    #[tokio::test(start_paused = true)]
    async fn a_running_pause_that_does_not_fit_ends_the_call_unsent() {
        let script = Script {
            answers: Mutex::new([Ok(limited(Some(500), None))].into_iter().collect()),
            ..Script::default()
        };
        let layer = Retrying::wrap(script, RetryPolicy::default(), true);
        let error = layer.get("/x", &token(), None).await.unwrap_err();
        assert!(
            matches!(error.last, Failure::RateLimited { .. }),
            "{error:?}"
        );
        assert_eq!(layer.pause_remaining(), Duration::from_secs(500));
        let error = layer.get("/x", &token(), None).await.unwrap_err();
        assert!(
            matches!(
                error,
                Exhausted {
                    attempts: 0,
                    last: Failure::RateLimited { wait: Some(_) },
                    ..
                }
            ),
            "{error:?}"
        );
        assert_eq!(offsets(&layer).len(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn client_errors_are_not_retried() {
        for status in [400, 403, 404, 422] {
            let layer = scripted([Ok(reply(status))]);
            let got = layer.get("/x", &token(), None).await.expect("a reply");
            assert_eq!(got.status.as_u16(), status);
            assert!(!got.after_ambiguous);
            assert_eq!(offsets(&layer).len(), 1, "{status}");
        }
    }

    #[tokio::test(start_paused = true)]
    async fn put_and_delete_are_retried() {
        let body = serde_json::json!({"sha": "abc"});
        // A PUT is sent again only after an attempt that was not carried out.
        for definite in [Err(Failure::Connect), Ok(limited(Some(1), None))] {
            let layer = scripted([definite, Ok(reply(200))]);
            let got = layer.put("/x", &token(), &body).await.expect("a reply");
            assert_eq!(got.status, StatusCode::OK);
            assert!(!got.after_ambiguous);
            assert_eq!(offsets(&layer).len(), 2);
        }
        // After one that may have been, the call ends with it.
        for ambiguous in [
            Ok(reply(502)),
            Err(Failure::Timeout),
            Err(Failure::Reset),
            Err(Failure::Body),
        ] {
            let last = match &ambiguous {
                Ok(_) => Failure::Status(502),
                Err(failure) => *failure,
            };
            let layer = scripted([ambiguous, Ok(reply(200))]);
            let error = layer.put("/x", &token(), &body).await.unwrap_err();
            assert_eq!(error.attempts, 1);
            assert_eq!(error.last, last);
            assert_eq!(offsets(&layer).len(), 1);
        }

        // The first DELETE may have removed it, so the 404 says so.
        let layer = scripted([Err(Failure::Reset), Ok(reply(404))]);
        let got = layer.delete("/x", &token()).await.expect("a reply");
        assert_eq!(got.status, StatusCode::NOT_FOUND);
        assert!(got.after_ambiguous);
        assert_eq!(offsets(&layer).len(), 2);

        // After a refused connection nothing was removed, so the 404 is GitHub's own.
        let layer = scripted([Err(Failure::Connect), Ok(reply(404))]);
        let got = layer.delete("/x", &token()).await.expect("a reply");
        assert!(!got.after_ambiguous);

        let layer = scripted([Ok(reply(503)), Ok(reply(201))]);
        let got = layer
            .post_idempotent("/x", &token(), &body)
            .await
            .expect("a reply");
        assert_eq!(got.status, StatusCode::CREATED);
        assert_eq!(offsets(&layer).len(), 2);
    }

    #[tokio::test(start_paused = true)]
    async fn a_create_asks_its_probe_after_an_ambiguous_failure() {
        let body = serde_json::json!({"body": "hello"});
        for ambiguous in [
            Ok(reply(500)),
            Err(Failure::Timeout),
            Err(Failure::Reset),
            Err(Failure::Body),
        ] {
            // Landed: the probe's reply is the call's, and nothing is sent again.
            let layer = scripted([ambiguous.clone(), Ok(reply(201))]);
            let asked = AtomicUsize::new(0);
            let got = layer
                .create(
                    "/x",
                    &token(),
                    &body,
                    Box::new(|| {
                        asked.fetch_add(1, Ordering::Relaxed);
                        Box::pin(async { Landing::Landed(reply(200)) })
                    }),
                    no_gate(),
                )
                .await
                .expect("a reply");
            assert_eq!(got.status, StatusCode::OK);
            assert_eq!(asked.load(Ordering::Relaxed), 1);
            assert_eq!(offsets(&layer).len(), 1);

            // Not landed: it is sent again.
            let layer = scripted([ambiguous.clone(), Ok(reply(201))]);
            let asked = AtomicUsize::new(0);
            let got = layer
                .create(
                    "/x",
                    &token(),
                    &body,
                    Box::new(|| {
                        asked.fetch_add(1, Ordering::Relaxed);
                        Box::pin(async { Landing::NotLanded })
                    }),
                    no_gate(),
                )
                .await
                .expect("a reply");
            assert_eq!(got.status, StatusCode::CREATED);
            assert_eq!(asked.load(Ordering::Relaxed), 1);
            assert_eq!(offsets(&layer).len(), 2);

            // Unknown: the call gives up without sending again.
            let layer = scripted([ambiguous.clone(), Ok(reply(201))]);
            let error = layer
                .create(
                    "/x",
                    &token(),
                    &body,
                    Box::new(|| Box::pin(async { Landing::Unknown })),
                    no_gate(),
                )
                .await
                .unwrap_err();
            assert_eq!(error.attempts, 1);
            let expected = match &ambiguous {
                Ok(_) => Failure::Status(500),
                Err(failure) => *failure,
            };
            assert_eq!(error.last, expected);
            assert_eq!(offsets(&layer).len(), 1);
        }
    }

    #[tokio::test(start_paused = true)]
    async fn a_create_is_retried_without_a_probe_after_a_rate_limit_or_refused_connection() {
        let body = serde_json::json!({"body": "hello"});
        for definite in [Ok(limited(Some(2), None)), Err(Failure::Connect)] {
            let layer = scripted([definite, Ok(reply(201))]);
            let asked = AtomicUsize::new(0);
            let got = layer
                .create(
                    "/x",
                    &token(),
                    &body,
                    Box::new(|| {
                        asked.fetch_add(1, Ordering::Relaxed);
                        Box::pin(async { Landing::Unknown })
                    }),
                    no_gate(),
                )
                .await
                .expect("a reply");
            assert_eq!(got.status, StatusCode::CREATED);
            assert!(!got.after_ambiguous);
            assert_eq!(asked.load(Ordering::Relaxed), 0);
            assert_eq!(offsets(&layer).len(), 2);
        }
    }

    #[tokio::test(start_paused = true)]
    async fn a_create_waits_on_its_gate_before_every_later_attempt() {
        let body = serde_json::json!({"body": "hello"});
        // An ambiguous attempt the probe finds did not land, then a refused connection.
        let layer = scripted([Err(Failure::Reset), Err(Failure::Connect), Ok(reply(201))]);
        let gated = AtomicUsize::new(0);
        let got = layer
            .create(
                "/x",
                &token(),
                &body,
                Box::new(|| Box::pin(async { Landing::NotLanded })),
                Box::new(|| {
                    gated.fetch_add(1, Ordering::Relaxed);
                    Box::pin(tokio::time::sleep(Duration::from_secs(5)))
                }),
            )
            .await
            .expect("a reply");
        assert_eq!(got.status, StatusCode::CREATED);
        assert_eq!(gated.load(Ordering::Relaxed), 2);
        let offsets = offsets(&layer);
        assert_eq!(offsets.len(), 3);
        // Each later attempt waited on the gate as well as on the policy.
        assert!(offsets[1] >= Duration::from_secs(5));
        assert!(offsets[2] - offsets[1] >= Duration::from_secs(5));

        // A first attempt that settles the call never waits on the gate.
        let layer = scripted([Ok(reply(201))]);
        let gated = AtomicUsize::new(0);
        layer
            .create(
                "/x",
                &token(),
                &body,
                Box::new(|| Box::pin(async { Landing::Unknown })),
                Box::new(|| {
                    gated.fetch_add(1, Ordering::Relaxed);
                    Box::pin(async {})
                }),
            )
            .await
            .expect("a reply");
        assert_eq!(gated.load(Ordering::Relaxed), 0);
    }

    #[tokio::test(start_paused = true)]
    async fn a_gate_that_ends_past_the_total_ends_the_create() {
        let layer = layer_with(
            [Err(Failure::Connect), Ok(reply(201))],
            RetryPolicy {
                total: Duration::from_secs(60),
                ..RetryPolicy::default()
            },
            Duration::ZERO,
        );
        let start = Instant::now();
        let error = layer
            .create(
                "/x",
                &token(),
                &serde_json::json!({}),
                Box::new(|| Box::pin(async { Landing::Unknown })),
                Box::new(|| Box::pin(tokio::time::sleep(Duration::from_secs(120)))),
            )
            .await
            .unwrap_err();
        assert_eq!(start.elapsed(), Duration::from_secs(60));
        assert_eq!(error.attempts, 1);
        assert_eq!(error.last, Failure::Connect);
        assert_eq!(offsets(&layer).len(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn patch_and_gated_posts_are_retried_behind_their_gate() {
        let body = serde_json::json!({"body": "edited"});
        for method in [Method::PATCH, Method::POST, Method::DELETE] {
            let layer = scripted([Ok(reply(502)), Ok(reply(200))]);
            let gated = AtomicUsize::new(0);
            let gate: Gate<'_> = Box::new(|| {
                gated.fetch_add(1, Ordering::Relaxed);
                Box::pin(async {})
            });
            let got = if method == Method::POST {
                layer
                    .post_idempotent_gated("/x", &token(), &body, gate)
                    .await
            } else if method == Method::DELETE {
                layer.delete_gated("/x", &token(), &body, gate).await
            } else {
                layer.patch("/x", &token(), &body, gate).await
            }
            .expect("a reply");
            assert_eq!(got.status, StatusCode::OK);
            assert!(got.after_ambiguous);
            assert_eq!(gated.load(Ordering::Relaxed), 1, "method: {method}");
            assert_eq!(offsets(&layer).len(), 2);
        }

        // A gate still waiting when the total ends ends the call, and nothing more is sent.
        let layer = layer_with(
            [Ok(reply(502)), Ok(reply(200))],
            RetryPolicy {
                total: Duration::from_secs(60),
                ..RetryPolicy::default()
            },
            Duration::ZERO,
        );
        let start = Instant::now();
        let error = layer
            .patch(
                "/x",
                &token(),
                &body,
                Box::new(|| Box::pin(tokio::time::sleep(Duration::from_secs(120)))),
            )
            .await
            .unwrap_err();
        assert_eq!(start.elapsed(), Duration::from_secs(60));
        assert_eq!(error.attempts, 1);
        assert_eq!(error.last, Failure::Status(502));
        assert_eq!(offsets(&layer).len(), 1);
    }

    #[test]
    fn full_jitter_stays_under_the_ceiling() {
        let policy = RetryPolicy::default();
        let draws: Vec<f64> = (0..1000).map(|_| random_unit()).collect();
        assert!(draws.iter().all(|draw| (0.0..1.0).contains(draw)));
        // The draws spread over the range rather than repeating one value.
        assert!(draws.iter().any(|draw| *draw < 0.25));
        assert!(draws.iter().any(|draw| *draw > 0.75));
        for (n, draw) in (1..=8).cycle().zip(draws) {
            assert!(policy.jittered(n, draw) <= policy.ceiling(n));
        }
    }
}
