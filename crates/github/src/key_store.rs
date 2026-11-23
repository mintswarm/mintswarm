//! Readers for the key stores `key-store.sh` writes to. A value moves only through a pipe from
//! the backend's stdout, or is read in-process; it is never in argv, and no backend output other
//! than the value is kept.

use std::io;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Stdio};
use std::time::Duration;

use tokio::process::Command;

use crate::config::{KeyStore, valid_name};
use crate::secret::SecretBytes;

/// The backend executables. Each is run as given, so a bare name is looked up on `PATH`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tools {
    /// macOS `security`.
    pub security: PathBuf,
    /// `secret-tool`, from libsecret.
    pub secret_tool: PathBuf,
    /// `pass`.
    pub pass: PathBuf,
    /// `systemd-creds`.
    pub systemd_creds: PathBuf,
    /// `git`, for the file store's work-tree check.
    pub git: PathBuf,
}

impl Default for Tools {
    fn default() -> Self {
        Self {
            security: "security".into(),
            secret_tool: "secret-tool".into(),
            pass: "pass".into(),
            systemd_creds: "systemd-creds".into(),
            git: "git".into(),
        }
    }
}

/// The parts of the environment the readers consult, passed in rather than read implicitly.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyStoreEnv {
    /// The user's home directory.
    pub home: PathBuf,
    /// `$CREDENTIALS_DIRECTORY`, set by systemd for a service with credentials.
    pub credentials_directory: Option<PathBuf>,
    /// `$PASSWORD_STORE_DIR`, passed on to `pass`.
    pub password_store_dir: Option<PathBuf>,
    /// How long `secret-tool` may take before it is stopped.
    pub timeout: Duration,
    /// Whether the process runs as root.
    pub is_root: bool,
}

/// The default for [`KeyStoreEnv::timeout`], as `MSW_KEY_STORE_TIMEOUT` defaults in
/// `key-store.sh`.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);

impl KeyStoreEnv {
    /// An environment with `home`, no credentials or password-store directory, the default
    /// timeout, and not root.
    pub fn new(home: PathBuf) -> Self {
        Self {
            home,
            credentials_directory: None,
            password_store_dir: None,
            timeout: DEFAULT_TIMEOUT,
            is_root: false,
        }
    }

    /// Reads `HOME`, `CREDENTIALS_DIRECTORY`, `PASSWORD_STORE_DIR` and `MSW_KEY_STORE_TIMEOUT`
    /// from the process environment, and whether the process runs as root.
    pub fn from_process() -> Result<Self, KeyStoreError> {
        let home = std::env::var_os("HOME")
            .filter(|home| !home.is_empty())
            .ok_or(KeyStoreError::NoHome)?;
        let timeout = match std::env::var("MSW_KEY_STORE_TIMEOUT") {
            Ok(secs) => match secs.parse::<u64>() {
                Ok(secs) if secs > 0 => Duration::from_secs(secs),
                _ => return Err(KeyStoreError::InvalidTimeout),
            },
            Err(_) => DEFAULT_TIMEOUT,
        };
        let is_root = process_uid().map_err(KeyStoreError::Uid)? == 0;
        Ok(Self {
            home: home.into(),
            credentials_directory: std::env::var_os("CREDENTIALS_DIRECTORY")
                .filter(|dir| !dir.is_empty())
                .map(PathBuf::from),
            password_store_dir: std::env::var_os("PASSWORD_STORE_DIR")
                .filter(|dir| !dir.is_empty())
                .map(PathBuf::from),
            timeout,
            is_root,
        })
    }

    fn config_dir(&self) -> PathBuf {
        self.home.join(".config").join("mintswarm")
    }
}

