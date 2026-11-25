//! The runner's local state, design §12: a locked state directory, the SQLite database with the
//! tables of §12, the content-addressed blob store, the outbox and leases.
//!
//! [`Store::open`] takes an exclusive lock on the state directory and holds it for the store's
//! lifetime, so one runner process at a time owns it. The lock is an advisory `flock`, which
//! some network filesystems do not honour: the state directory must be on a local filesystem.
//!
//! The store is synchronous and owns one SQLite connection. Every time it records is integer
//! unix milliseconds from the [`Clock`] it was opened with.

mod blobs;
mod leases;
mod outbox;
mod schema;
#[cfg(test)]
mod tests;

use std::ffi::OsStr;
use std::fs::{DirBuilder, File, OpenOptions, Permissions};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use mintswarm_domain::ids::RunnerId;
use rusqlite::{Connection, TransactionBehavior};
use rustix::fs::FlockOperation;
use rustix::io::Errno;

pub use blobs::BlobId;
pub use leases::{Lease, LeaseResource};
pub use outbox::{Effect, EffectKind, EffectState, NewEffect};
pub use schema::SCHEMA_VERSION;

/// The mode of the state directory and of every directory the store creates in it.
const DIR_MODE: u32 = 0o700;

/// The mode of the lock file.
const LOCK_MODE: u32 = 0o600;

/// How long a statement waits for a locked database before it fails.
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

/// The directories the store keeps under the state directory, parents first.
const SUBDIRS: &[&str] = &[
    "blobs",
    "blobs/sha256",
    "repos",
    "snapshots",
    "worktrees",
    "tmp",
];

/// What went wrong in the runner's local store.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    /// A file operation failed. The text is `context` alone; the underlying error is only the
    /// `source()`.
    #[error("{context}")]
    Io {
        /// What the store was doing.
        context: String,
        /// The underlying error.
        #[source]
        source: std::io::Error,
    },
    /// A database operation failed. The text is `context` alone; the underlying error is only
    /// the `source()`.
    #[error("{context}")]
    Sqlite {
        /// What the store was doing.
        context: String,
        /// The underlying error.
        #[source]
        source: rusqlite::Error,
    },
    /// Another open store, in this process or another, holds the state directory's lock.
    #[error("the state directory {path} is locked by another runner")]
    Locked {
        /// The state directory.
        path: PathBuf,
    },
    /// A runner id that cannot name a state directory.
    #[error("`{0}` cannot name a state directory")]
    RunnerId(String),
    /// Neither `XDG_STATE_HOME` nor `HOME` is an absolute path.
    #[error("neither XDG_STATE_HOME nor HOME is set to an absolute path")]
    NoStateHome,
    /// The state directory exists with a mode other than 0700.
    #[error("the state directory {path} has mode {mode:o}, not 700")]
    Mode {
        /// The state directory.
        path: PathBuf,
        /// Its permission bits.
        mode: u32,
    },
    /// Something other than a directory, such as a file or a symbolic link, is at the state
    /// directory's path.
    #[error("the state directory {path} is not a directory")]
    NotADirectory {
        /// The state directory.
        path: PathBuf,
    },
    /// The database was migrated by a newer runner than this one.
    #[error("the database has schema version {found}, newer than the known {known}")]
    SchemaTooNew {
        /// The database's `user_version`.
        found: i64,
        /// The newest version this runner knows, [`SCHEMA_VERSION`].
        known: i64,
    },
    /// The database's `user_version` is negative, which no runner writes.
    #[error("the database has schema version {0}, which no runner writes")]
    SchemaInvalid(i64),
    /// The database stays in another journal mode when write-ahead logging is asked for, for
    /// example on a filesystem without shared memory.
    #[error("the database uses journal mode `{0}`, not write-ahead logging")]
    JournalMode(String),
    /// Text that is not a blob id: 64 lowercase hexadecimal digits.
    #[error("`{0}` is not a sha256 blob id")]
    BlobId(String),
    /// No blob of that id is stored.
    #[error("blob {0} is not in the store")]
    BlobMissing(BlobId),
    /// The stored content no longer hashes to the blob's id.
    #[error("blob {0} does not match its id")]
    BlobCorrupt(BlobId),
    /// A live lease on the resource is held already.
    #[error("the lease on {0} is held")]
    LeaseHeld(String),
    /// The caller does not hold a live lease on the resource.
    #[error("the lease on {0} is not held by the caller")]
    LeaseNotHeld(String),
    /// A lease's time to live that is not a positive number of milliseconds.
    #[error("a lease cannot last {0} ms")]
    LeaseTtl(i64),
    /// A stored value that the store does not know, such as an unknown outbox state.
    #[error("unknown value `{value}` in {column}")]
    Value {
        /// The table and column, as `table.column`.
        column: &'static str,
        /// The value found.
        value: String,
    },
}

