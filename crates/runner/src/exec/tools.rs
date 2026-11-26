//! The per-attempt tool directory, design §8 "Read tools for stages": `msw-attempt-<id>/` in the
//! system temp directory, mode 0700, holding the `msw-vcs` client and the Unix socket `s`, mode
//! 0600, that a server thread answers on. The directory is the stage's `PATH` entry; the client
//! finds the socket next to its own executable, so no environment variable or argument given to
//! the stage names the runner's state directory or the store.
//!
//! The server is scoped to the job: it runs the same pipeline as `mintswarm-runner tool msw-vcs`
//! ([`crate::tool::msw_vcs`]) over the job's base and head, and passes every request to a
//! [`RequestSink`]. [`ToolDir`] removes the directory when it is closed or dropped, and
//! [`sweep`] removes those a killed runner left behind.

#[cfg(test)]
mod tests;
pub mod wire;

use std::fs::{DirBuilder, OpenOptions, Permissions};
use std::io;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::Duration;

use mintswarm_domain::ids::RevId;
use mintswarm_domain::vcs::Vcs;

use crate::exec::git::GitError;
use crate::tool::msw_vcs::args::{self, Fail};
use crate::tool::msw_vcs::{JobScope, Setup, execute};
use wire::Reply;

/// The prefix of a tool directory's name.
pub const PREFIX: &str = "msw-attempt-";

/// The client's name in the tool directory.
pub const CLIENT: &str = "msw-vcs";

/// The socket's name in the tool directory.
pub const SOCKET: &str = "s";

/// How long the server waits on a client's request or for its reply to be taken.
pub const TIMEOUT: Duration = Duration::from_secs(10);

/// The size of `sun_path`, the platform's limit on a socket path with its final NUL.
#[cfg(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "freebsd",
    target_os = "openbsd",
    target_os = "netbsd",
    target_os = "dragonfly"
))]
const SUN_PATH: usize = 104;
#[cfg(not(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "freebsd",
    target_os = "openbsd",
    target_os = "netbsd",
    target_os = "dragonfly"
)))]
const SUN_PATH: usize = 108;

/// One request the server answered, for the executor to record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RequestRecord {
    /// The attempt whose stage made it.
    pub attempt: String,
    /// Its arguments, as given.
    pub argv: Vec<Vec<u8>>,
    /// The exit code it got.
    pub exit: u8,
}

/// Where the server passes every request, refused and failed ones included.
pub trait RequestSink: Send + Sync {
    /// Record one answered request.
    fn record(&self, request: &RequestRecord);
}

/// What went wrong with a tool directory.
#[derive(Debug, thiserror::Error)]
pub enum ToolError {
    /// An attempt id that cannot name a directory: empty, starting with `.`, or holding `/` or
    /// NUL.
    #[error("the attempt id `{0}` cannot name a tool directory")]
    AttemptId(String),
    /// A socket path longer than the platform allows.
    #[error("the socket path {0} is too long")]
    SocketPath(PathBuf),
    /// A file or socket operation failed. The text is `context` alone.
    #[error("{context}")]
    Io {
        /// What was being done.
        context: String,
        /// The underlying error.
        #[source]
        source: io::Error,
    },
}

fn io_error(context: String) -> impl FnOnce(io::Error) -> ToolError {
    move |source| ToolError::Io { context, source }
}

/// The set-up of the job's scope for a base and a head.
type Rescope = dyn Fn(&RevId, &RevId) -> Result<JobScope, Setup> + Send + Sync;

/// What the server thread shares with its [`ToolDir`].
struct Shared {
    attempt: String,
    /// The job's base and the set-up result for the current head.
    scope: Mutex<(RevId, Result<JobScope, Setup>)>,
    /// Set before the accept is woken, to stop the thread.
    stop: AtomicBool,
}

