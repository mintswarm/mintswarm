//! Installation tokens, each narrowed to one repository and the permissions asked for, each at
//! read or write level, kept in memory and minted again five minutes before they expire.

use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use reqwest::StatusCode;
use serde::Deserialize;
use tokio::sync::Mutex;

use mintswarm_domain::retry::{Exhausted, RetryPolicy};

use crate::config::{AppConfig, ConfigError};
use crate::jwt::{JwtError, app_jwt};
use crate::key_store::{KeyStoreEnv, KeyStoreError, Tools, read_key};
use crate::rest::Rest;
use crate::secret::Token;

/// GitHub's REST API.
pub const GITHUB_API: &str = "https://api.github.com";
/// A cached token is minted again once fewer than this many seconds remain before it expires.
pub const REFRESH_MARGIN_SECS: u64 = 300;
/// How long connecting to GitHub may take.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// How long a whole request to GitHub may take. [`DEFAULT_TOTAL`] is derived from it, four of
/// these plus the waits between them; a test in `rest.rs` fails when the two drift apart.
///
/// [`DEFAULT_TOTAL`]: mintswarm_domain::retry::DEFAULT_TOTAL
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// The source of the current Unix time, in seconds.
#[derive(Clone)]
pub struct Clock(Arc<dyn Fn() -> u64 + Send + Sync>);

impl Clock {
    /// The system clock.
    pub fn system() -> Self {
        Self(Arc::new(|| {
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |elapsed| elapsed.as_secs())
        }))
    }

    /// A clock that asks `now`.
    pub fn from_fn(now: impl Fn() -> u64 + Send + Sync + 'static) -> Self {
        Self(Arc::new(now))
    }

    /// The current Unix time, in seconds.
    pub fn now(&self) -> u64 {
        (self.0)()
    }
}

impl fmt::Debug for Clock {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Clock")
    }
}

/// The level of access a permission is asked for at.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Access {
    /// Read access.
    Read,
    /// Write access, which on GitHub includes read access.
    Write,
}

impl Access {
    fn as_str(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Write => "write",
        }
    }
}

/// The permissions an installation token is narrowed to, each by name at read or write level.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Permissions(BTreeMap<String, Access>);

impl Permissions {
    /// Read access to each of `names`, for example `contents` or `issues`.
    pub fn read<'a>(names: impl IntoIterator<Item = &'a str>) -> Self {
        Self::at(names, Access::Read)
    }

    /// Write access to each of `names`, for example `issues`. It grants nothing else: a write
    /// that also needs `metadata` asks for it with [`Permissions::and`].
    pub fn write<'a>(names: impl IntoIterator<Item = &'a str>) -> Self {
        Self::at(names, Access::Write)
    }

    /// Both sets; a name in both is asked for at the higher level, write over read.
    #[must_use]
    pub fn and(mut self, other: Self) -> Self {
        for (name, access) in other.0 {
            let level = self.0.entry(name).or_insert(access);
            *level = (*level).max(access);
        }
        self
    }

    /// Whether no permission is asked for.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    fn at<'a>(names: impl IntoIterator<Item = &'a str>, access: Access) -> Self {
        Self(
            names
                .into_iter()
                .map(|name| (name.to_owned(), access))
                .collect(),
        )
    }

    fn body(&self) -> serde_json::Map<String, serde_json::Value> {
        self.0
            .iter()
            .map(|(name, access)| (name.clone(), serde_json::Value::from(access.as_str())))
            .collect()
    }
}