/// The source of the current time, in unix milliseconds.
pub trait Clock: Send + Sync {
    /// The current time, in milliseconds since the unix epoch.
    fn now_ms(&self) -> i64;
}

/// The system's wall clock.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now_ms(&self) -> i64 {
        // A clock before the epoch reads as the epoch; one past i64 milliseconds saturates.
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |since| {
                i64::try_from(since.as_millis()).unwrap_or(i64::MAX)
            })
    }
}

impl<C: Clock + ?Sized> Clock for Arc<C> {
    fn now_ms(&self) -> i64 {
        (**self).now_ms()
    }
}

/// What the outbox does with an inserted effect, fixed when the store is opened.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutboxMode {
    /// Effects are stored as `pending`, for a sender.
    Live,
    /// Effects are stored as `suppressed` and never reach a sender (M1 and shadow mode).
    Suppressed,
}

/// The state directory of `runner_id`: `${XDG_STATE_HOME:-$HOME/.local/state}/mintswarm/<id>`.
///
/// An empty or relative `XDG_STATE_HOME` is ignored, as the XDG specification says, and so is
/// an empty or relative `HOME`. The runner id must be one path component that does not start
/// with `.`.
pub fn state_dir(
    runner_id: &RunnerId,
    xdg_state_home: Option<&OsStr>,
    home: Option<&OsStr>,
) -> Result<PathBuf, StoreError> {
    let id = runner_id.0.as_str();
    if id.is_empty() || id.starts_with('.') || id.contains(['/', '\0']) {
        return Err(StoreError::RunnerId(id.to_owned()));
    }
    let base = match (absolute(xdg_state_home), absolute(home)) {
        (Some(xdg), _) => xdg.to_path_buf(),
        (None, Some(home)) => home.join(".local").join("state"),
        (None, None) => return Err(StoreError::NoStateHome),
    };
    Ok(base.join("mintswarm").join(id))
}

/// `value` as a path when it is an absolute one.
fn absolute(value: Option<&OsStr>) -> Option<&Path> {
    value.map(Path::new).filter(|path| path.is_absolute())
}

/// The state directory of `runner_id`, from `XDG_STATE_HOME` and `HOME` in the environment.
pub fn default_state_dir(runner_id: &RunnerId) -> Result<PathBuf, StoreError> {
    state_dir(
        runner_id,
        std::env::var_os("XDG_STATE_HOME").as_deref(),
        std::env::var_os("HOME").as_deref(),
    )
}

/// The parts of an open store that a transaction uses besides the connection.
struct Context {
    /// The state directory.
    root: PathBuf,
    /// What the outbox does with an inserted effect.
    mode: OutboxMode,
    /// The source of the current time.
    clock: Box<dyn Clock>,
}

/// The runner's local store over one locked state directory.
pub struct Store {
    /// The database, `<state>/runner.sqlite`.
    conn: Connection,
    /// The state directory, the outbox mode and the clock.
    ctx: Context,
    /// The open lock file; its `flock` is released when it is closed.
    _lock: File,
}

impl std::fmt::Debug for Store {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Store")
            .field("root", &self.ctx.root)
            .field("mode", &self.ctx.mode)
            .finish_non_exhaustive()
    }
}

impl Store {
    /// Open the state directory at `state_dir`, creating it with mode 0700 when it does not
    /// exist, lock it, and migrate its database.
    ///
    /// An existing directory with another mode is refused, and so is one whose lock another
    /// open store holds.
    pub fn open(
        state_dir: &Path,
        clock: impl Clock + 'static,
        mode: OutboxMode,
    ) -> Result<Store, StoreError> {
        let root = std::path::absolute(state_dir).map_err(io(state_dir, "cannot resolve"))?;
        prepare_root(&root)?;
        let lock = take_lock(&root)?;
        for dir in SUBDIRS {
            ensure_dir(&root.join(dir))?;
        }
        clear_tmp(&root.join("tmp"))?;
        let mut conn = open_database(&root.join("runner.sqlite"))?;
        schema::migrate(&mut conn)?;
        Ok(Store {
            conn,
            ctx: Context {
                root,
                mode,
                clock: Box::new(clock),
            },
            _lock: lock,
        })
    }

    /// The state directory.
    pub fn path(&self) -> &Path {
        &self.ctx.root
    }

    /// What the outbox does with an inserted effect.
    pub fn mode(&self) -> OutboxMode {
        self.ctx.mode
    }

