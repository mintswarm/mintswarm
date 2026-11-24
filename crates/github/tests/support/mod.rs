//! Shared test support: temporary directories under `CARGO_TARGET_TMPDIR`, fake backend CLIs, a
//! throwaway RSA key, a scripted HTTP responder and a capturing `tracing` subscriber.
#![allow(
    dead_code,
    reason = "each test crate uses a different part of the shared support"
)]

use std::fmt;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::Command;
use std::sync::{Arc, Mutex, OnceLock};

use mintswarm_github::{KeyStoreEnv, Tools};
use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id, Record};
use tracing::{Event, Metadata, Subscriber};

/// The value every fake backend prints.
pub const MARKER: &str = "MARKER-k3y-v4lue-0123456789";
/// [`MARKER`] in base64, as the keychain holds it.
pub const MARKER_BASE64: &str = "TUFSS0VSLWszeS12NGx1ZS0wMTIzNDU2Nzg5";

/// A new temporary directory under the test target's tmp directory.
pub fn temp_dir() -> TempDir {
    tempfile::Builder::new()
        .prefix("mintswarm-github-")
        .tempdir_in(env!("CARGO_TARGET_TMPDIR"))
        .expect("cannot create a temporary directory")
}

/// The argument that makes a fake CLI exit at once, without logging.
const WARM_UP: &str = "--mintswarm-warm-up";

/// Fake backend CLIs in a temporary directory, each appending its argv to `argv.log`.
pub struct Fakes {
    pub dir: TempDir,
    pub tools: Tools,
}

impl Fakes {
    /// Fakes that print the marker; `git` answers `inside_work_tree` to the work-tree check.
    pub fn new(inside_work_tree: bool) -> Self {
        let dir = temp_dir();
        let tools = Tools {
            security: dir.path().join("security"),
            secret_tool: dir.path().join("secret-tool"),
            pass: dir.path().join("pass"),
            systemd_creds: dir.path().join("systemd-creds"),
            git: dir.path().join("git"),
        };
        let fakes = Self { dir, tools };
        fakes.write("security", &format!("printf '%s\\n' '{MARKER_BASE64}'"));
        fakes.write("secret-tool", &format!("printf '%s' '{MARKER}'"));
        fakes.write("pass", &format!("printf '%s' '{MARKER}'"));
        fakes.write(
            "systemd-creds",
            &format!(
                "if [ \"$1\" = --version ]; then echo 'systemd 256 (256.1)'; exit 0; fi\n\
                 printf '%s' '{MARKER}'"
            ),
        );
        fakes.write("git", &format!("echo {inside_work_tree}"));
        fakes
    }

    /// Replaces the fake `name` with a script running `body` after logging its argv, and runs it
    /// once to warm it up.
    ///
    /// The first exec of a freshly written script can stall for seconds under a loaded, parallel
    /// test run (macOS assesses new executables), so no test may time it. The warm-up run exits
    /// before logging, so it is not among [`Fakes::calls`].
    pub fn write(&self, name: &str, body: &str) {
        let path = self.dir.path().join(name);
        let log = self.log_path();
        let script = format!(
            "#!/bin/sh\n[ \"$1\" = {WARM_UP} ] && exit 0\n\
             printf '%s\\n' \"{name} $*\" >> '{}'\n{body}\n",
            log.display()
        );
        std::fs::write(&path, script).expect("cannot write a fake CLI");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
            .expect("cannot make a fake CLI executable");
        let warmed = Command::new(&path)
            .arg(WARM_UP)
            .status()
            .expect("cannot run a fake CLI to warm it up");
        assert!(warmed.success(), "the warm-up of {name} failed: {warmed}");
    }

    pub fn log_path(&self) -> PathBuf {
        self.dir.path().join("argv.log")
    }

    /// Every recorded invocation, one per line: the fake's name, then its arguments.
    pub fn calls(&self) -> Vec<String> {
        match std::fs::read_to_string(self.log_path()) {
            Ok(log) => log.lines().map(str::to_owned).collect(),
            Err(_) => Vec::new(),
        }
    }
}

/// A home directory with `.config/mintswarm`, and an environment rooted at it.
pub struct Home {
    pub dir: TempDir,
    pub env: KeyStoreEnv,
}

