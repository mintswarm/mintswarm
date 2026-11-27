//! Short-lived Postgres instances with durability off, for tests.
//!
//! Each test process starts at most one instance, the first time [`database`] or [`instance`]
//! is called, and every [`database`] call creates a database of its own on it. The instance
//! comes from local binaries (`initdb` and `postgres` from `MINTSWARM_PG_BIN`, `PATH` or
//! `pg_config --bindir`) or, failing those, from `docker` or `podman` running an image they
//! already hold (`MINTSWARM_PG_IMAGE`, default `postgres:17`). Nothing is ever downloaded.
//! `fsync`, `synchronous_commit` and `full_page_writes` are off.
//!
//! The binaries backend listens only on a Unix socket in a directory only its owner can open;
//! the container backend listens on a loopback port and requires a password generated for the
//! instance.
//!
//! [`migrated_database`] creates a database and applies a migration tree to it through
//! [`migrations::apply`](crate::migrations::apply), the path production uses.
//!
//! When the test process ends, normally or killed, a watchdog child stops the server and
//! deletes its directory, or removes the container.
//!
//! ```no_run
//! # async fn example() -> Result<(), mintswarm_core::testing::pg::PgHarnessError> {
//! let db = mintswarm_core::testing::pg::database().await?;
//! let (client, connection) = db
//!     .config()
//!     .connect(tokio_postgres::NoTls)
//!     .await
//!     .expect("the test database accepts connections");
//! tokio::spawn(connection);
//! client.execute("CREATE TABLE t (id int)", &[]).await.expect("the table is created");
//! # Ok(())
//! # }
//! ```

mod binaries;
mod container;
mod discovery;
mod error;
mod watchdog;

use std::path::{Path, PathBuf};
use std::process::Child;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use tokio::sync::OnceCell;

use crate::migrations::{self, Outcome};
use tokio_postgres::{Client, Config, NoTls};

pub use discovery::Backend;
pub use error::PgHarnessError;

use container::Container;
use discovery::Environment;
use watchdog::Watchdog;

/// How long the harness waits for a started instance to accept connections.
const READY_TIMEOUT: Duration = Duration::from_secs(60);
/// How long it waits between connection attempts.
const READY_INTERVAL: Duration = Duration::from_millis(100);

static INSTANCE: OnceCell<Instance> = OnceCell::const_new();
static NEXT_DATABASE: AtomicU64 = AtomicU64::new(0);

/// A database created for one caller on the process's instance.
#[derive(Debug, Clone)]
pub struct TestDatabase {
    name: String,
    config: Config,
}

impl TestDatabase {
    /// The database's name, unique in the instance.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The connection settings for this database.
    pub fn config(&self) -> &Config {
        &self.config
    }
}

/// The process's Postgres instance.
#[derive(Debug)]
pub struct Instance {
    backend: Backend,
    config: Config,
    dir: Option<PathBuf>,
    server_pid: Option<u32>,
    container_id: Option<String>,
    // Kept for the life of the process: only its exit closes the watchdog's pipe.
    _watchdog: Watchdog,
}

impl Instance {
    /// Where the instance comes from.
    pub fn backend(&self) -> &Backend {
        &self.backend
    }

    /// The instance's temporary directory, for the binaries backend.
    pub fn dir(&self) -> Option<&Path> {
        self.dir.as_deref()
    }

    /// The server's process id, for the binaries backend.
    pub fn server_pid(&self) -> Option<u32> {
        self.server_pid
    }

    /// The container's id, for the container backend.
    pub fn container_id(&self) -> Option<&str> {
        self.container_id.as_deref()
    }
}

/// What a backend hands back once its server is started, before it is ready.
#[derive(Debug)]
pub(crate) struct Started {
    pub(crate) config: Config,
    pub(crate) dir: Option<PathBuf>,
    pub(crate) server: Option<Child>,
    pub(crate) container: Option<Container>,
    pub(crate) watchdog: Watchdog,
}

