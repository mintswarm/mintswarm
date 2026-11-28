//! The control plane's start: the leader lock, the readiness it reports, the migrations and the
//! reconciliation of `projects:` from `mintswarm.yaml`.
//!
//! One instance leads at a time. It holds [`LEADER_LOCK_KEY`], a Postgres session advisory lock,
//! on a dedicated connection; a second instance against the same database is a standby that
//! polls for the lock and reports itself not ready until the leader's session ends. The lock is
//! taken by polling `pg_try_advisory_lock`, so a standby holds no waiting statement and can tell
//! a held lock from a database it cannot reach.
//!
//! [`start`] takes the lock, applies the embedded migrations, reconciles the projects and starts
//! the readiness probe, and gives the [`Leader`], whose [`lost`](Leader::lost) resolves when the
//! lock's session ends.

use std::future::Future;
use std::time::Duration;

use deadpool_postgres::{Manager, Object, Pool, Runtime};
use mintswarm_config::{DatabaseSettings, ProjectEntry};
use mintswarm_domain::ids::ProjectId;
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio_postgres::types::ToSql;
use tokio_postgres::{Client, Config, NoTls, Row};

use crate::migrations::{self, ApplyError, Outcome, plan_database, validate};
use crate::store::{PgStore, Store, StoreError, Transaction, Versioned};

/// The session advisory lock key the leader holds: the ASCII bytes `mswmlead`, apart from
/// [`MIGRATION_LOCK_KEY`](crate::migrations::MIGRATION_LOCK_KEY).
pub const LEADER_LOCK_KEY: i64 = 0x6d73_776d_6c65_6164;

/// How long the leader lock's connection may be idle before TCP keepalives probe it.
const KEEPALIVE_IDLE: Duration = Duration::from_secs(10);

/// How many lock intervals one session check may take before the leader gives up its lead: a
/// slow answer is waited for, and only a check that has not answered by then counts as a loss.
const CHECK_INTERVALS: u32 = 3;

/// The bound on each readiness check.
const PROBE_TIMEOUT: Duration = Duration::from_secs(2);

/// The most connections the plane's pool holds.
const POOL_SIZE: usize = 4;

/// How often the plane polls and checks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Intervals {
    /// How often a standby tries for the lock, and the leader checks its session: 1 second by
    /// default.
    pub lock: Duration,
    /// How often the readiness probe runs: 2 seconds by default.
    pub probe: Duration,
}

impl Default for Intervals {
    fn default() -> Self {
        Self {
            lock: Duration::from_secs(1),
            probe: Duration::from_secs(2),
        }
    }
}

/// Why `database` settings do not give a connection config. It names neither the URL nor the
/// password, and keeps no source, whose text might quote them.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("database.url is not a Postgres connection string; its value is not shown")]
pub struct DatabaseConfigError;

/// The connection config of `database`: its URL, with the password replaced by
/// `database.password` when that is given. This is the one conversion from the settings.
pub fn database_config(settings: &DatabaseSettings) -> Result<Config, DatabaseConfigError> {
    let mut config: Config = settings
        .url
        .expose()
        .parse()
        .map_err(|_| DatabaseConfigError)?;
    if let Some(password) = &settings.password {
        config.password(password.expose());
    }
    Ok(config)
}

/// What `/readyz` reports. The plane is ready when all three hold.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Readiness {
    /// This instance holds the leader lock.
    pub leader: bool,
    /// The database answers.
    pub database: bool,
    /// Every embedded migration is applied.
    pub migrations: bool,
}

impl Readiness {
    /// Whether every check holds.
    pub fn is_ready(&self) -> bool {
        self.leader && self.database && self.migrations
    }

    /// The names of the checks that fail, in the order `leader`, `database`, `migrations`.
    pub fn failing(&self) -> Vec<&'static str> {
        [
            ("leader", self.leader),
            ("database", self.database),
            ("migrations", self.migrations),
        ]
        .into_iter()
        .filter(|(_, holds)| !holds)
        .map(|(name, _)| name)
        .collect()
    }
}