/// Why a token could not be had. No variant carries a response body or a credential.
#[derive(Debug, thiserror::Error)]
pub enum TokenError {
    /// No permission was asked for; GitHub would grant the installation's full permissions.
    #[error("an installation token needs at least one permission")]
    NoPermissions,
    /// A permission, owner or repository name is not valid.
    #[error("invalid {what}: '{value}'")]
    InvalidName {
        /// What the name is.
        what: &'static str,
        /// The name.
        value: String,
    },
    /// The App configuration could not be used.
    #[error(transparent)]
    Config(#[from] ConfigError),
    /// The App's key could not be read.
    #[error(transparent)]
    KeyStore(#[from] KeyStoreError),
    /// The JWT could not be signed.
    #[error(transparent)]
    Jwt(#[from] JwtError),
    /// The HTTP client could not be built.
    #[error("cannot build the HTTP client")]
    Client,
    /// The request kept failing transiently until its retries ran out.
    #[error(transparent)]
    Exhausted(#[from] Exhausted),
    /// GitHub answered with an error status.
    #[error("{endpoint}: GitHub answered {status}")]
    Status {
        /// The method and path.
        endpoint: String,
        /// The status.
        status: StatusCode,
    },
    /// GitHub's answer is not what was expected.
    #[error("{endpoint}: unexpected response ({what})")]
    Response {
        /// The method and path.
        endpoint: String,
        /// What is wrong with it.
        what: &'static str,
    },
}

#[derive(Clone, PartialEq, Eq, Hash)]
struct CacheKey {
    owner: String,
    repo: String,
    permissions: Permissions,
}

/// One slot per owner, repository and permission set. The map's lock is held only to find a
/// slot; a slot's own lock is held while its token is minted, so a slow mint delays only the
/// callers that want the same token.
#[derive(Default)]
struct Cache(HashMap<CacheKey, Arc<Mutex<Option<Token>>>>);

impl fmt::Debug for Cache {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // A slot that is locked is being minted, and counts as empty.
        let tokens = self
            .0
            .values()
            .filter(|slot| slot.try_lock().is_ok_and(|token| token.is_some()))
            .count();
        f.debug_struct("Cache").field("tokens", &tokens).finish()
    }
}

/// Mints installation tokens as the App configured at `app_config_path`.
pub struct TokenClient {
    rest: Rest,
    api_base: String,
    app_config_path: PathBuf,
    tools: Tools,
    env: KeyStoreEnv,
    clock: Clock,
    cache: Mutex<Cache>,
}

impl fmt::Debug for TokenClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let cache = match self.cache.try_lock() {
            Ok(cache) => format!("{:?}", *cache),
            Err(_) => String::from("locked"),
        };
        f.debug_struct("TokenClient")
            .field("api_base", &self.api_base)
            .field("app_config_path", &self.app_config_path)
            .field("clock", &self.clock)
            .field("cache", &cache)
            .finish_non_exhaustive()
    }
}

#[derive(Deserialize)]
struct Installation {
    id: u64,
}

#[derive(Deserialize)]
struct AccessToken {
    token: String,
    expires_at: String,
}

impl TokenClient {
    /// A client for the API at `api_base` (normally [`GITHUB_API`]), reading the App's
    /// configuration from `app_config_path` and its key with `tools` in `env`, retrying with
    /// the default [`RetryPolicy`].
    pub fn new(
        api_base: impl Into<String>,
        app_config_path: PathBuf,
        tools: Tools,
        env: KeyStoreEnv,
        clock: Clock,
    ) -> Result<Self, TokenError> {
        let api_base = api_base.into().trim_end_matches('/').to_owned();
        Ok(Self {
            rest: Rest::new(&api_base, RetryPolicy::default()).map_err(|_| TokenError::Client)?,
            api_base,
            app_config_path,
            tools,
            env,
            clock,
            cache: Mutex::new(Cache::default()),
        })
    }

    /// The client retrying with `policy` in place of the default.
    #[must_use]
    pub fn with_retry(mut self, policy: RetryPolicy) -> Self {
        self.rest = self.rest.with_policy(policy);
        self
    }

    /// An installation token for `owner/repo` with `permissions` only, each at its level: the
    /// cached one while more than [`REFRESH_MARGIN_SECS`] remain, otherwise a new one.
    pub async fn token(
        &self,
        owner: &str,
        repo: &str,
        permissions: &Permissions,
    ) -> Result<Token, TokenError> {
        if permissions.is_empty() {
            return Err(TokenError::NoPermissions);
        }
        for name in permissions.0.keys() {
            if name.is_empty() || !name.chars().all(|c| c.is_ascii_lowercase() || c == '_') {
                return Err(TokenError::InvalidName {
                    what: "permission",
                    value: name.clone(),
                });
            }
        }
        for (what, value) in [("owner", owner), ("repository", repo)] {
            if !crate::config::valid_name(value) {
                return Err(TokenError::InvalidName {
                    what,
                    value: value.to_owned(),
                });
            }
        }
        let key = CacheKey {
            owner: owner.to_owned(),
            repo: repo.to_owned(),
            permissions: permissions.clone(),
        };
        let slot = Arc::clone(self.cache.lock().await.0.entry(key).or_default());
        let mut slot = slot.lock().await;
        let now = self.clock.now();
        if let Some(token) = slot.as_ref()
            && token.expires_at() >= now + REFRESH_MARGIN_SECS
        {
            tracing::debug!(owner, repo, "using the cached installation token");
            return Ok(token.clone());
        }
        let token = self.mint(owner, repo, permissions, now).await?;
        *slot = Some(token.clone());
        Ok(token)
    }

    async fn mint(
        &self,
        owner: &str,
        repo: &str,
        permissions: &Permissions,
        now: u64,
    ) -> Result<Token, TokenError> {
        tracing::debug!(owner, repo, ?permissions, "minting an installation token");
        let config = AppConfig::load(&self.app_config_path)?;
        let store = config.key_store()?;
        let jwt = {
            let key = read_key(
                store,
                &config.keychain_service,
                &config.client_id,
                &self.tools,
                &self.env,
            )
            .await?;
            app_jwt(&config, &key, now)?
        };

        let path = format!("/repos/{owner}/{repo}/installation");
        let installation: Installation = self.call(&path, &jwt, None).await?;

        let path = format!("/app/installations/{}/access_tokens", installation.id);
        let body = serde_json::json!({
            "repositories": [repo],
            "permissions": permissions.body(),
        });
        let response: AccessToken = self.call(&path, &jwt, Some(&body)).await?;
        let endpoint = || format!("POST {path}");
        if !response.token.starts_with("ghs_") || response.token.len() <= 4 {
            return Err(TokenError::Response {
                endpoint: endpoint(),
                what: "not an installation token",
            });
        }
        let expires_at = parse_utc(&response.expires_at).ok_or_else(|| TokenError::Response {
            endpoint: endpoint(),
            what: "expires_at is not YYYY-MM-DDTHH:MM:SSZ",
        })?;
        tracing::debug!(
            owner,
            repo,
            installation = installation.id,
            expires_at,
            "minted an installation token"
        );
        Ok(Token::new(response.token, expires_at))
    }