/// An attempt's tool directory and the server thread answering on its socket. Closing or
/// dropping it stops the thread and removes the directory.
pub struct ToolDir {
    path: PathBuf,
    socket: PathBuf,
    shared: Arc<Shared>,
    rescope: Box<Rescope>,
    thread: Option<JoinHandle<()>>,
}

impl std::fmt::Debug for ToolDir {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolDir")
            .field("path", &self.path)
            .field("attempt", &self.shared.attempt)
            .finish_non_exhaustive()
    }
}

impl ToolDir {
    /// Create `<temp_dir>/msw-attempt-<attempt>` with mode 0700, copy `client` into it as
    /// `msw-vcs` with mode 0755, bind the socket `s` with mode 0600 and serve the job of `base`
    /// and `head` on it. The directory must not exist; a symlink in its place is refused too.
    pub fn create<V>(
        attempt: &str,
        client: &Path,
        vcs: Arc<V>,
        base: RevId,
        head: RevId,
        sink: Arc<dyn RequestSink>,
    ) -> Result<ToolDir, ToolError>
    where
        V: Vcs<Error = GitError> + 'static,
    {
        if attempt.is_empty() || attempt.starts_with('.') || attempt.contains(['/', '\0']) {
            return Err(ToolError::AttemptId(attempt.to_owned()));
        }
        let path = std::env::temp_dir().join(format!("{PREFIX}{attempt}"));
        let socket = path.join(SOCKET);
        if socket.as_os_str().len() >= SUN_PATH {
            return Err(ToolError::SocketPath(socket));
        }
        DirBuilder::new()
            .mode(0o700)
            .create(&path)
            .map_err(io_error(format!("cannot create {}", path.display())))?;
        // From here on, a failure removes what was made.
        let made = Made(Some(path.clone()));
        // The umask may have narrowed the mode, never widened it; set it exactly.
        std::fs::set_permissions(&path, Permissions::from_mode(0o700)).map_err(io_error(
            format!("cannot set the mode of {}", path.display()),
        ))?;
        copy_client(client, &path.join(CLIENT))?;
        let listener = UnixListener::bind(&socket)
            .map_err(io_error(format!("cannot bind {}", socket.display())))?;
        std::fs::set_permissions(&socket, Permissions::from_mode(0o600)).map_err(io_error(
            format!("cannot set the mode of {}", socket.display()),
        ))?;

        let rescope_vcs = Arc::clone(&vcs);
        let rescope: Box<Rescope> =
            Box::new(move |base, head| JobScope::new(&*rescope_vcs, base.clone(), head.clone()));
        let scope = rescope(&base, &head);
        let shared = Arc::new(Shared {
            attempt: attempt.to_owned(),
            scope: Mutex::new((base, scope)),
            stop: AtomicBool::new(false),
        });
        let thread = {
            let shared = Arc::clone(&shared);
            std::thread::Builder::new()
                .name(format!("msw-vcs-{attempt}"))
                .spawn(move || serve(&listener, &shared, &*vcs, &*sink))
                .map_err(io_error("cannot start the msw-vcs server".to_owned()))?
        };
        made.keep();
        Ok(ToolDir {
            path,
            socket,
            shared,
            rescope,
            thread: Some(thread),
        })
    }

    /// The directory, the stage's `PATH` entry.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Move the job's head, after the stage committed, and recompute the merge base.
    pub fn set_head(&self, head: RevId) {
        let mut scope = self
            .shared
            .scope
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let setup = (self.rescope)(&scope.0, &head);
        scope.1 = setup;
    }

    /// Stop the server and remove the directory.
    pub fn close(mut self) -> Result<(), ToolError> {
        self.shutdown()
    }

    fn shutdown(&mut self) -> Result<(), ToolError> {
        if let Some(thread) = self.thread.take() {
            self.shared.stop.store(true, Ordering::SeqCst);
            // Wake the accept; when nothing can connect, leave the thread rather than wait.
            if UnixStream::connect(&self.socket).is_ok() {
                let _ = thread.join();
            }
        }
        match std::fs::remove_dir_all(&self.path) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => Err(ToolError::Io {
                context: format!("cannot remove {}", self.path.display()),
                source: error,
            }),
            _ => Ok(()),
        }
    }
}