    /// Run `f` in one database transaction, committed when `f` succeeds and rolled back when
    /// it fails.
    pub fn transaction<T>(
        &mut self,
        f: impl FnOnce(&StoreTx<'_>) -> Result<T, StoreError>,
    ) -> Result<T, StoreError> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sql("cannot begin a transaction"))?;
        let store_tx = StoreTx { tx, ctx: &self.ctx };
        let value = f(&store_tx)?;
        store_tx
            .tx
            .commit()
            .map_err(sql("cannot commit a transaction"))?;
        Ok(value)
    }

    /// Store `content` as a blob and return its id.
    pub fn put_blob(&self, content: &[u8]) -> Result<BlobId, StoreError> {
        blobs::put(&self.ctx.root, content)
    }

    /// The content of blob `id`, re-hashed: a blob whose content does not match its id is
    /// refused with [`StoreError::BlobCorrupt`].
    pub fn get_blob(&self, id: &BlobId) -> Result<Vec<u8>, StoreError> {
        blobs::get(&self.ctx.root, id)
    }

    /// Insert `effect` into the outbox in its own transaction; see [`StoreTx::insert_effect`].
    pub fn insert_effect(&mut self, effect: &NewEffect) -> Result<i64, StoreError> {
        self.transaction(|tx| tx.insert_effect(effect))
    }

    /// The outbox row `effect_id`, whatever its state, for inspection; senders read
    /// [`Store::due`].
    pub fn effect(&self, effect_id: i64) -> Result<Option<Effect>, StoreError> {
        outbox::effect(&self.conn, effect_id)
    }

    /// The outbox rows a sender may send at `now_ms`, at most `limit`, in order per job.
    ///
    /// This is the only read of the outbox meant for a sender. It returns only `pending` and
    /// `sent` rows whose `next_at` is unset or not later than `now_ms`, so a `suppressed` row
    /// never reaches a sender.
    pub fn due(&self, now_ms: i64, limit: u32) -> Result<Vec<Effect>, StoreError> {
        outbox::due(&self.conn, now_ms, limit)
    }

    /// Take the lease on `resource` for `holder`, for `ttl_ms` from now.
    ///
    /// It succeeds only when no live lease on the resource exists, even one `holder` holds;
    /// a holder extends its lease with [`Store::renew`].
    pub fn acquire(
        &self,
        resource: &LeaseResource,
        holder: &str,
        ttl_ms: i64,
    ) -> Result<Lease, StoreError> {
        leases::acquire(
            &self.conn,
            self.ctx.clock.now_ms(),
            resource,
            holder,
            ttl_ms,
        )
    }

    /// Extend `holder`'s live lease on `resource` to `ttl_ms` from now.
    ///
    /// A lease is live while now is before its `expires_at`; from then on it cannot be
    /// renewed, even by its holder.
    pub fn renew(
        &self,
        resource: &LeaseResource,
        holder: &str,
        ttl_ms: i64,
    ) -> Result<Lease, StoreError> {
        leases::renew(
            &self.conn,
            self.ctx.clock.now_ms(),
            resource,
            holder,
            ttl_ms,
        )
    }

    /// Release `holder`'s lease on `resource`, live or expired.
    pub fn release(&self, resource: &LeaseResource, holder: &str) -> Result<(), StoreError> {
        leases::release(&self.conn, resource, holder)
    }

    /// The live lease on `resource`, if there is one.
    pub fn lease(&self, resource: &LeaseResource) -> Result<Option<Lease>, StoreError> {
        leases::lease(&self.conn, self.ctx.clock.now_ms(), resource)
    }
}

/// One database transaction of a [`Store`], from [`Store::transaction`].
pub struct StoreTx<'s> {
    /// The open transaction.
    tx: rusqlite::Transaction<'s>,
    /// The store's state directory, outbox mode and clock.
    ctx: &'s Context,
}

impl StoreTx<'_> {
    /// The connection inside the transaction, for statements of the store's tables.
    pub fn connection(&self) -> &Connection {
        &self.tx
    }

    /// The current time of the store's clock, in unix milliseconds.
    pub fn now_ms(&self) -> i64 {
        self.ctx.clock.now_ms()
    }

    /// Store `content` as a blob and return its id. The blob is written at once, outside the
    /// transaction; a rolled-back transaction leaves it unreferenced.
    pub fn put_blob(&self, content: &[u8]) -> Result<BlobId, StoreError> {
        blobs::put(&self.ctx.root, content)
    }

    /// Insert `effect` into the outbox, with its payload as a blob, and return its
    /// `effect_id`.
    ///
    /// The row is `pending` when the store is [`OutboxMode::Live`] and `suppressed` when it is
    /// [`OutboxMode::Suppressed`]. An effect whose `(job_id, effect_key)` is stored already is
    /// not inserted again: the existing row's id is returned and nothing changes.
    pub fn insert_effect(&self, effect: &NewEffect) -> Result<i64, StoreError> {
        outbox::insert(&self.tx, &self.ctx.root, self.ctx.mode, effect)
    }
}

