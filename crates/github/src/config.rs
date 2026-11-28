//! The runner App's (the bee's) configuration, `~/.config/mintswarm/bee-app.json`. It holds no
//! secret: the private key stays in the key store the configuration names.

use std::path::{Path, PathBuf};

use mintswarm_domain::tracker::Login;
use serde::Deserialize;

/// The key store that holds an App's private key.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum KeyStore {
    /// The macOS login Keychain, through `security`.
    Keychain,
    /// The freedesktop Secret Service, through `secret-tool`.
    SecretService,
    /// The `pass` password store.
    Pass,
    /// A systemd credential, passed in `$CREDENTIALS_DIRECTORY` or decrypted with
    /// `systemd-creds`.
    SystemdCreds,
    /// An unencrypted file under `~/.config/mintswarm/keys`.
    File,
}

impl KeyStore {
    /// The name the configuration and `MSW_KEY_STORE` use for this store.
    pub fn name(self) -> &'static str {
        match self {
            Self::Keychain => "keychain",
            Self::SecretService => "secret-service",
            Self::Pass => "pass",
            Self::SystemdCreds => "systemd-creds",
            Self::File => "file",
        }
    }
}

/// An App's identifiers and where its private key is kept. Unknown fields, such as a
/// `key_confirm` an older `key-store.sh` recorded, are ignored: the runner runs unattended, so a
/// key read that waits for a confirmation is stopped by the key store's timeout instead.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct AppConfig {
    /// The App's numeric id.
    pub app_id: u64,
    /// The App's slug, for example `acme-bee`.
    pub slug: String,
    /// The App's client id: the JWT's issuer, and the key's account name in every key store.
    pub client_id: String,
    /// The key's service name in every key store.
    pub keychain_service: String,
    /// The key store the key was written to, when recorded.
    #[serde(default)]
    pub key_store: Option<KeyStore>,
    /// The fingerprint of the key, for example `SHA256:…`.
    #[serde(default)]
    pub key_fingerprint: Option<String>,
    /// The App's bot user id, which `store-credentials.sh` records: the number in the bot's
    /// noreply e-mail.
    #[serde(default)]
    pub bot_user_id: Option<u64>,
}

/// Why the App's configuration could not be used.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// The file could not be read.
    #[error("cannot read the App config {path}: {source}")]
    Read {
        /// The configuration file.
        path: PathBuf,
        /// The underlying error.
        source: std::io::Error,
    },
    /// The file is not a valid App configuration.
    #[error("the App config {path} is not valid: {source}")]
    Parse {
        /// The configuration file.
        path: PathBuf,
        /// The underlying error.
        source: serde_json::Error,
    },
    /// A name does not match `[A-Za-z0-9][A-Za-z0-9._-]*`.
    #[error("invalid {field} in the App config: '{value}'")]
    InvalidName {
        /// The field.
        field: &'static str,
        /// Its value.
        value: String,
    },
    /// The configuration records no key store, and this platform has no default.
    #[error(
        "key_store is missing from the App config: store the key with \
         MSW_KEY_STORE=<one of: keychain, secret-service, pass, systemd-creds, file> \
         store-credentials.sh rotate PEM_PATH"
    )]
    NoKeyStore,
}

impl AppConfig {
    /// The bee's (the runner App's) configuration file below `home`:
    /// `home/.config/mintswarm/bee-app.json`.
    pub fn default_path(home: &Path) -> PathBuf {
        home.join(".config").join("mintswarm").join("bee-app.json")
    }

    /// Reads and checks the configuration at `path`.
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        let text = std::fs::read(path).map_err(|source| ConfigError::Read {
            path: path.to_owned(),
            source,
        })?;
        Self::parse(&text).map_err(|error| match error {
            ParseError::Json(source) => ConfigError::Parse {
                path: path.to_owned(),
                source,
            },
            ParseError::Config(error) => error,
        })
    }

    fn parse(text: &[u8]) -> Result<Self, ParseError> {
        let config: Self = serde_json::from_slice(text).map_err(ParseError::Json)?;
        for (field, value) in [
            ("client_id", &config.client_id),
            ("keychain_service", &config.keychain_service),
        ] {
            if !valid_name(value) {
                return Err(ParseError::Config(ConfigError::InvalidName {
                    field,
                    value: value.clone(),
                }));
            }
        }
        Ok(config)
    }

    /// The App's bot login, derived from its slug: `<slug>[bot]`.
    pub fn bot_login(&self) -> Login {
        Login(format!("{}[bot]", self.slug))
    }

    /// The App's bot noreply e-mail, derived from its slug and bot user id:
    /// `<bot_user_id>+<slug>[bot]@users.noreply.github.com`, or `None` when no id is recorded.
    pub fn noreply_email(&self) -> Option<String> {
        self.bot_user_id
            .map(|id| format!("{id}+{}[bot]@users.noreply.github.com", self.slug))
    }

    /// The key store that holds the key: the recorded one, or `keychain` on macOS when none is
    /// recorded.
    pub fn key_store(&self) -> Result<KeyStore, ConfigError> {
        match self.key_store {
            Some(store) => Ok(store),
            None if cfg!(target_os = "macos") => Ok(KeyStore::Keychain),
            None => Err(ConfigError::NoKeyStore),
        }
    }
}

enum ParseError {
    Json(serde_json::Error),
    Config(ConfigError),
}

