//! GitHub access for the runner: it acts as the runner App with an RS256 App JWT and
//! installation tokens narrowed to one repository and the read permissions each use asks for.
//! The App's private key is read from the key store its configuration names, for each JWT, and
//! dropped after signing.

mod config;
mod jwt;
mod key_store;
mod secret;
mod token;

pub use config::{AppConfig, ConfigError, KeyConfirm, KeyStore};
pub use jwt::{JWT_BACKDATE_SECS, JWT_LIFETIME_SECS, JwtError, app_jwt};
pub use key_store::{DEFAULT_TIMEOUT, KeyStoreEnv, KeyStoreError, Tools, read_key};
pub use secret::{SecretBytes, Token};
pub use token::{Clock, GITHUB_API, Permissions, REFRESH_MARGIN_SECS, TokenClient, TokenError};