/// Map an IO error to [`StoreError::Io`] with `what` and `path` as the context.
fn io(path: &Path, what: &str) -> impl FnOnce(std::io::Error) -> StoreError {
    let context = format!("{what} {}", path.display());
    move |source| StoreError::Io { context, source }
}

/// Map a database error to [`StoreError::Sqlite`] with `context`.
fn sql(context: &str) -> impl FnOnce(rusqlite::Error) -> StoreError {
    let context = context.to_owned();
    move |source| StoreError::Sqlite { context, source }
}

/// Create the state directory with mode 0700, or check the mode of an existing one.
fn prepare_root(root: &Path) -> Result<(), StoreError> {
    match std::fs::symlink_metadata(root) {
        Ok(meta) => {
            let mode = meta.permissions().mode();
            if !meta.is_dir() {
                return Err(StoreError::NotADirectory {
                    path: root.to_path_buf(),
                });
            }
            if mode & 0o7777 != DIR_MODE {
                return Err(StoreError::Mode {
                    path: root.to_path_buf(),
                    mode: mode & 0o7777,
                });
            }
            Ok(())
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            if let Some(parent) = root.parent() {
                DirBuilder::new()
                    .recursive(true)
                    .mode(DIR_MODE)
                    .create(parent)
                    .map_err(io(parent, "cannot create"))?;
            }
            ensure_dir(root)
        }
        Err(err) => Err(io(root, "cannot read")(err)),
    }
}

/// Create `dir` when it does not exist, and set its mode to exactly 0700.
fn ensure_dir(dir: &Path) -> Result<(), StoreError> {
    match DirBuilder::new().mode(DIR_MODE).create(dir) {
        Ok(()) => {}
        Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(err) => return Err(io(dir, "cannot create")(err)),
    }
    // The umask may have narrowed the mode, never widened it; set it exactly.
    std::fs::set_permissions(dir, Permissions::from_mode(DIR_MODE))
        .map_err(io(dir, "cannot set the mode of"))
}

/// Open `<root>/lock` and take an exclusive, non-blocking `flock` on it.
fn take_lock(root: &Path) -> Result<File, StoreError> {
    let path = root.join("lock");
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(LOCK_MODE)
        .open(&path)
        .map_err(io(&path, "cannot open the lock file"))?;
    match rustix::fs::flock(&file, FlockOperation::NonBlockingLockExclusive) {
        Ok(()) => Ok(file),
        Err(errno) if errno == Errno::WOULDBLOCK => Err(StoreError::Locked {
            path: root.to_path_buf(),
        }),
        Err(errno) => Err(io(&path, "cannot lock")(errno.into())),
    }
}

/// Remove whatever an earlier run left in `tmp`; nothing else uses it while the lock is held.
fn clear_tmp(tmp: &Path) -> Result<(), StoreError> {
    let entries = std::fs::read_dir(tmp).map_err(io(tmp, "cannot list"))?;
    for entry in entries {
        let entry = entry.map_err(io(tmp, "cannot list"))?;
        let path = entry.path();
        let file_type = entry.file_type().map_err(io(&path, "cannot read"))?;
        if file_type.is_dir() {
            std::fs::remove_dir_all(&path).map_err(io(&path, "cannot remove"))?;
        } else {
            std::fs::remove_file(&path).map_err(io(&path, "cannot remove"))?;
        }
    }
    Ok(())
}

/// Open the database with foreign keys on, write-ahead logging and a busy timeout.
///
/// A database that stays in another journal mode is refused with [`StoreError::JournalMode`].
fn open_database(path: &Path) -> Result<Connection, StoreError> {
    let conn = Connection::open(path).map_err(sql("cannot open the database"))?;
    conn.pragma_update(None, "foreign_keys", true)
        .map_err(sql("cannot enable foreign keys"))?;
    let journal_mode = conn
        .query_row("PRAGMA journal_mode = WAL", [], |row| {
            row.get::<_, String>(0)
        })
        .map_err(sql("cannot set the journal mode"))?;
    if !journal_mode.eq_ignore_ascii_case("wal") {
        return Err(StoreError::JournalMode(journal_mode));
    }
    conn.busy_timeout(BUSY_TIMEOUT)
        .map_err(sql("cannot set the busy timeout"))?;
    Ok(conn)
}
