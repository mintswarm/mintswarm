//! A minimal authenticated REST helper for the field client and the field catalogue: one request
//! with a caller's token, answered with its status, ETag, rate-limit state and raw body.

use reqwest::header::{
    ACCEPT, AUTHORIZATION, CONTENT_TYPE, ETAG, HeaderMap, HeaderValue, IF_NONE_MATCH, RETRY_AFTER,
    USER_AGENT,
};
use reqwest::{Method, StatusCode};

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
}

impl Rest {
    /// A client for the API at `api_base`, with the token client's timeouts.
    pub fn new(api_base: &str) -> Result<Self, ClientError> {
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
        let body = response.bytes().await.map_err(|_| failed())?.to_vec();
        Ok(Reply {
            endpoint,
            status,
            etag,
            rate_limited,
            body,
        })
    }
}
