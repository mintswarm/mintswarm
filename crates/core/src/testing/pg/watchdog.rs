//! The watchdog: a `/bin/sh` child in its own process group that removes the instance once the test
//! process ends.
//!
//! The watchdog reads its stdin from a pipe whose write end only the test process holds. It
//! first reads one line naming the server's pid or the container's name, then waits for end of
//! file, which comes when the test process exits or is killed. It then stops the server or
//! removes the container. Statics never drop, so this is how a per-process instance is removed
//! without `unsafe` code.

use std::io::Write;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};

use super::PgHarnessError;

/// The binaries backend's cleanup: `$1` is the instance directory; the line read is the
/// server's pid, or empty when no server was started. The server leads its own process group,
/// so the whole group (the postmaster and its backends) is signalled, and killed only when it
/// is still there after the wait. `kill -SIG -pgid` without `--` is the form every `sh` takes.
const BINARIES_SCRIPT: &str = r#"
IFS= read -r pid || pid=
case $pid in ''|*[!0-9]*) pid= ;; esac
cat >/dev/null
if [ -n "$pid" ]; then
    kill -QUIT "-$pid" 2>/dev/null
    i=0
    while [ "$i" -lt 100 ] && kill -0 "-$pid" 2>/dev/null; do
        sleep 0.1
        i=$((i + 1))
    done
    if kill -0 "-$pid" 2>/dev/null; then
        kill -KILL "-$pid" 2>/dev/null
    fi
fi
rm -rf "$1"
"#;

/// The container backend's cleanup: `$1` is the runtime; the line read is the container's name,
/// armed before the container is run. Removing a name no container has is harmless.
const CONTAINER_SCRIPT: &str = r#"
IFS= read -r name || name=
cat >/dev/null
if [ -n "$name" ]; then
    "$1" rm -f "$name" >/dev/null 2>&1
fi
"#;

/// The shell the watchdog runs, by absolute path so that cleanup does not depend on `PATH`.
const SHELL: &str = "/bin/sh";

/// What the watchdog removes.
#[derive(Debug, Clone)]
pub(crate) enum Cleanup {
    /// Stop the server whose pid is armed, then delete this directory.
    Binaries {
        /// The instance's temporary directory.
        dir: PathBuf,
    },
    /// Remove the container whose name is armed, with this runtime.
    Container {
        /// The `docker` or `podman` executable.
        runtime: PathBuf,
    },
}

/// A running watchdog. Dropping it closes the pipe and starts the cleanup, so the instance
/// static keeps it for the life of the process.
#[derive(Debug)]
pub(crate) struct Watchdog {
    stdin: ChildStdin,
    // Never waited for: it must outlive the test process.
    _child: Child,
}

impl Watchdog {
    /// Starts the watchdog. Its arguments are positional parameters, never part of the script.
    pub(crate) fn spawn(cleanup: Cleanup) -> Result<Self, PgHarnessError> {
        let mut command = Command::new(SHELL);
        match &cleanup {
            Cleanup::Binaries { dir } => {
                command
                    .args(["-c", BINARIES_SCRIPT, "msw-pg-watchdog"])
                    .arg(dir);
            }
            Cleanup::Container { runtime } => {
                command
                    .args(["-c", CONTAINER_SCRIPT, "msw-pg-watchdog"])
                    .arg(runtime);
            }
        }
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .process_group(0)
            .spawn()
            .map_err(|source| PgHarnessError::Spawn {
                program: SHELL.to_owned(),
                source,
            })?;
        let stdin = child
            .stdin
            .take()
            .expect("stdin was requested as a pipe, so the child has one");
        Ok(Self {
            stdin,
            _child: child,
        })
    }

    /// Names the server pid or container name to remove. Called at most once.
    pub(crate) fn arm(&mut self, target: &str) -> Result<(), PgHarnessError> {
        writeln!(self.stdin, "{target}")?;
        self.stdin.flush()?;
        Ok(())
    }
}