impl Home {
    pub fn new() -> Self {
        let dir = temp_dir();
        let config = dir.path().join(".config").join("mintswarm");
        std::fs::create_dir_all(&config).expect("cannot create the config directory");
        let env = KeyStoreEnv::new(dir.path().to_owned());
        Self { dir, env }
    }

    pub fn config_dir(&self) -> PathBuf {
        self.dir.path().join(".config").join("mintswarm")
    }

    /// Writes `value` to the file store as `service@account`, 0600 in a 0700 directory.
    pub fn put_file_key(&self, service: &str, account: &str, value: &[u8]) -> PathBuf {
        let keys = self.config_dir().join("keys");
        std::fs::create_dir_all(&keys).expect("cannot create the keys directory");
        std::fs::set_permissions(&keys, std::fs::Permissions::from_mode(0o700))
            .expect("cannot set the keys directory's mode");
        let file = keys.join(format!("{service}@{account}"));
        std::fs::write(&file, value).expect("cannot write the key file");
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600))
            .expect("cannot set the key file's mode");
        file
    }

    /// Writes `dev-app.json` for a file-store key and returns its path.
    pub fn put_app_config(&self, client_id: &str, service: &str) -> PathBuf {
        let path = self.config_dir().join("dev-app.json");
        let config = serde_json::json!({
            "app_id": 1,
            "slug": "mintswarm-dev",
            "client_id": client_id,
            "keychain_service": service,
            "key_store": "file",
            "key_fingerprint": "SHA256:test",
        });
        std::fs::write(&path, config.to_string()).expect("cannot write the App config");
        path
    }
}

impl Default for Home {
    fn default() -> Self {
        Self::new()
    }
}

/// A throwaway 2048-bit RSA key pair in PEM, generated with `openssl` once per test binary.
pub struct KeyPair {
    pub private_pem: Vec<u8>,
    pub public_pem: Vec<u8>,
}

pub fn key_pair() -> &'static KeyPair {
    static KEY: OnceLock<KeyPair> = OnceLock::new();
    KEY.get_or_init(|| {
        let dir = temp_dir();
        let private = dir.path().join("private.pem");
        let public = dir.path().join("public.pem");
        let run = |args: &[&std::ffi::OsStr]| {
            let status = Command::new("openssl")
                .args(args)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .expect("openssl is needed on PATH to generate the test key, and is missing");
            assert!(status.success(), "openssl {args:?} failed");
        };
        run(&[
            "genrsa".as_ref(),
            "-out".as_ref(),
            private.as_os_str(),
            "2048".as_ref(),
        ]);
        run(&[
            "rsa".as_ref(),
            "-in".as_ref(),
            private.as_os_str(),
            "-pubout".as_ref(),
            "-out".as_ref(),
            public.as_os_str(),
        ]);
        KeyPair {
            private_pem: std::fs::read(&private).expect("cannot read the private key"),
            public_pem: std::fs::read(&public).expect("cannot read the public key"),
        }
    })
}

/// A request the responder received.
#[derive(Clone, Debug)]
pub struct Request {
    pub method: String,
    pub path: String,
    /// Header names in lower case, with their values.
    pub headers: Vec<(String, String)>,
    pub body: String,
}

impl Request {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }
}

/// An HTTP/1.1 responder on a local port that serves one connection at a time, records each
/// request and answers from a script of `(status, body)` replies, one per request.
pub struct Responder {
    pub base: String,
    requests: Arc<Mutex<Vec<Request>>>,
    script: Arc<Mutex<Vec<(u16, String)>>>,
}

