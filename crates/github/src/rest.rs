//! A minimal authenticated REST helper for the field client, the field catalogue and the reads:
//! one request with a caller's token, answered with its status, ETag, rate-limit state,
//! `retry-after` and raw body.
//!
//! A pausing helper ([`Rest::pausing`]) also honours `retry-after`: after a rate-limited reply
//! that carries it, every later request waits until that long has passed.

use reqwest::header::{
    ACCEPT, AUTHORIZATION, CONTENT_TYPE, ETAG, HeaderMap, HeaderValue, IF_NONE_MATCH, RETRY_AFTER,
    USER_AGENT,
};
use reqwest::{Method, StatusCode};
use std::sync::{Mutex, PoisonError};
use std::time::Duration;
use tokio::time::Instant;

use mintswarm_domain::field::FieldError;

use crate::secret::Token;
use crate::token::{CONNECT_TIMEOUT, REQUEST_TIMEOUT};

/// Why a GitHub field client could not be built. No variant carries a credential.
#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    /// An owner or repository name is not valid.
    #[error("invalid {what}: '{value}'")]
    InvalidName {
        /// What the name is.
        what: &'static str,
        /// The name.
        value: String,
    },
    /// The HTTP client could not be built.
    #[error("cannot build the HTTP client")]
    Client,
}

/// Checks that `owner` and `repo` are names GitHub accepts, so they are safe in a path.
pub(crate) fn check_names(owner: &str, repo: &str) -> Result<(), ClientError> {
    for (what, value) in [("owner", owner), ("repository", repo)] {
        if !crate::config::valid_name(value) {
            return Err(ClientError::InvalidName {
                what,
                value: value.to_owned(),
            });
        }
    }
    Ok(())
}

/// GitHub's answer to one request.
#[derive(Debug)]
pub(crate) struct Reply {
    /// The method and path, for error messages; never the token.
    pub endpoint: String,
    pub status: StatusCode,
    pub etag: Option<String>,
    /// A 429, or a 403 with `x-ratelimit-remaining: 0` or a `retry-after` header.
    pub rate_limited: bool,
    /// The `retry-after` header in delta-seconds; an HTTP-date is ignored.
    pub retry_after: Option<Duration>,
    pub body: Vec<u8>,
}

impl Reply {
    /// The body as JSON, or a transport error naming the endpoint.
    pub fn json(&self) -> Result<serde_json::Value, FieldError> {
        serde_json::from_slice(&self.body)
            .map_err(|_| FieldError::Transport(format!("{}: not the expected JSON", self.endpoint)))
    }
}

/// An HTTP client for GitHub's REST API at one base URL.
#[derive(Debug)]
pub(crate) struct Rest {
    http: reqwest::Client,
    base: String,
    /// For a pausing client, the instant before which no request is sent; `None` inside when
    /// no pause was asked for yet.
    pause: Option<Mutex<Option<Instant>>>,
}

impl Rest {
    /// A client for the API at `api_base`, with the token client's timeouts. It does not pause
    /// on `retry-after`.
    pub fn new(api_base: &str) -> Result<Self, ClientError> {
        Self::build(api_base, None)
    }

    /// A client like [`Rest::new`] that, after a rate-limited reply carrying `retry-after`,
    /// sends no request until that long has passed.
    pub fn pausing(api_base: &str) -> Result<Self, ClientError> {
        Self::build(api_base, Some(Mutex::new(None)))
    }

    fn build(api_base: &str, pause: Option<Mutex<Option<Instant>>>) -> Result<Self, ClientError> {
        let http = reqwest::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(REQUEST_TIMEOUT)
            .build()
            .map_err(|_| ClientError::Client)?;
        Ok(Self {
            http,
            base: api_base.trim_end_matches('/').to_owned(),
            pause,
        })
    }

    /// The instant before which a pausing client sends nothing, if it is still ahead.
    fn deadline(&self) -> Option<Instant> {
        let pause = self.pause.as_ref()?;
        let deadline = *pause.lock().unwrap_or_else(PoisonError::into_inner);
        deadline.filter(|deadline| *deadline > Instant::now())
    }

    /// What is left of a pausing client's pause; zero when there is none.
    pub fn pause_remaining(&self) -> Duration {
        self.deadline()
            .map_or(Duration::ZERO, |deadline| deadline - Instant::now())
    }

    /// Moves a pausing client's deadline to `wait` from now, never earlier than it is.
    fn pause_for(&self, wait: Duration) {
        let Some(pause) = &self.pause else {
            return;
        };
        let until = Instant::now() + wait;
        let mut deadline = pause.lock().unwrap_or_else(PoisonError::into_inner);
        if deadline.is_none_or(|deadline| deadline < until) {
            *deadline = Some(until);
        }
    }

    /// Sends one request to `path` with `token`, a JSON `body` if any, and `If-None-Match` if
    /// an ETag is given. Only a failure to send or to read the body is an error here; statuses
    /// are the caller's to classify.
    pub async fn send(
        &self,
        method: Method,
        path: &str,
        token: &Token,
        body: Option<serde_json::Value>,
        if_none_match: Option<&str>,
    ) -> Result<Reply, FieldError> {
        if let Some(deadline) = self.deadline() {
            tokio::time::sleep_until(deadline).await;
        }
        let endpoint = format!("{method} {path}");
        let failed = || FieldError::Transport(format!("{endpoint}: the request failed"));
        let mut authorization =
            HeaderValue::from_str(&format!("Bearer {}", token.expose())).map_err(|_| failed())?;
        authorization.set_sensitive(true);
        let mut headers = HeaderMap::new();
        headers.insert(
            ACCEPT,
            HeaderValue::from_static("application/vnd.github+json"),
        );
        headers.insert(
            "x-github-api-version",
            HeaderValue::from_static("2022-11-28"),
        );
        headers.insert(USER_AGENT, HeaderValue::from_static("mintswarm"));
        headers.insert(AUTHORIZATION, authorization);
        if let Some(etag) = if_none_match {
            headers.insert(
                IF_NONE_MATCH,
                HeaderValue::from_str(etag).map_err(|_| failed())?,
            );
        }
        let mut request = self
            .http
            .request(method, format!("{}{path}", self.base))
            .headers(headers);
        if let Some(body) = body {
            request = request
                .header(CONTENT_TYPE, "application/json")
                .body(body.to_string());
        }
        let response = request.send().await.map_err(|_| failed())?;
        let status = response.status();
        let header = |name| {
            response
                .headers()
                .get(name)
                .and_then(|value: &HeaderValue| value.to_str().ok())
                .map(str::to_owned)
        };
        let etag = header(ETAG);
        let rate_limited = status == StatusCode::TOO_MANY_REQUESTS
            || (status == StatusCode::FORBIDDEN
                && (header(reqwest::header::HeaderName::from_static(
                    "x-ratelimit-remaining",
                ))
                .is_some_and(|remaining| remaining.trim() == "0")
                    || header(RETRY_AFTER).is_some()));
        let retry_after = header(RETRY_AFTER)
            .and_then(|value| value.trim().parse::<u64>().ok())
            .map(Duration::from_secs);
        let body = response.bytes().await.map_err(|_| failed())?.to_vec();
        if let Some(wait) = retry_after.filter(|_| rate_limited) {
            self.pause_for(wait);
        }
        Ok(Reply {
            endpoint,
            status,
            etag,
            rate_limited,
            retry_after,
            body,
        })
    }
}