/// Why a key could not be read. No variant carries backend output.
#[derive(Debug, thiserror::Error)]
pub enum KeyStoreError {
    /// A service or account name does not match `[A-Za-z0-9][A-Za-z0-9._-]*`.
    #[error("invalid {what}: '{value}'")]
    InvalidName {
        /// `service` or `account`.
        what: &'static str,
        /// The name.
        value: String,
    },
    /// `HOME` is not set.
    #[error("HOME is not set")]
    NoHome,
    /// `MSW_KEY_STORE_TIMEOUT` is not a whole number of seconds above 0.
    #[error("MSW_KEY_STORE_TIMEOUT must be a whole number of seconds above 0")]
    InvalidTimeout,
    /// The process uid could not be found.
    #[error("cannot find the process uid: {0}")]
    Uid(io::Error),
    /// A backend could not be started.
    #[error("cannot run {program}: {source}")]
    Spawn {
        /// The executable.
        program: PathBuf,
        /// The underlying error.
        source: io::Error,
    },
    /// A backend failed.
    #[error("{program} failed ({status})")]
    Failed {
        /// The executable.
        program: PathBuf,
        /// Its exit status.
        status: ExitStatus,
    },
    /// `secret-tool` did not answer in time.
    #[error(
        "secret-tool did not answer within {} seconds: the keyring is probably locked; \
         unlock it first",
        .timeout.as_secs()
    )]
    TimedOut {
        /// The timeout.
        timeout: Duration,
    },
    /// The keychain value is not base64.
    #[error("the keychain value for {service}/{account} is not base64")]
    Decode {
        /// The service name.
        service: String,
        /// The account name.
        account: String,
    },
    /// The store holds an empty value.
    #[error("{service}/{account} in the {store} key store is empty")]
    Empty {
        /// The store.
        store: &'static str,
        /// The service name.
        service: String,
        /// The account name.
        account: String,
    },
    /// The credential or key file does not exist.
    #[error("no {0}")]
    NotFound(String),
    /// `systemd-creds` cannot decrypt for this user.
    #[error(
        "systemd-creds cannot decrypt here: it needs systemd-creds on PATH, and systemd 256 or \
         later unless root"
    )]
    SystemdUnusable,
    /// The file store refused a path.
    #[error("refusing {path}: {reason}")]
    Refused {
        /// The path.
        path: PathBuf,
        /// Why.
        reason: &'static str,
    },
    /// A file could not be read.
    #[error("cannot read {path}: {source}")]
    Io {
        /// The path.
        path: PathBuf,
        /// The underlying error.
        source: io::Error,
    },
}

/// Reads the value kept under `service` and `account` in `store`.
pub async fn read_key(
    store: KeyStore,
    service: &str,
    account: &str,
    tools: &Tools,
    env: &KeyStoreEnv,
) -> Result<SecretBytes, KeyStoreError> {
    for (what, value) in [("service", service), ("account", account)] {
        if !valid_name(value) {
            return Err(KeyStoreError::InvalidName {
                what,
                value: value.to_owned(),
            });
        }
    }
    let value = match store {
        KeyStore::Keychain => keychain(service, account, tools).await?,
        KeyStore::SecretService => secret_service(service, account, tools, env).await?,
        KeyStore::Pass => pass(service, account, tools, env).await?,
        KeyStore::SystemdCreds => systemd_creds(service, account, tools, env).await?,
        KeyStore::File => file(service, account, tools, env).await?,
    };
    if value.is_empty() {
        return Err(KeyStoreError::Empty {
            store: store.name(),
            service: service.to_owned(),
            account: account.to_owned(),
        });
    }
    Ok(SecretBytes::new(value))
}

async fn keychain(service: &str, account: &str, tools: &Tools) -> Result<Vec<u8>, KeyStoreError> {
    let mut command = Command::new(&tools.security);
    command.args(["find-generic-password", "-s", service, "-a", account, "-w"]);
    let encoded = run(command, &tools.security, None).await?;
    decode_base64(&encoded).ok_or_else(|| KeyStoreError::Decode {
        service: service.to_owned(),
        account: account.to_owned(),
    })
}

async fn secret_service(
    service: &str,
    account: &str,
    tools: &Tools,
    env: &KeyStoreEnv,
) -> Result<Vec<u8>, KeyStoreError> {
    let mut command = Command::new(&tools.secret_tool);
    command.args(["lookup", "service", service, "account", account]);
    run(command, &tools.secret_tool, Some(env.timeout)).await
}

async fn pass(
    service: &str,
    account: &str,
    tools: &Tools,
    env: &KeyStoreEnv,
) -> Result<Vec<u8>, KeyStoreError> {
    let mut command = Command::new(&tools.pass);
    command
        .arg("show")
        .arg(format!("mintswarm/{service}/{account}"));
    if let Some(dir) = &env.password_store_dir {
        command.env("PASSWORD_STORE_DIR", dir);
    }
    run(command, &tools.pass, None).await
}

