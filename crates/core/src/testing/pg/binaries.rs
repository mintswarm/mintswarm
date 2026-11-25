//! The binaries backend: `initdb` and `postgres` in a temporary directory, listening only on a
//! Unix socket in that directory.

use std::fs::{self, File};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use super::watchdog::{Cleanup, Watchdog};
use super::{PgHarnessError, Started};

/// Starts a server from `initdb` and `postgres` in `bindir`.
pub(crate) fn start(bindir: &Path) -> Result<Started, PgHarnessError> {
    let dir = instance_dir()?;
    // From here on the watchdog owns the directory: if the start fails, dropping it cleans up.
    let mut watchdog = match Watchdog::spawn(Cleanup::Binaries { dir: dir.clone() }) {
        Ok(watchdog) => watchdog,
        Err(err) => {
            // Best effort: the spawn error is the one worth reporting.
            let _ = fs::remove_dir_all(&dir);
            return Err(err);
        }
    };
    let data = dir.join("data");

    let initdb = bindir.join("initdb");
    let log_path = dir.join("initdb.log");
    let log = File::create(&log_path)?;
    let status = Command::new(&initdb)
        .arg("-D")
        .arg(&data)
        .args(["-A", "trust", "-U", "postgres", "-E", "UTF8", "--locale=C"])
        .args(["--no-sync", "--no-instructions"])
        .stdin(Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log)
        .status()
        .map_err(|source| spawn_error(&initdb, source))?;
    if !status.success() {
        return Err(PgHarnessError::InitFailed {
            log: fs::read_to_string(&log_path).unwrap_or_default(),
        });
    }

    let postgres = bindir.join("postgres");
    let log = File::create(dir.join("postgres.log"))?;
    let server = Command::new(&postgres)
        .arg("-D")
        .arg(&data)
        .arg("-k")
        .arg(&dir)
        .args(["-c", "listen_addresses="])
        .args(["-c", "fsync=off"])
        .args(["-c", "synchronous_commit=off"])
        .args(["-c", "full_page_writes=off"])
        .stdin(Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log)
        .process_group(0)
        .spawn()
        .map_err(|source| spawn_error(&postgres, source))?;
    watchdog.arm(&server.id().to_string())?;

    let mut config = tokio_postgres::Config::new();
    config.host_path(&dir).user("postgres").dbname("postgres");
    Ok(Started {
        config,
        dir: Some(dir),
        server: Some(server),
        container_id: None,
        watchdog,
    })
}

/// Creates the instance directory under `/tmp`, whose short path keeps the socket path within
/// the 103-byte limit on macOS, or under the default temporary directory without `/tmp`.
fn instance_dir() -> Result<PathBuf, PgHarnessError> {
    let mut builder = tempfile::Builder::new();
    builder.prefix("msw-pg-");
    let tmp = Path::new("/tmp");
    let dir = if tmp.is_dir() {
        builder.tempdir_in(tmp)?
    } else {
        builder.tempdir()?
    };
    Ok(dir.keep())
}

fn spawn_error(program: &Path, source: std::io::Error) -> PgHarnessError {
    PgHarnessError::Spawn {
        program: program.display().to_string(),
        source,
    }
}
