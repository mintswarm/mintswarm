//! GitHub access for the runner: it acts as the runner App with an RS256 App JWT and
//! installation tokens narrowed to one repository and the read permissions each use asks for.
//! The App's private key is read from the key store its configuration names, for each JWT, and
//! dropped after signing. [`FieldClient`] and [`GitHubFieldCatalogue`] read and write a role's
//! issue field or labels with a token their caller passes in.
//!
//! [`GitHubReads`] is the runner's GET-only view of one repository, design §9 "Reads and
//! writes": issues with their comments since a cursor, parent and sub-issue links, issue
//! artifacts and pull requests with their reviews and checks, paged, revalidated with ETags and
//! paused by `retry-after`. [`comment_mark`] tells steers and notes from other comments.
//! [`ContentLimiter`] budgets content-creating requests for the writes to come.

mod catalogue;
mod config;
mod etag;
mod fields;
mod jwt;
mod key_store;
mod limit;
mod pulls;
mod reads;
mod rest;
mod secret;
mod token;

pub use catalogue::GitHubFieldCatalogue;
pub use config::{AppConfig, ConfigError, KeyStore};
pub use fields::{FieldClient, FieldPass};
pub use jwt::{JWT_BACKDATE_SECS, JWT_LIFETIME_SECS, JwtError, app_jwt};
pub use key_store::{DEFAULT_TIMEOUT, KeyStoreEnv, KeyStoreError, Tools, read_key};
pub use limit::{CONTENT_PER_HOUR, CONTENT_PER_MINUTE, ContentLimiter};
pub use reads::{
    GitHubReads, KindSource, Kinds, MAX_PAGES, NOTE_MARKER, PER_PAGE, ReadError, STEER_MARKER,
    comment_mark,
};
pub use rest::ClientError;
pub use secret::{SecretBytes, Token};
pub use token::{
    CONNECT_TIMEOUT, Clock, GITHUB_API, Permissions, REFRESH_MARGIN_SECS, REQUEST_TIMEOUT,
    TokenClient, TokenError,
};