/// A project as the `projects` table keeps it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Project {
    /// The key.
    pub project_id: ProjectId,
    /// The repository as `owner/name`.
    pub repo: String,
    /// The code host, such as `github`.
    pub host: String,
    /// The GitHub App installation.
    pub installation_id: i64,
    /// The project's share of the work.
    pub weight: i32,
}

impl From<&ProjectEntry> for Project {
    fn from(entry: &ProjectEntry) -> Self {
        Self {
            project_id: entry.project_id.clone(),
            repo: entry.repo.clone(),
            host: entry.host.name().to_owned(),
            installation_id: entry.installation_id,
            weight: entry.weight,
        }
    }
}

impl Versioned for Project {
    const TABLE: &'static str = "projects";
    const KEY_COLUMN: &'static str = "project_id";
    const COLUMNS: &'static [&'static str] = &["repo", "host", "installation_id", "weight"];

    type Key = String;

    fn key(&self) -> String {
        self.project_id.0.clone()
    }

    fn values(&self) -> Vec<&(dyn ToSql + Sync)> {
        vec![&self.repo, &self.host, &self.installation_id, &self.weight]
    }

    fn from_row(row: &Row) -> Result<Self, tokio_postgres::Error> {
        Ok(Self {
            project_id: ProjectId(row.try_get("project_id")?),
            repo: row.try_get("repo")?,
            host: row.try_get("host")?,
            installation_id: row.try_get("installation_id")?,
            weight: row.try_get("weight")?,
        })
    }
}

/// What a reconciliation changed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Reconciled {
    /// The projects that had no row.
    pub inserted: usize,
    /// The rows whose repository, host, installation or weight changed.
    pub updated: usize,
}

/// Makes the `projects` rows match `entries`, in one transaction: a missing row is inserted,
/// and a row that differs is updated. Rows of projects that `entries` does not name are left as
/// they are.
pub async fn reconcile<S: Store>(
    store: &S,
    entries: &[ProjectEntry],
) -> Result<Reconciled, StoreError> {
    let mut transaction = store.begin().await?;
    let mut reconciled = Reconciled::default();
    for entry in entries {
        let project = Project::from(entry);
        match transaction.get::<Project>(&project.key()).await? {
            None => {
                transaction.insert(&project).await?;
                reconciled.inserted += 1;
            }
            Some(stored) if stored.record != project => {
                transaction.update(&project, stored.version).await?;
                reconciled.updated += 1;
            }
            Some(_) => {}
        }
    }
    transaction.commit().await?;
    Ok(reconciled)
}

/// The held leader lock. Dropping it ends the lock's session, which releases the lock, and
/// stops the tasks [`start`] began.
#[derive(Debug)]
pub struct Leader {
    backend_pid: i32,
    lost: watch::Receiver<bool>,
    tasks: Vec<JoinHandle<()>>,
}

impl Drop for Leader {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
    }
}

