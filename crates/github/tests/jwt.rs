//! A minted App JWT verifies against the public half of a throwaway key, with the claims GitHub
//! requires.
// Not redundant: clippy's `allow-unwrap-in-tests` covers a `tests/` crate only under `cfg(test)`.
#![cfg(test)]

mod support;

use std::time::{SystemTime, UNIX_EPOCH};

use jsonwebtoken::{Algorithm, DecodingKey, Validation};
use mintswarm_github::{AppConfig, JwtError, KeyStore, SecretBytes, app_jwt};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct Claims {
    iat: u64,
    exp: u64,
    iss: String,
}

fn config() -> AppConfig {
    AppConfig {
        app_id: 1,
        slug: String::from("mintswarm-dev"),
        client_id: String::from("Iv23liJwtTest"),
        keychain_service: String::from("mintswarm-dev"),
        key_store: Some(KeyStore::File),
        key_fingerprint: None,
    }
}

#[test]
fn the_jwt_verifies_with_the_claims_github_requires() {
    let keys = support::key_pair();
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let jwt = app_jwt(&config(), &SecretBytes::new(keys.private_pem.clone()), now).unwrap();
    assert_eq!(jwt.expires_at(), now + 540);

    let header = jsonwebtoken::decode_header(jwt.expose()).unwrap();
    assert_eq!(header.alg, Algorithm::RS256);

    let mut validation = Validation::new(Algorithm::RS256);
    validation.set_issuer(&["Iv23liJwtTest"]);
    validation.set_required_spec_claims(&["iat", "exp", "iss"]);
    let key = DecodingKey::from_rsa_pem(&keys.public_pem).unwrap();
    let claims = jsonwebtoken::decode::<Claims>(jwt.expose(), &key, &validation)
        .unwrap()
        .claims;
    assert_eq!(claims.iss, "Iv23liJwtTest");
    assert_eq!(claims.iat, now - 60);
    assert_eq!(claims.exp, now + 540);
    assert!(claims.exp - claims.iat <= 600);
}

#[test]
fn the_jwt_does_not_verify_with_another_key() {
    let keys = support::key_pair();
    let jwt = app_jwt(
        &config(),
        &SecretBytes::new(keys.private_pem.clone()),
        1_800_000_000,
    );
    let jwt = jwt.unwrap();
    let mut validation = Validation::new(Algorithm::RS256);
    validation.validate_exp = false;
    let other = DecodingKey::from_secret(b"not the key");
    assert!(jsonwebtoken::decode::<Claims>(jwt.expose(), &other, &validation).is_err());
}

#[test]
fn a_key_that_is_not_rsa_pem_is_refused_without_showing_it() {
    let key = SecretBytes::new(b"-----BEGIN NOT A KEY-----\nc2VjcmV0\n".to_vec());
    let error = app_jwt(&config(), &key, 1_800_000_000).unwrap_err();
    assert!(matches!(error, JwtError::Key));
    assert!(!format!("{error} {error:?}").contains("c2VjcmV0"));
}