impl Drop for ToolDir {
    fn drop(&mut self) {
        if self.thread.is_some()
            && let Err(error) = self.shutdown()
        {
            tracing::warn!(error = %error, "the tool directory was not removed");
        }
    }
}

/// Removes a directory on drop unless kept.
struct Made(Option<PathBuf>);

impl Made {
    fn keep(mut self) {
        self.0 = None;
    }
}

impl Drop for Made {
    fn drop(&mut self) {
        if let Some(path) = &self.0 {
            let _ = std::fs::remove_dir_all(path);
        }
    }
}

/// Copy the client to `to`, a new file, with mode 0755.
fn copy_client(client: &Path, to: &Path) -> Result<(), ToolError> {
    let mut from = std::fs::File::open(client)
        .map_err(io_error(format!("cannot read {}", client.display())))?;
    let context = || format!("cannot write {}", to.display());
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o755)
        .open(to)
        .map_err(io_error(context()))?;
    io::copy(&mut from, &mut file).map_err(io_error(context()))?;
    file.set_permissions(Permissions::from_mode(0o755))
        .map_err(io_error(context()))
}

/// The server loop: one connection at a time, until stopped.
fn serve<V>(listener: &UnixListener, shared: &Shared, vcs: &V, sink: &dyn RequestSink)
where
    V: Vcs<Error = GitError>,
{
    for stream in listener.incoming() {
        if shared.stop.load(Ordering::SeqCst) {
            return;
        }
        let Ok(mut stream) = stream else {
            continue;
        };
        if stream.set_read_timeout(Some(TIMEOUT)).is_err()
            || stream.set_write_timeout(Some(TIMEOUT)).is_err()
        {
            continue;
        }
        // A client that stalls, disconnects or is killed mid-frame sent no request.
        let Ok(argv) = wire::read_request(&mut stream) else {
            continue;
        };
        let reply = answer(&argv, shared, vcs);
        sink.record(&RequestRecord {
            attempt: shared.attempt.clone(),
            argv,
            exit: reply.exit,
        });
        let _ = wire::write_reply(&mut stream, &reply);
    }
}

/// The reply to one request: the arguments, then the set-up, then the revisions and paths.
fn answer<V>(argv: &[Vec<u8>], shared: &Shared, vcs: &V) -> Reply
where
    V: Vcs<Error = GitError>,
{
    let result = args::parse(argv).and_then(|request| {
        let scope = shared
            .scope
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .1
            .clone()
            .map_err(|setup| setup.fail())?;
        execute(&request, &scope, vcs)
    });
    match result {
        Ok(stdout) => Reply {
            exit: 0,
            stdout,
            stderr: Vec::new(),
        },
        Err(fail) => failed(&fail),
    }
}

fn failed(fail: &Fail) -> Reply {
    Reply {
        exit: fail.exit,
        stdout: Vec::new(),
        stderr: fail.line(),
    }
}

/// Remove the tool directories in `temp_dir` that a killed runner left: every `msw-attempt-*`
/// entry that is a directory, never a symlink, ignoring those that cannot be removed. Only the
/// runner's start, before any attempt, may call it, since it also removes a live attempt's
/// directory; that is `exec-supervisor`'s start-up recovery. Returns how many were removed.
pub fn sweep(temp_dir: &Path) -> usize {
    let Ok(entries) = std::fs::read_dir(temp_dir) else {
        return 0;
    };
    entries
        .flatten()
        .filter(|entry| {
            entry
                .file_name()
                .as_encoded_bytes()
                .starts_with(PREFIX.as_bytes())
        })
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .filter(|entry| std::fs::remove_dir_all(entry.path()).is_ok())
        .count()
}