impl Leader {
    /// Polls for the leader lock every `interval` until this instance holds it, publishing to
    /// `readiness` whether the database answers and, while it waits, that it is not the leader.
    ///
    /// The lock is held on a dedicated connection with TCP keepalives on, apart from any pool.
    /// A connection that fails or is lost while waiting is made again at the next interval, so
    /// this never fails; it ends only once the lock is held. Once held, the session is checked
    /// with `SELECT 1` every `interval`. The lock counts as lost when its connection ends, a
    /// check fails, or a check has not answered within three intervals; a slower answer that
    /// comes in time keeps the lead.
    pub async fn acquire(
        config: &Config,
        interval: Duration,
        readiness: &watch::Sender<Readiness>,
    ) -> Leader {
        let mut config = config.clone();
        config
            .keepalives(true)
            .keepalives_idle(KEEPALIVE_IDLE)
            .connect_timeout(interval.max(PROBE_TIMEOUT));
        let publish = |database: bool, leader: bool| {
            readiness.send_modify(|state| {
                state.database = database;
                state.leader = leader;
            });
        };
        loop {
            match try_lock(&config, interval).await {
                Attempt::Held(client, closed) => {
                    let Ok(Ok(row)) = tokio::time::timeout(
                        interval,
                        client.query_one("SELECT pg_backend_pid()", &[]),
                    )
                    .await
                    else {
                        // The session went before it could be named; the lock goes with it.
                        publish(false, false);
                        tokio::time::sleep(interval).await;
                        continue;
                    };
                    let backend_pid: i32 = row.get(0);
                    publish(true, true);
                    let (lost_tx, lost) = watch::channel(false);
                    let heartbeat = tokio::spawn(heartbeat(
                        client,
                        closed,
                        interval,
                        lost_tx,
                        readiness.clone(),
                    ));
                    return Leader {
                        backend_pid,
                        lost,
                        tasks: vec![heartbeat],
                    };
                }
                Attempt::Taken => publish(true, false),
                Attempt::Unreachable => publish(false, false),
            }
            tokio::time::sleep(interval).await;
        }
    }

    /// The process id of the backend whose session holds the lock.
    pub fn backend_pid(&self) -> i32 {
        self.backend_pid
    }

    /// Resolves once the lock's session has ended or stopped answering.
    pub async fn lost(&mut self) {
        // An error means the heartbeat task is gone, which only happens once it has reported.
        let _ = self.lost.wait_for(|lost| *lost).await;
    }
}

/// The result of one try for the leader lock.
enum Attempt {
    /// The lock is held on this client; `closed` resolves when its connection ends.
    Held(Client, tokio::sync::oneshot::Receiver<()>),
    /// The database answered, and another session holds the lock.
    Taken,
    /// No connection could be made, or it failed during the try.
    Unreachable,
}

/// Connects and tries the lock once, each step bounded by `interval` or the connect timeout.
async fn try_lock(config: &Config, interval: Duration) -> Attempt {
    let Ok((client, connection)) = config.connect(NoTls).await else {
        return Attempt::Unreachable;
    };
    let (closed_tx, closed) = tokio::sync::oneshot::channel();
    tokio::spawn(async move {
        // The connection's error has no one to go to; its end is what matters.
        let _ = connection.await;
        let _ = closed_tx.send(());
    });
    match tokio::time::timeout(
        interval,
        client.query_one("SELECT pg_try_advisory_lock($1)", &[&LEADER_LOCK_KEY]),
    )
    .await
    {
        Ok(Ok(row)) => match row.try_get::<_, bool>(0) {
            Ok(true) => Attempt::Held(client, closed),
            Ok(false) => Attempt::Taken,
            Err(_) => Attempt::Unreachable,
        },
        // Dropping the client ends the connection, and with it anything the try left behind.
        _ => Attempt::Unreachable,
    }
}

/// Watches the lock's session until it is lost, then reports the loss on `lost` and in
/// `readiness`. It owns the client, so the session lasts as long as it runs.
async fn heartbeat(
    client: Client,
    closed: tokio::sync::oneshot::Receiver<()>,
    interval: Duration,
    lost: watch::Sender<bool>,
    readiness: watch::Sender<Readiness>,
) {
    let client = &client;
    watch_session(closed, interval, || async move {
        client
            .simple_query("SELECT 1")
            .await
            .map(drop)
            .map_err(drop)
    })
    .await;
    readiness.send_modify(|state| state.leader = false);
    let _ = lost.send(true);
}