async fn systemd_creds(
    service: &str,
    account: &str,
    tools: &Tools,
    env: &KeyStoreEnv,
) -> Result<Vec<u8>, KeyStoreError> {
    let name = format!("{service}@{account}");
    if let Some(dir) = &env.credentials_directory {
        let passed = dir.join(&name);
        if passed.is_file() {
            return read_file(&passed);
        }
    }
    let file = env
        .config_dir()
        .join("credstore")
        .join(format!("{name}.cred"));
    if !file.is_file() {
        return Err(KeyStoreError::NotFound(format!(
            "credential {name} in $CREDENTIALS_DIRECTORY or {}",
            file.display()
        )));
    }
    if !env.is_root {
        let mut version = Command::new(&tools.systemd_creds);
        version.arg("--version");
        let output = run(version, &tools.systemd_creds, None)
            .await
            .map_err(|_| KeyStoreError::SystemdUnusable)?;
        if systemd_version(&output).is_none_or(|v| v < 256) {
            return Err(KeyStoreError::SystemdUnusable);
        }
    }
    let mut command = Command::new(&tools.systemd_creds);
    if !env.is_root {
        command.arg("--user");
    }
    command
        .arg("decrypt")
        .arg(format!("--name={name}"))
        .arg(&file)
        .arg("-");
    run(command, &tools.systemd_creds, None).await
}

/// The major version on the first line of `systemd-creds --version`: `systemd 256 (256.1)`.
fn systemd_version(output: &[u8]) -> Option<u32> {
    let first = output.split(|&b| b == b'\n').next()?;
    let rest = first.strip_prefix(b"systemd ")?;
    let digits: Vec<u8> = rest
        .iter()
        .copied()
        .take_while(u8::is_ascii_digit)
        .collect();
    std::str::from_utf8(&digits).ok()?.parse().ok()
}

async fn file(
    service: &str,
    account: &str,
    tools: &Tools,
    env: &KeyStoreEnv,
) -> Result<Vec<u8>, KeyStoreError> {
    let config_dir = env.config_dir();
    let dir = config_dir.join("keys");
    let file = dir.join(format!("{service}@{account}"));
    tracing::warn!(
        path = %file.display(),
        "the file key store keeps the key unencrypted"
    );
    for parent in [env.home.join(".config"), config_dir] {
        if parent
            .symlink_metadata()
            .is_ok_and(|m| m.file_type().is_symlink())
        {
            return Err(KeyStoreError::Refused {
                path: parent,
                reason: "it is a symlink (a dotfiles link?); the file key store needs real \
                         directories below $HOME",
            });
        }
    }
    if inside_work_tree(&dir, tools).await {
        return Err(KeyStoreError::Refused {
            path: dir,
            reason: "it is inside a git work tree, where the key could be committed",
        });
    }
    let uid = process_uid().map_err(KeyStoreError::Uid)?;
    check_path(&dir, Kind::Directory, uid)?;
    check_path(&file, Kind::File, uid)?;
    read_file(&file)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Directory,
    File,
}

/// Refuses a symlink, the wrong kind, another owner, or any group or other permission.
fn check_path(path: &Path, kind: Kind, uid: u32) -> Result<(), KeyStoreError> {
    let refuse = |reason| {
        Err(KeyStoreError::Refused {
            path: path.to_owned(),
            reason,
        })
    };
    let meta = match path.symlink_metadata() {
        Ok(meta) => meta,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Err(KeyStoreError::NotFound(path.display().to_string()));
        }
        Err(source) => {
            return Err(KeyStoreError::Io {
                path: path.to_owned(),
                source,
            });
        }
    };
    if meta.file_type().is_symlink() {
        return refuse("it is a symlink");
    }
    match kind {
        Kind::Directory if !meta.is_dir() => return refuse("not a directory"),
        Kind::File if !meta.is_file() => return refuse("not a regular file"),
        _ => {}
    }
    if meta.uid() != uid {
        return refuse("it is not owned by this user");
    }
    if meta.permissions().mode() & 0o077 != 0 {
        return refuse("it has group or other permissions (chmod go= it)");
    }
    Ok(())
}