    /// A GET of `path`, or a POST of `body` to it, with `jwt`, answered as JSON.
    async fn call<T: for<'de> Deserialize<'de>>(
        &self,
        path: &str,
        jwt: &Token,
        body: Option<&serde_json::Value>,
    ) -> Result<T, TokenError> {
        let reply = match body {
            None => self.rest.get(path, jwt, None).await?,
            // Minting mints a new token each time; a repeat after a first that landed costs one
            // more token, which simply goes unused, and changes nothing else.
            Some(body) => self.rest.post_idempotent(path, jwt, body).await?,
        };
        if !reply.status.is_success() {
            return Err(TokenError::Status {
                endpoint: reply.endpoint,
                status: reply.status,
            });
        }
        serde_json::from_slice(&reply.body).map_err(|_| TokenError::Response {
            endpoint: reply.endpoint,
            what: "not the expected JSON",
        })
    }
}

/// Parses `YYYY-MM-DDTHH:MM:SSZ` into a Unix time.
fn parse_utc(text: &str) -> Option<u64> {
    let b = text.as_bytes();
    if !text.is_ascii()
        || b.len() != 20
        || b[4] != b'-'
        || b[7] != b'-'
        || b[10] != b'T'
        || b[13] != b':'
        || b[16] != b':'
        || b[19] != b'Z'
    {
        return None;
    }
    let number = |range: std::ops::Range<usize>| -> Option<u64> {
        let digits = &text[range];
        if digits.bytes().all(|c| c.is_ascii_digit()) {
            digits.parse().ok()
        } else {
            None
        }
    };
    let (year, month, day) = (number(0..4)?, number(5..7)?, number(8..10)?);
    let (hour, minute, second) = (number(11..13)?, number(14..16)?, number(17..19)?);
    if year < 1970
        || !(1..=12).contains(&month)
        || day < 1
        || day > days_in_month(year, month)
        || hour > 23
        || minute > 59
        || second > 59
    {
        return None;
    }
    Some(days_from_civil(year, month, day) * 86_400 + hour * 3600 + minute * 60 + second)
}

fn days_in_month(year: u64, month: u64) -> u64 {
    match month {
        2 if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// Days from 1970-01-01 to a date on or after it (Howard Hinnant's `days_from_civil`).
fn days_from_civil(year: u64, month: u64, day: u64) -> u64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year / 400;
    let year_of_era = year - era * 400;
    let month_index = (month + 9) % 12;
    let day_of_year = (153 * month_index + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

#[cfg(test)]
mod tests {
    use super::{Permissions, parse_utc};

    #[test]
    fn parses_utc_times() {
        assert_eq!(parse_utc("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_utc("2000-03-01T00:00:00Z"), Some(951_868_800));
        assert_eq!(parse_utc("2024-02-29T12:34:56Z"), Some(1_709_210_096));
        assert_eq!(parse_utc("2027-01-15T08:00:00Z"), Some(1_800_000_000));
    }

    #[test]
    fn refuses_other_times() {
        for bad in [
            "",
            "2027-01-15 08:00:00Z",
            "2027-01-15T08:00:00+00:00",
            "2027-01-15T08:00:00.000Z",
            "2027-13-01T00:00:00Z",
            "2023-02-29T00:00:00Z",
            "2027-01-15T24:00:00Z",
            "1969-12-31T23:59:59Z",
            "2027-0a-15T08:00:00Z",
            "+027-01-15T08:00:00Z",
        ] {
            assert_eq!(parse_utc(bad), None, "{bad}");
        }
    }

    #[test]
    fn permissions_are_sorted_with_their_levels_and_write_wins() {
        let permissions = Permissions::read(["issues", "contents", "issues"]);
        let body = serde_json::Value::Object(permissions.body());
        assert_eq!(
            body,
            serde_json::json!({"contents": "read", "issues": "read"})
        );
        assert!(Permissions::read([]).is_empty());
        assert!(Permissions::write([]).is_empty());

        let body = |permissions: Permissions| serde_json::Value::Object(permissions.body());
        assert_eq!(
            body(Permissions::write(["issues"]).and(Permissions::read(["metadata"]))),
            serde_json::json!({"issues": "write", "metadata": "read"})
        );
        // A name asked for at both levels is asked for at write, in either order.
        let expected = serde_json::json!({"contents": "read", "issues": "write"});
        assert_eq!(
            body(Permissions::read(["issues", "contents"]).and(Permissions::write(["issues"]))),
            expected
        );
        assert_eq!(
            body(Permissions::write(["issues"]).and(Permissions::read(["issues", "contents"]))),
            expected
        );
        assert_ne!(
            Permissions::read(["issues"]),
            Permissions::write(["issues"])
        );
    }
}