/// Runs `check` every `interval` and returns once the session is lost: when `closed` resolves,
/// when a check fails, or when a check has not answered within [`CHECK_INTERVALS`] intervals.
/// A check that is slow but answers in time keeps the lead.
async fn watch_session<F, C>(
    mut closed: tokio::sync::oneshot::Receiver<()>,
    interval: Duration,
    mut check: C,
) where
    C: FnMut() -> F,
    F: Future<Output = Result<(), ()>>,
{
    loop {
        tokio::select! {
            _ = &mut closed => return,
            () = tokio::time::sleep(interval) => {}
        }
        tokio::select! {
            _ = &mut closed => return,
            answered = tokio::time::timeout(interval * CHECK_INTERVALS, check()) => {
                if !matches!(answered, Ok(Ok(()))) {
                    return;
                }
            }
        }
    }
}

/// What one readiness probe found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Probe {
    /// A pooled connection answered `SELECT 1`.
    pub database: bool,
    /// Nothing of `tree` is pending on the database.
    pub migrations: bool,
}

/// Checks that the database answers and that every migration of `tree` is applied, each step
/// bounded by 2 seconds. A tree that fails its check, or no longer matches the ledger, counts
/// as migrations not current. A connection whose check timed out is not returned to the pool.
pub async fn probe(pool: &Pool, tree: &[(&str, &[u8])]) -> Probe {
    let down = Probe {
        database: false,
        migrations: false,
    };
    let Ok(Ok(mut client)) = tokio::time::timeout(PROBE_TIMEOUT, pool.get()).await else {
        return down;
    };
    match tokio::time::timeout(PROBE_TIMEOUT, client.simple_query("SELECT 1")).await {
        Ok(Ok(_)) => {}
        Ok(Err(_)) => return down,
        Err(_) => {
            detach(client);
            return down;
        }
    }
    let Ok(report) = validate(tree) else {
        return Probe {
            database: true,
            migrations: false,
        };
    };
    let migrations = match tokio::time::timeout(
        PROBE_TIMEOUT,
        plan_database(&mut client, &report, tree),
    )
    .await
    {
        Ok(Ok(plan)) => plan.pending.is_empty(),
        Ok(Err(_)) => false,
        Err(_) => {
            detach(client);
            false
        }
    };
    Probe {
        database: true,
        migrations,
    }
}

/// Takes a connection out of its pool for good, so a statement left running on it never
/// reaches the next caller.
fn detach(client: Object) {
    drop(Object::take(client));
}

/// Runs [`probe`] every `interval` and publishes its result to `readiness`.
fn spawn_probe(
    pool: Pool,
    tree: &'static [(&'static str, &'static [u8])],
    interval: Duration,
    readiness: watch::Sender<Readiness>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            let found = probe(&pool, tree).await;
            readiness.send_modify(|state| {
                state.database = found.database;
                state.migrations = found.migrations;
            });
            tokio::time::sleep(interval).await;
        }
    })
}

/// Applies the embedded migrations to the database of `config`, and gives what was applied.
pub async fn migrate(config: &Config) -> Result<Outcome, ApplyError> {
    migrations::apply(config, migrations::embedded()).await
}

