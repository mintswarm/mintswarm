//! Tests of the runner's local store: the runner directory, its lock and opening it.

mod blobs;
mod leases;
mod outbox;
mod schema;
mod unrecognised;

use std::io::{BufRead, BufReader, Read};
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::{Duration, Instant};

use tempfile::TempDir;

use super::{Clock, OutboxMode, Store, StoreError, SystemClock};

/// A clock the test sets by hand.
#[derive(Debug, Default)]
pub(super) struct ManualClock(AtomicI64);

impl ManualClock {
    /// A clock reading `now_ms`.
    pub(super) fn at(now_ms: i64) -> Arc<ManualClock> {
        Arc::new(ManualClock(AtomicI64::new(now_ms)))
    }

    /// Set the time to `now_ms`.
    pub(super) fn set(&self, now_ms: i64) {
        self.0.store(now_ms, Ordering::SeqCst);
    }
}

impl Clock for ManualClock {
    fn now_ms(&self) -> i64 {
        self.0.load(Ordering::SeqCst)
    }
}

/// A fresh runner directory path inside a temporary directory; the directory does not exist.
pub(super) fn fresh_state() -> (TempDir, PathBuf) {
    let tmp = TempDir::new().unwrap();
    let state = tmp.path().join("state");
    (tmp, state)
}

/// How long [`open`] retries a store that reads as locked.
const LOCKED_RETRY: Duration = Duration::from_secs(5);

/// Open the store at `state` with the system clock and `mode`, retrying while it reads as
/// locked.
///
/// Other tests in this binary spawn processes. A child forked while a test holds a store's lock
/// shares the lock until it execs, so a reopen right after a drop can briefly read as locked.
/// The tests of the lock itself use [`open_once`].
pub(super) fn open(state: &Path, mode: OutboxMode) -> Result<Store, StoreError> {
    let deadline = Instant::now() + LOCKED_RETRY;
    loop {
        match open_once(state, mode) {
            Err(StoreError::Locked { .. }) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(10));
            }
            result => return result,
        }
    }
}

/// Open the store at `state` with the system clock and `mode`, once.
pub(super) fn open_once(state: &Path, mode: OutboxMode) -> Result<Store, StoreError> {
    Store::open(state, SystemClock, mode)
}

/// The permission bits of `path`.
pub(super) fn mode_of(path: &Path) -> u32 {
    std::fs::metadata(path).unwrap().permissions().mode() & 0o7777
}

#[test]
fn open_creates_the_layout_with_mode_0700() {
    let (_tmp, state) = fresh_state();
    let store = open(&state, OutboxMode::Suppressed).unwrap();
    assert_eq!(store.path(), state);
    assert_eq!(store.mode(), OutboxMode::Suppressed);
    assert_eq!(mode_of(&state), 0o700);
    for dir in [
        "blobs",
        "blobs/sha256",
        "repos",
        "snapshots",
        "worktrees",
        "tmp",
    ] {
        assert_eq!(mode_of(&state.join(dir)), 0o700, "{dir}");
    }
    assert_eq!(mode_of(&state.join("lock")), 0o600);
    assert!(state.join("runner.sqlite").is_file());
}

#[test]
fn open_refuses_a_runner_directory_with_another_mode() {
    let (_tmp, state) = fresh_state();
    std::fs::DirBuilder::new()
        .mode(0o755)
        .create(&state)
        .unwrap();
    std::fs::set_permissions(&state, std::fs::Permissions::from_mode(0o755)).unwrap();
    let err = open(&state, OutboxMode::Live).unwrap_err();
    assert!(
        matches!(err, StoreError::Mode { mode: 0o755, .. }),
        "{err:?}"
    );
}

#[test]
fn open_refuses_a_file_in_place_of_the_runner_directory() {
    let (_tmp, state) = fresh_state();
    std::fs::write(&state, b"").unwrap();
    let err = open(&state, OutboxMode::Live).unwrap_err();
    assert!(matches!(err, StoreError::NotADirectory { .. }), "{err:?}");
}

#[test]
fn open_refuses_a_symlink_in_place_of_the_runner_directory() {
    let (tmp, state) = fresh_state();
    let target = tmp.path().join("target");
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&target)
        .unwrap();
    std::os::unix::fs::symlink(&target, &state).unwrap();
    let err = open(&state, OutboxMode::Live).unwrap_err();
    assert!(matches!(err, StoreError::NotADirectory { .. }), "{err:?}");
}

