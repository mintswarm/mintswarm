//! The App JWT: RS256, issued a minute in the past so clock drift cannot make it early, and
//! valid for nine minutes after `now`, inside GitHub's ten-minute limit.

use jsonwebtoken::{Algorithm, EncodingKey, Header};
use serde::Serialize;

use crate::config::AppConfig;
use crate::secret::{SecretBytes, Token};

/// How far before `now` the JWT is issued.
pub const JWT_BACKDATE_SECS: u64 = 60;
/// How long after `now` the JWT expires.
pub const JWT_LIFETIME_SECS: u64 = 540;

/// Why a JWT could not be signed. It carries no key material.
#[derive(Debug, thiserror::Error)]
pub enum JwtError {
    /// The key is not an RSA private key in PEM.
    #[error("the App's key is not an RSA private key in PEM")]
    Key,
    /// Signing failed.
    #[error("signing the App JWT failed")]
    Sign,
}

#[derive(Serialize)]
struct Claims<'a> {
    iat: u64,
    exp: u64,
    iss: &'a str,
}

/// Signs an App JWT for `config` with the private key `key`, at the Unix time `now`.
pub fn app_jwt(config: &AppConfig, key: &SecretBytes, now: u64) -> Result<Token, JwtError> {
    let key = EncodingKey::from_rsa_pem(key.expose()).map_err(|_| JwtError::Key)?;
    let exp = now + JWT_LIFETIME_SECS;
    let claims = Claims {
        iat: now.saturating_sub(JWT_BACKDATE_SECS),
        exp,
        iss: &config.client_id,
    };
    let jwt = jsonwebtoken::encode(&Header::new(Algorithm::RS256), &claims, &key)
        .map_err(|_| JwtError::Sign)?;
    Ok(Token::new(jwt, exp))
}