/// Creates a new database on the process's instance, starting the instance on the first call.
///
/// Connections are made on the caller's runtime and none is kept by the harness.
pub async fn database() -> Result<TestDatabase, PgHarnessError> {
    let instance = instance().await?;
    let name = format!(
        "msw_test_{}_{}",
        std::process::id(),
        NEXT_DATABASE.fetch_add(1, Ordering::Relaxed)
    );
    let client = connect(&instance.config).await?;
    client
        .batch_execute(&format!("CREATE DATABASE \"{name}\""))
        .await
        .map_err(|source| PgHarnessError::Connect { source })?;
    let mut config = instance.config.clone();
    config.dbname(&name);
    Ok(TestDatabase { name, config })
}

/// Creates a new database as [`database`] does and applies `tree`, given as `(path relative to
/// the tree root, bytes)` pairs, to it through [`migrations::apply`].
pub async fn migrated_database(
    tree: &[(&str, &[u8])],
) -> Result<(TestDatabase, Outcome), PgHarnessError> {
    let db = database().await?;
    let outcome = migrations::apply(db.config(), tree)
        .await
        .map_err(|source| PgHarnessError::Migrate { source })?;
    Ok((db, outcome))
}

/// The process's instance, started on the first call and ready for connections.
pub async fn instance() -> Result<&'static Instance, PgHarnessError> {
    INSTANCE.get_or_try_init(start).await
}

async fn start() -> Result<Instance, PgHarnessError> {
    let (backend, started) = tokio::task::spawn_blocking(|| {
        let backend = discovery::discover(&Environment::from_process())?;
        let started = match &backend {
            Backend::Binaries { bindir } => binaries::start(bindir)?,
            Backend::Container { runtime, image } => container::start(runtime, image)?,
        };
        Ok::<_, PgHarnessError>((backend, started))
    })
    .await
    .map_err(|_| unfinished("the start task did not complete"))??;
    let Started {
        config,
        dir,
        mut server,
        container,
        watchdog,
    } = started;
    wait_ready(&config, dir.as_deref(), server.as_mut(), container.as_ref()).await?;
    Ok(Instance {
        backend,
        config,
        dir,
        server_pid: server.as_ref().map(Child::id),
        container_id: container.map(|container| container.id),
        _watchdog: watchdog,
    })
}

/// Connects and runs `SELECT 1` until it succeeds, the server or container exits, or the time
/// is up.
async fn wait_ready(
    config: &Config,
    dir: Option<&Path>,
    mut server: Option<&mut Child>,
    container: Option<&Container>,
) -> Result<(), PgHarnessError> {
    let begun = Instant::now();
    loop {
        if let Ok(client) = connect(config).await {
            if client.simple_query("SELECT 1").await.is_ok() {
                return Ok(());
            }
        }
        if let Some(server) = server.as_deref_mut() {
            if let Some(status) = server.try_wait()? {
                let log = dir.map(|dir| dir.join("postgres.log")).unwrap_or_default();
                return Err(PgHarnessError::CommandFailed {
                    program: "postgres".to_owned(),
                    status,
                    output: std::fs::read_to_string(log).unwrap_or_default(),
                });
            }
        }
        if let Some(container) = container.cloned() {
            tokio::task::spawn_blocking(move || container.ensure_running())
                .await
                .map_err(|_| unfinished("the container check did not complete"))??;
        }
        if begun.elapsed() >= READY_TIMEOUT {
            return Err(PgHarnessError::NotReady {
                waited: begun.elapsed(),
            });
        }
        tokio::time::sleep(READY_INTERVAL).await;
    }
}

/// The error for a blocking task that did not run to its end.
fn unfinished(what: &'static str) -> PgHarnessError {
    PgHarnessError::Io {
        source: std::io::Error::other(what),
    }
}

/// Connects with `config`, driving the connection on the caller's runtime.
async fn connect(config: &Config) -> Result<Client, PgHarnessError> {
    let (client, connection) = config
        .connect(NoTls)
        .await
        .map_err(|source| PgHarnessError::Connect { source })?;
    tokio::spawn(async move {
        // The connection ends when the client is dropped; its error has no one to go to.
        let _ = connection.await;
    });
    Ok(client)
}