/// Whether `value` matches `[A-Za-z0-9][A-Za-z0-9._-]*`, the names key stores accept.
pub(crate) fn valid_name(value: &str) -> bool {
    let mut chars = value.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphanumeric())
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use mintswarm_domain::tracker::Login;

    use super::{AppConfig, ConfigError, KeyStore, ParseError, valid_name};

    const FULL: &str = r#"{"app_id": 123, "slug": "acme-bee", "client_id": "Iv23liAbc",
        "keychain_service": "acme-bee", "key_store": "secret-service",
        "key_fingerprint": "SHA256:abc", "bot_user_id": 7}"#;

    fn parse(text: &str) -> Result<AppConfig, ParseError> {
        AppConfig::parse(text.as_bytes())
    }

    #[test]
    fn reads_every_field() {
        let config = parse(FULL).ok().unwrap();
        assert_eq!(config.app_id, 123);
        assert_eq!(config.slug, "acme-bee");
        assert_eq!(config.client_id, "Iv23liAbc");
        assert_eq!(config.keychain_service, "acme-bee");
        assert_eq!(config.key_store, Some(KeyStore::SecretService));
        assert_eq!(config.key_fingerprint.as_deref(), Some("SHA256:abc"));
        assert_eq!(config.key_store().unwrap(), KeyStore::SecretService);
        assert_eq!(config.bot_user_id, Some(7));
    }

    #[test]
    fn derives_the_bot_login_from_the_slug() {
        let config = parse(FULL).ok().unwrap();
        assert_eq!(config.bot_login(), Login(String::from("acme-bee[bot]")));
    }

    #[test]
    fn derives_the_noreply_email_from_the_slug_and_the_id() {
        let config = parse(FULL).ok().unwrap();
        assert_eq!(
            config.noreply_email().as_deref(),
            Some("7+acme-bee[bot]@users.noreply.github.com")
        );
    }

    #[test]
    fn has_no_noreply_email_without_an_id() {
        let text =
            r#"{"app_id": 1, "slug": "acme-bee", "client_id": "c", "keychain_service": "k"}"#;
        let config = parse(text).ok().unwrap();
        assert_eq!(config.bot_user_id, None);
        assert_eq!(config.bot_login(), Login(String::from("acme-bee[bot]")));
        assert_eq!(config.noreply_email(), None);
    }

    #[test]
    fn reads_every_store_name() {
        for store in [
            KeyStore::Keychain,
            KeyStore::SecretService,
            KeyStore::Pass,
            KeyStore::SystemdCreds,
            KeyStore::File,
        ] {
            let text = FULL.replace("secret-service", store.name());
            assert_eq!(parse(&text).ok().unwrap().key_store, Some(store));
        }
    }

    #[test]
    fn refuses_an_unknown_store() {
        let text = FULL.replace("secret-service", "vault");
        assert!(matches!(parse(&text), Err(ParseError::Json(_))));
    }

    #[test]
    fn optional_fields_default() {
        let text = r#"{"app_id": 1, "slug": "s", "client_id": "c", "keychain_service": "k"}"#;
        let config = parse(text).ok().unwrap();
        assert_eq!(config.key_store, None);
        assert_eq!(config.key_fingerprint, None);
        assert_eq!(config.bot_user_id, None);
        if cfg!(target_os = "macos") {
            assert_eq!(config.key_store().unwrap(), KeyStore::Keychain);
        } else {
            assert!(matches!(config.key_store(), Err(ConfigError::NoKeyStore)));
        }
    }

    #[test]
    fn ignores_a_recorded_key_confirm() {
        let text = FULL.replace('}', r#", "key_confirm": "keychain-acl"}"#);
        assert_eq!(parse(&text).ok().unwrap(), parse(FULL).ok().unwrap());
    }

    #[test]
    fn refuses_a_missing_field() {
        let text = r#"{"app_id": 1, "slug": "s", "keychain_service": "k"}"#;
        assert!(matches!(parse(text), Err(ParseError::Json(_))));
    }

    #[test]
    fn refuses_invalid_names() {
        for field in ["client_id", "keychain_service"] {
            for value in ["-lead", ".lead", "has space", "a/b", ""] {
                let mut json: serde_json::Value = serde_json::from_str(FULL).unwrap();
                json[field] = serde_json::Value::from(value);
                match parse(&json.to_string()) {
                    Err(ParseError::Config(ConfigError::InvalidName { field: f, .. })) => {
                        assert_eq!(f, field, "{value}");
                    }
                    _ => panic!("{field} '{value}' was accepted"),
                }
            }
        }
    }

    #[test]
    fn name_rule() {
        for good in ["a", "A1", "acme-bee", "Iv23li.x_y-z", "0"] {
            assert!(valid_name(good), "{good}");
        }
        for bad in ["", "-a", "_a", ".a", "a b", "a/b", "a@b", "é"] {
            assert!(!valid_name(bad), "{bad}");
        }
    }

    #[test]
    fn default_path_is_below_home() {
        assert_eq!(
            AppConfig::default_path(Path::new("/home/u")),
            Path::new("/home/u/.config/mintswarm/bee-app.json")
        );
    }

    #[test]
    fn load_reports_the_path() {
        let missing = Path::new("/nonexistent/mintswarm/bee-app.json");
        let error = AppConfig::load(missing).unwrap_err();
        assert!(matches!(error, ConfigError::Read { .. }));
        assert!(error.to_string().contains("/nonexistent/mintswarm"));
    }
}