#[test]
fn open_creates_the_layout_inside_the_runner_directory_only() {
    let parent = TempDir::new().unwrap();
    let existing = parent.path().join("existing");
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&existing)
        .unwrap();
    let missing = parent.path().join("missing");
    for runner_dir in [&existing, &missing] {
        let store = open_once(runner_dir, OutboxMode::Suppressed).unwrap();
        assert_eq!(mode_of(runner_dir), 0o700, "{}", runner_dir.display());
        for dir in ["blobs", "repos", "snapshots", "worktrees", "tmp"] {
            let path = runner_dir.join(dir);
            assert!(path.is_dir(), "{}", path.display());
            assert_eq!(mode_of(&path), 0o700, "{}", path.display());
        }
        let err = open_once(runner_dir, OutboxMode::Suppressed).unwrap_err();
        assert!(matches!(err, StoreError::Locked { .. }), "{err:?}");
        drop(store);
    }
    let mut names: Vec<_> = std::fs::read_dir(parent.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    names.sort();
    assert_eq!(names, ["existing", "missing"]);
}

#[test]
fn a_database_not_in_write_ahead_logging_is_refused() {
    // An in-memory database has no write-ahead log and stays in journal mode `memory`.
    let err = super::open_database(Path::new(":memory:")).unwrap_err();
    assert!(matches!(err, StoreError::JournalMode(_)), "{err:?}");
}

#[test]
fn open_clears_stale_files_in_tmp() {
    let (_tmp, state) = fresh_state();
    drop(open(&state, OutboxMode::Live).unwrap());
    std::fs::write(state.join("tmp/stale"), b"left over").unwrap();
    std::fs::create_dir(state.join("tmp/stale-dir")).unwrap();
    drop(open(&state, OutboxMode::Live).unwrap());
    assert_eq!(std::fs::read_dir(state.join("tmp")).unwrap().count(), 0);
}

#[test]
fn a_second_open_in_the_same_process_is_locked_out() {
    let (_tmp, state) = fresh_state();
    let first = open(&state, OutboxMode::Live).unwrap();
    let err = open_once(&state, OutboxMode::Live).unwrap_err();
    assert!(matches!(err, StoreError::Locked { .. }), "{err:?}");
    drop(first);
    open(&state, OutboxMode::Live).unwrap();
}

/// The environment variable that turns [`lock_holder_child`] on, naming the runner directory.
const HOLDER_ENV: &str = "MINTSWARM_TEST_STORE_LOCK_HOLDER";

/// The line the child writes once it holds the store.
const HELD_SENTINEL: &str = "mintswarm-test-store-lock-held";

/// Not a test of its own: [`another_process_holding_the_lock_locks_out_open`] runs it in a child
/// process, where it holds the store until its standard input closes. Without [`HOLDER_ENV`] it
/// does nothing.
#[test]
#[ignore = "a helper run by another test in a child process"]
fn lock_holder_child() {
    let Some(state) = std::env::var_os(HOLDER_ENV) else {
        return;
    };
    let _store = open(Path::new(&state), OutboxMode::Live).unwrap();
    // libtest has written the test's name without a newline; start a line of our own.
    println!("\n{HELD_SENTINEL}");
    let mut rest = Vec::new();
    std::io::stdin().read_to_end(&mut rest).unwrap();
}

#[test]
fn another_process_holding_the_lock_locks_out_open() {
    let (_tmp, state) = fresh_state();
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args([
            "exec::store::tests::lock_holder_child",
            "--exact",
            "--ignored",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(HOLDER_ENV, &state)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    let mut held = false;
    let mut line = String::new();
    while stdout.read_line(&mut line).unwrap() > 0 {
        if line.trim_end() == HELD_SENTINEL {
            held = true;
            break;
        }
        line.clear();
    }
    assert!(held, "the child process never held the store");

    let err = open_once(&state, OutboxMode::Live).unwrap_err();
    assert!(matches!(err, StoreError::Locked { .. }), "{err:?}");

    drop(child.stdin.take());
    let status = child.wait().unwrap();
    assert!(status.success(), "the child process failed: {status}");
    open(&state, OutboxMode::Live).unwrap();
}