impl Responder {
    pub async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("cannot bind the responder");
        let base = format!(
            "http://{}",
            listener.local_addr().expect("the responder has no address")
        );
        let requests = Arc::new(Mutex::new(Vec::new()));
        let script: Arc<Mutex<Vec<(u16, String)>>> = Arc::new(Mutex::new(Vec::new()));
        let (recorded, replies) = (Arc::clone(&requests), Arc::clone(&script));
        tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                let Some(request) = read_request(&mut stream).await else {
                    continue;
                };
                recorded.lock().expect("poisoned").push(request);
                let (status, body) = {
                    let mut replies = replies.lock().expect("poisoned");
                    if replies.is_empty() {
                        (500, String::from("{\"message\":\"no reply scripted\"}"))
                    } else {
                        replies.remove(0)
                    }
                };
                let response = format!(
                    "HTTP/1.1 {status} Scripted\r\nContent-Type: application/json\r\n\
                     Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes()).await;
                let _ = stream.shutdown().await;
            }
        });
        Self {
            base,
            requests,
            script,
        }
    }

    /// Queues a reply.
    pub fn reply(&self, status: u16, body: impl Into<String>) {
        self.script
            .lock()
            .expect("poisoned")
            .push((status, body.into()));
    }

    pub fn requests(&self) -> Vec<Request> {
        self.requests.lock().expect("poisoned").clone()
    }
}

async fn read_request(stream: &mut tokio::net::TcpStream) -> Option<Request> {
    let mut data = Vec::new();
    let mut buffer = [0u8; 4096];
    let head_end = loop {
        if let Some(at) = data.windows(4).position(|w| w == b"\r\n\r\n") {
            break at;
        }
        let read = stream.read(&mut buffer).await.ok()?;
        if read == 0 {
            return None;
        }
        data.extend_from_slice(&buffer[..read]);
    };
    let head = String::from_utf8(data[..head_end].to_vec()).ok()?;
    let mut lines = head.split("\r\n");
    let mut start = lines.next()?.split(' ');
    let method = start.next()?.to_owned();
    let path = start.next()?.to_owned();
    let headers: Vec<(String, String)> = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.trim().to_ascii_lowercase(), value.trim().to_owned()))
        .collect();
    let length: usize = headers
        .iter()
        .find(|(name, _)| name == "content-length")
        .and_then(|(_, value)| value.parse().ok())
        .unwrap_or(0);
    let mut body = data[head_end + 4..].to_vec();
    while body.len() < length {
        let read = stream.read(&mut buffer).await.ok()?;
        if read == 0 {
            break;
        }
        body.extend_from_slice(&buffer[..read]);
    }
    Some(Request {
        method,
        path,
        headers,
        body: String::from_utf8_lossy(&body).into_owned(),
    })
}

/// A `tracing` subscriber that records every span and event, at every level and for every
/// target, as one line of text each.
#[derive(Clone, Default)]
pub struct Capture {
    lines: Arc<Mutex<Vec<String>>>,
    next_id: Arc<Mutex<u64>>,
}

impl Capture {
    pub fn lines(&self) -> Vec<String> {
        self.lines.lock().expect("poisoned").clone()
    }

    fn push(&self, line: String) {
        self.lines.lock().expect("poisoned").push(line);
    }
}

struct Line(String);

impl Visit for Line {
    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        self.0.push_str(&format!(" {}={value:?}", field.name()));
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        self.0.push_str(&format!(" {}={value}", field.name()));
    }
}

impl Subscriber for Capture {
    fn enabled(&self, _: &Metadata<'_>) -> bool {
        true
    }

    fn max_level_hint(&self) -> Option<tracing::level_filters::LevelFilter> {
        Some(tracing::level_filters::LevelFilter::TRACE)
    }

    fn new_span(&self, span: &Attributes<'_>) -> Id {
        let mut line = Line(format!(
            "span {} {}",
            span.metadata().target(),
            span.metadata().name()
        ));
        span.record(&mut line);
        self.push(line.0);
        let mut next = self.next_id.lock().expect("poisoned");
        *next += 1;
        Id::from_u64(*next)
    }

    fn record(&self, _: &Id, values: &Record<'_>) {
        let mut line = Line(String::from("record"));
        values.record(&mut line);
        self.push(line.0);
    }

    fn record_follows_from(&self, _: &Id, _: &Id) {}

    fn event(&self, event: &Event<'_>) {
        let mut line = Line(format!(
            "event {} {}",
            event.metadata().level(),
            event.metadata().target()
        ));
        event.record(&mut line);
        self.push(line.0);
    }

    fn enter(&self, _: &Id) {}

    fn exit(&self, _: &Id) {}
}
