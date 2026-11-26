//! GitHub's REST API for every client in this crate: [`Http`] sends one request with a
//! caller's token and answers with its status, ETag, rate-limit state, `retry-after`,
//! `x-ratelimit-reset` wait and raw body, and [`Rest`] is the retry layer over it, the only way
//! to reach it.
//!
//! A pausing layer ([`Rest::pausing`]) also makes every later call wait out a rate limit's
//! named wait.

use reqwest::StatusCode;
use reqwest::header::{
    ACCEPT, AUTHORIZATION, CONTENT_TYPE, DATE, ETAG, HeaderMap, HeaderName, HeaderValue,
    IF_NONE_MATCH, LINK, RETRY_AFTER, USER_AGENT,
};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use mintswarm_domain::field::FieldError;
use mintswarm_domain::retry::{Failure, RetryPolicy};

use crate::retry::{Attempt, Request, Retrying, Transport};

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

/// `text` percent-encoded as one path segment: every byte but RFC 3986's unreserved ones.
pub(crate) fn path_segment(text: &str) -> String {
    let mut encoded = String::with_capacity(text.len());
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            encoded.push(char::from(byte));
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

/// Whether a `Link` header value (`<url>; rel="next", <url>; rel="last"`) has a `next` link.
fn links_next(link: &str) -> bool {
    link.split(',').any(|entry| {
        entry.split(';').skip(1).any(|param| {
            param.trim().strip_prefix("rel=").is_some_and(|rel| {
                rel.trim_matches('"')
                    .split_whitespace()
                    .any(|rel| rel == "next")
            })
        })
    })
}

/// GitHub's answer to one request.
#[derive(Clone, Debug)]
pub(crate) struct Reply {
    /// The method and path, for error messages; never the token.
    pub endpoint: String,
    pub status: StatusCode,
    pub etag: Option<String>,
    /// A 429, or a 403 with `x-ratelimit-remaining: 0`, a `retry-after` header or a message
    /// naming a rate limit.
    pub rate_limited: bool,
    /// The `retry-after` header in delta-seconds; an HTTP-date is ignored.
    pub retry_after: Option<Duration>,
    /// With `x-ratelimit-remaining: 0`, the time until `x-ratelimit-reset`.
    pub reset_wait: Option<Duration>,
    /// Whether the `Link` header names a `rel="next"` page; `None` when there is no `Link`.
    pub next_page: Option<bool>,
    /// The raw `Date` header: GitHub's clock at the reply, in IMF-fixdate form.
    pub date: Option<String>,
    pub body: Vec<u8>,
    /// Whether an earlier attempt of the same call may have been carried out, so a DELETE's 404
    /// may be that attempt's doing. Set by the retry layer.
    pub after_ambiguous: bool,
}

impl Reply {
    /// The body as JSON, or a transport error naming the endpoint.
    pub fn json(&self) -> Result<serde_json::Value, FieldError> {
        serde_json::from_slice(&self.body)
            .map_err(|_| FieldError::Transport(format!("{}: not the expected JSON", self.endpoint)))
    }
}

/// GitHub's REST API at one base URL, one attempt per request: the only HTTP client in this
/// crate. Only this module can build one, and only [`Rest`] holds one.
#[derive(Debug)]
pub(crate) struct Http {
    http: reqwest::Client,
    base: String,
}

impl Http {
    /// A client for the API at `api_base`, with the token client's timeouts.
    fn new(api_base: &str) -> Result<Self, ClientError> {
        let http = reqwest::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(REQUEST_TIMEOUT)
            .build()
            .map_err(|_| ClientError::Client)?;
        Ok(Self {
            http,
            base: api_base.trim_end_matches('/').to_owned(),
        })
    }
}

/// The class of a failed send: a request reqwest could not build was never sent and would fail
/// again, nothing was sent without a connection, a timeout is its own class, and anything else
/// broke a request that may have been sent. reqwest's own error, and its text, go no further.
fn send_failure(error: &reqwest::Error) -> Failure {
    if error.is_builder() {
        Failure::Request
    } else if error.is_connect() {
        Failure::Connect
    } else if error.is_timeout() {
        Failure::Timeout
    } else {
        Failure::Reset
    }
}

impl Transport for Http {
    /// Sends one request with the caller's token, a JSON body if any, and `If-None-Match` if an
    /// ETag is given. Only a failure to send or to read the body is a [`Failure`] here; statuses
    /// are the layer's and the caller's to classify. A header or URL that cannot be built means
    /// nothing was sent and nothing would be on another attempt, a [`Failure::Request`].
    async fn send(&self, request: &Request<'_>) -> Attempt {
        let endpoint = request.endpoint();
        let mut authorization =
            HeaderValue::from_str(&format!("Bearer {}", request.token.expose()))
                .map_err(|_| Failure::Request)?;
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
        if let Some(etag) = request.if_none_match {
            headers.insert(
                IF_NONE_MATCH,
                HeaderValue::from_str(etag).map_err(|_| Failure::Request)?,
            );
        }
        let mut builder = self
            .http
            .request(
                request.method.clone(),
                format!("{}{}", self.base, request.path),
            )
            .headers(headers);
        if let Some(body) = request.body {
            builder = builder
                .header(CONTENT_TYPE, "application/json")
                .body(body.to_string());
        }
        let response = builder.send().await.map_err(|error| send_failure(&error))?;
        let status = response.status();
        let header = |name| {
            response
                .headers()
                .get(name)
                .and_then(|value: &HeaderValue| value.to_str().ok())
                .map(str::to_owned)
        };
        let etag = header(ETAG);
        let exhausted = header(HeaderName::from_static("x-ratelimit-remaining"))
            .is_some_and(|remaining| remaining.trim() == "0");
        let retry_after = header(RETRY_AFTER)
            .and_then(|value| value.trim().parse::<u64>().ok())
            .map(Duration::from_secs);
        let reset_wait = header(HeaderName::from_static("x-ratelimit-reset"))
            .filter(|_| exhausted)
            .and_then(|value| value.trim().parse::<u64>().ok())
            .map(|reset| {
                let now = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map_or(0, |elapsed| elapsed.as_secs());
                Duration::from_secs(reset.saturating_sub(now))
            });
        let next_page = header(LINK).map(|link| links_next(&link));
        let date = header(DATE);
        let has_retry_after = header(RETRY_AFTER).is_some();
        let body = response.bytes().await.map_err(|_| Failure::Body)?.to_vec();
        let rate_limited = status == StatusCode::TOO_MANY_REQUESTS
            || (status == StatusCode::FORBIDDEN
                && (exhausted || has_retry_after || error_text(&body).contains("rate limit")));
        Ok(Reply {
            endpoint,
            status,
            etag,
            rate_limited,
            retry_after,
            reset_wait,
            next_page,
            date,
            body,
            after_ambiguous: false,
        })
    }
}

/// The retry layer over GitHub's REST API: what every client in this crate holds.
pub(crate) type Rest = Retrying<Http>;

impl Rest {
    /// A layer over the API at `api_base` with `policy`. It does not make later calls wait out
    /// a rate limit.
    pub fn new(api_base: &str, policy: RetryPolicy) -> Result<Self, ClientError> {
        Ok(Self::wrap(Http::new(api_base)?, policy, false))
    }

    /// A layer like [`Rest::new`] that, after a rate-limited reply naming a wait, sends no
    /// request of any call until that long has passed.
    pub fn pausing(api_base: &str, policy: RetryPolicy) -> Result<Self, ClientError> {
        Ok(Self::wrap(Http::new(api_base)?, policy, true))
    }
}

/// The lower-cased `message`, `errors[].message` and `errors[].field` of an error body.
pub(crate) fn error_text(body: &[u8]) -> String {
    let Ok(body) = serde_json::from_slice::<serde_json::Value>(body) else {
        return String::new();
    };
    let mut parts = Vec::new();
    if let Some(message) = body.get("message").and_then(serde_json::Value::as_str) {
        parts.push(message);
    }
    for error in body
        .get("errors")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
    {
        for key in ["message", "field"] {
            if let Some(text) = error.get(key).and_then(serde_json::Value::as_str) {
                parts.push(text);
            }
        }
    }
    parts.join("\n").to_lowercase()
}

#[cfg(test)]
mod tests {
    use mintswarm_domain::retry::{DEFAULT_ATTEMPTS, DEFAULT_TOTAL, Failure, RetryPolicy};
    use reqwest::Method;

    use super::{Http, links_next, path_segment};
    use crate::retry::{Request, Transport};
    use crate::secret::Token;
    use crate::token::REQUEST_TIMEOUT;

    #[test]
    fn the_default_total_is_derived_from_the_request_timeout() {
        let policy = RetryPolicy::default();
        let waits: std::time::Duration = (1..DEFAULT_ATTEMPTS).map(|n| policy.ceiling(n)).sum();
        assert_eq!(REQUEST_TIMEOUT * DEFAULT_ATTEMPTS + waits, DEFAULT_TOTAL);
    }

    /// One attempt of a GET at `path` with `token` through an [`Http`] at `base`.
    async fn send(base: &str, path: &str, token: &str) -> Result<(), Failure> {
        let http = Http::new(base).expect("a client");
        let token = Token::new(token.to_owned(), 0);
        let request = Request {
            method: Method::GET,
            path,
            token: &token,
            body: None,
            if_none_match: None,
        };
        http.send(&request).await.map(|_| ())
    }

    #[tokio::test]
    async fn a_request_that_cannot_be_built_is_its_own_class() {
        // A header that cannot hold the token, and a URL that does not parse; neither is sent.
        assert_eq!(
            send("http://127.0.0.1:9", "/x", "ghs_bad\ntoken").await,
            Err(Failure::Request)
        );
        assert_eq!(
            send("not a url", "/x", "ghs_t0ken").await,
            Err(Failure::Request)
        );
    }

    #[test]
    fn the_link_header_names_a_next_page() {
        let base = "https://api.github.com/repositories/1/issues/180/comments";
        assert!(links_next(&format!(
            "<{base}?page=2>; rel=\"next\", <{base}?page=5>; rel=\"last\""
        )));
        assert!(!links_next(&format!(
            "<{base}?page=1>; rel=\"first\", <{base}?page=29>; rel=\"prev\""
        )));
        assert!(!links_next(""));
        // A URL that merely contains "next" is not a next link.
        assert!(!links_next(&format!("<{base}?next=1>; rel=\"prev\"")));
    }

    #[test]
    fn label_names_are_encoded_as_one_path_segment() {
        assert_eq!(path_segment("effort:S"), "effort%3AS");
        assert_eq!(
            path_segment("[M] size/é?#"),
            "%5BM%5D%20size%2F%C3%A9%3F%23"
        );
        assert_eq!(path_segment("a-b.c_d~1"), "a-b.c_d~1");
    }
}