/// Why [`start`] stopped after the lock was held.
#[derive(Debug, thiserror::Error)]
pub enum StartError {
    /// The migrations could not be applied.
    #[error(transparent)]
    Migrate(ApplyError),
    /// The pool could not be built.
    #[error("the database connection pool could not be built")]
    Pool,
    /// The projects could not be reconciled.
    #[error("the projects of mintswarm.yaml could not be reconciled into the database")]
    Reconcile(#[source] StoreError),
}

/// Starts the plane on the database of `config`: waits for the leader lock as
/// [`Leader::acquire`] does, applies the embedded migrations, reconciles `projects` through a
/// [`PgStore`] on a pool of its own, and starts the readiness probe, which runs until the
/// returned [`Leader`] is dropped.
pub async fn start(
    config: &Config,
    projects: &[ProjectEntry],
    readiness: &watch::Sender<Readiness>,
    intervals: Intervals,
) -> Result<Leader, StartError> {
    let mut leader = Leader::acquire(config, intervals.lock, readiness).await;
    migrate(config).await.map_err(StartError::Migrate)?;
    let pool = Pool::builder(Manager::new(config.clone(), NoTls))
        .max_size(POOL_SIZE)
        .wait_timeout(Some(PROBE_TIMEOUT))
        .create_timeout(Some(PROBE_TIMEOUT))
        .runtime(Runtime::Tokio1)
        .build()
        .map_err(|_| StartError::Pool)?;
    reconcile(&PgStore::new(pool.clone()), projects)
        .await
        .map_err(StartError::Reconcile)?;
    leader.tasks.push(spawn_probe(
        pool,
        migrations::embedded(),
        intervals.probe,
        readiness.clone(),
    ));
    Ok(leader)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::time::Duration;

    use tokio::sync::oneshot;

    use super::{CHECK_INTERVALS, watch_session};

    const INTERVAL: Duration = Duration::from_secs(1);

    /// One call of a fake session check.
    type Answer = std::pin::Pin<Box<dyn Future<Output = Result<(), ()>> + Send>>;

    /// A check whose `n`th call, counting from 0, takes `delay(n)` and then gives `answer(n)`.
    fn check(
        delay: impl Fn(u32) -> Duration,
        answer: impl Fn(u32) -> Result<(), ()>,
    ) -> (Arc<AtomicU32>, impl FnMut() -> Answer) {
        let calls = Arc::new(AtomicU32::new(0));
        let counter = Arc::clone(&calls);
        let check = move || {
            let n = counter.fetch_add(1, Ordering::SeqCst);
            let (delay, answer) = (delay(n), answer(n));
            Box::pin(async move {
                tokio::time::sleep(delay).await;
                answer
            }) as Answer
        };
        (calls, check)
    }

    #[tokio::test(start_paused = true)]
    async fn a_single_slow_check_keeps_the_lead() {
        let (_closed_tx, closed) = oneshot::channel::<()>();
        // The first check takes two intervals, longer than the interval but within the bound.
        let (calls, check) = check(
            |n| if n == 0 { INTERVAL * 2 } else { Duration::ZERO },
            |_| Ok(()),
        );
        let watched =
            tokio::time::timeout(INTERVAL * 20, watch_session(closed, INTERVAL, check)).await;
        assert!(watched.is_err(), "the session is still held");
        assert!(calls.load(Ordering::SeqCst) > 5, "later checks ran");
    }

    #[tokio::test(start_paused = true)]
    async fn a_check_that_never_answers_loses_the_lead_after_the_bound() {
        let (_closed_tx, closed) = oneshot::channel::<()>();
        let (_, check) = check(|_| Duration::from_secs(3600), |_| Ok(()));
        let started = tokio::time::Instant::now();
        watch_session(closed, INTERVAL, check).await;
        assert_eq!(started.elapsed(), INTERVAL + INTERVAL * CHECK_INTERVALS);
    }

    #[tokio::test(start_paused = true)]
    async fn a_failed_check_loses_the_lead_at_once() {
        let (_closed_tx, closed) = oneshot::channel::<()>();
        let (_, check) = check(|_| Duration::ZERO, |n| if n < 2 { Ok(()) } else { Err(()) });
        let started = tokio::time::Instant::now();
        watch_session(closed, INTERVAL, check).await;
        assert_eq!(started.elapsed(), INTERVAL * 3);
    }

    #[tokio::test(start_paused = true)]
    async fn a_closed_connection_loses_the_lead_during_a_check() {
        let (closed_tx, closed) = oneshot::channel::<()>();
        let (_, check) = check(|_| Duration::from_secs(3600), |_| Ok(()));
        let watching = tokio::spawn(watch_session(closed, INTERVAL, check));
        tokio::time::sleep(INTERVAL + INTERVAL / 2).await;
        closed_tx.send(()).expect("the watcher listens");
        tokio::time::timeout(INTERVAL / 4, watching)
            .await
            .expect("the watcher returns at once")
            .expect("the watcher completes");
    }
}
