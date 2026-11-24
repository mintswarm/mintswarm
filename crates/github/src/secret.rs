//! Newtypes for secret values: their `Debug` is redacted, they have no `Display` and no
//! `Serialize`, and [`Token::expose`] or [`SecretBytes::expose`] is the only way to read them.

use std::fmt;

/// A bearer credential, such as an installation token or an App JWT, with the Unix time at
/// which it expires.
///
/// It has no `Display`, so it cannot be formatted by accident:
///
/// ```compile_fail
/// let token = mintswarm_github::Token::new(String::from("ghs_example"), 0);
/// let _ = format!("{}", token);
/// ```
#[derive(Clone, PartialEq, Eq)]
pub struct Token {
    value: String,
    expires_at: u64,
}

impl Token {
    /// Wraps `value`, which expires at the Unix time `expires_at`.
    pub fn new(value: String, expires_at: u64) -> Self {
        Self { value, expires_at }
    }

    /// The secret value. Pass it straight to the code that needs it; never log or print it.
    pub fn expose(&self) -> &str {
        &self.value
    }

    /// The Unix time, in seconds, at which the credential expires.
    pub fn expires_at(&self) -> u64 {
        self.expires_at
    }
}

impl fmt::Debug for Token {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Token")
            .field("value", &Redacted)
            .field("expires_at", &self.expires_at)
            .finish()
    }
}

/// Secret bytes, such as an App's private key read from a key store.
///
/// It has no `Display`:
///
/// ```compile_fail
/// let key = mintswarm_github::SecretBytes::new(b"key".to_vec());
/// let _ = format!("{}", key);
/// ```
#[derive(Clone, PartialEq, Eq)]
pub struct SecretBytes(Vec<u8>);

impl SecretBytes {
    /// Wraps `bytes`.
    pub fn new(bytes: Vec<u8>) -> Self {
        Self(bytes)
    }

    /// The secret bytes. Pass them straight to the code that needs them; never log them.
    pub fn expose(&self) -> &[u8] {
        &self.0
    }

    /// Whether there are no bytes.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl fmt::Debug for SecretBytes {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("SecretBytes").field(&Redacted).finish()
    }
}

/// Prints `redacted` in place of a secret.
struct Redacted;

impl fmt::Debug for Redacted {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("redacted")
    }
}

#[cfg(test)]
mod tests {
    use super::{SecretBytes, Token};

    const VALUE: &str = "ghs_s3cretV4lue";

    fn assert_hidden(text: &str) {
        assert!(text.contains("redacted"), "{text}");
        for part in ["ghs_", "s3cret", "V4lue"] {
            assert!(!text.contains(part), "{text} shows {part}");
        }
    }

    #[test]
    fn token_debug_hides_the_value() {
        let token = Token::new(VALUE.to_owned(), 42);
        assert_hidden(&format!("{token:?}"));
        assert_hidden(&format!("{token:#?}"));
        assert!(format!("{token:?}").contains("42"));
        assert_eq!(token.expose(), VALUE);
        assert_eq!(token.expires_at(), 42);
    }

    #[test]
    fn secret_bytes_debug_hides_the_value() {
        let bytes = SecretBytes::new(VALUE.as_bytes().to_vec());
        assert_hidden(&format!("{bytes:?}"));
        assert_hidden(&format!("{bytes:#?}"));
        assert_eq!(bytes.expose(), VALUE.as_bytes());
        assert!(!bytes.is_empty());
    }
}