/// Whether the nearest existing directory at or above `dir` is inside a git work tree. When
/// `git` cannot run, there is no work tree it could commit to.
async fn inside_work_tree(dir: &Path, tools: &Tools) -> bool {
    let Some(existing) = dir.ancestors().find(|p| p.is_dir()) else {
        return false;
    };
    let mut command = Command::new(&tools.git);
    command
        .args(["rev-parse", "--is-inside-work-tree"])
        .current_dir(existing)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_CEILING_DIRECTORIES");
    match run(command, &tools.git, None).await {
        Ok(output) => output.trim_ascii() == b"true",
        Err(_) => false,
    }
}

fn read_file(path: &Path) -> Result<Vec<u8>, KeyStoreError> {
    std::fs::read(path).map_err(|source| KeyStoreError::Io {
        path: path.to_owned(),
        source,
    })
}

/// Runs `command` with stdin null and stdout and stderr piped, and returns its stdout. Stderr is
/// discarded. With a timeout, the child is killed when it expires.
async fn run(
    mut command: Command,
    program: &Path,
    timeout: Option<Duration>,
) -> Result<Vec<u8>, KeyStoreError> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let child = command.spawn().map_err(|source| KeyStoreError::Spawn {
        program: program.to_owned(),
        source,
    })?;
    let output = child.wait_with_output();
    let output = match timeout {
        Some(timeout) => match tokio::time::timeout(timeout, output).await {
            Ok(output) => output,
            Err(_) => return Err(KeyStoreError::TimedOut { timeout }),
        },
        None => output.await,
    }
    .map_err(|source| KeyStoreError::Spawn {
        program: program.to_owned(),
        source,
    })?;
    if !output.status.success() {
        return Err(KeyStoreError::Failed {
            program: program.to_owned(),
            status: output.status,
        });
    }
    Ok(output.stdout)
}

/// The uid of this process: the owner of a new unnamed temporary file.
pub(crate) fn process_uid() -> io::Result<u32> {
    Ok(tempfile::tempfile()?.metadata()?.uid())
}

/// Decodes standard base64 with padding, as `openssl base64 -d -A` does, ignoring surrounding
/// whitespace.
fn decode_base64(text: &[u8]) -> Option<Vec<u8>> {
    let text = text.trim_ascii();
    if text.len() % 4 != 0 {
        return None;
    }
    let mut out = Vec::with_capacity(text.len() / 4 * 3);
    let chunks = text.len() / 4;
    for (index, chunk) in text.chunks(4).enumerate() {
        let padding = chunk.iter().rev().take_while(|&&b| b == b'=').count();
        if padding > 2 || (padding > 0 && index + 1 != chunks) {
            return None;
        }
        let mut bits = 0u32;
        for &byte in &chunk[..4 - padding] {
            bits = bits << 6 | u32::from(sextet(byte)?);
        }
        bits <<= 6 * padding as u32;
        let bytes = bits.to_be_bytes();
        out.extend_from_slice(&bytes[1..4 - padding]);
    }
    Some(out)
}

fn sextet(byte: u8) -> Option<u8> {
    match byte {
        b'A'..=b'Z' => Some(byte - b'A'),
        b'a'..=b'z' => Some(byte - b'a' + 26),
        b'0'..=b'9' => Some(byte - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{decode_base64, systemd_version};

    #[test]
    fn decodes_base64() {
        let cases: [(&str, &[u8]); 8] = [
            ("", b""),
            ("Zg==", b"f"),
            ("Zm8=", b"fo"),
            ("Zm9v", b"foo"),
            ("Zm9vYg==", b"foob"),
            ("Zm9vYmE=", b"fooba"),
            ("Zm9vYmFy\n", b"foobar"),
            ("+/+/", &[0xfb, 0xff, 0xbf]),
        ];
        for (encoded, decoded) in cases {
            assert_eq!(
                decode_base64(encoded.as_bytes()).as_deref(),
                Some(decoded),
                "{encoded}"
            );
        }
    }

    #[test]
    fn refuses_bad_base64() {
        for bad in ["Zg=", "Z===", "Zg==Zm9v", "Zm9v!A==", "Zm 9v"] {
            assert_eq!(decode_base64(bad.as_bytes()), None, "{bad}");
        }
    }

    #[test]
    fn reads_the_systemd_version() {
        assert_eq!(systemd_version(b"systemd 256 (256.1-1)\n+PAM"), Some(256));
        assert_eq!(systemd_version(b"systemd 255\n"), Some(255));
        assert_eq!(systemd_version(b"other 256\n"), None);
        assert_eq!(systemd_version(b""), None);
    }
}
