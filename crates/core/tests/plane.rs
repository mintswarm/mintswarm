//! The plane's start: the database settings conversion, and, with `--features pg-tests` and a
//! local Postgres or container image, the leader lock and its takeover, the readiness probe,
//! the reconciliation of `projects:` and the embedded migrations.

#![cfg(test)]

use mintswarm_config::{DatabaseSettings, Secret};
use mintswarm_core::plane::database_config;

const PLANTED: &str = "s3cr3t-plant";

fn settings(url: &str, password: Option<&str>) -> DatabaseSettings {
    DatabaseSettings {
        url: Secret::new(url),
        password: password.map(Secret::new),
    }
}

#[test]
fn database_config_parses_a_url() {
    let config = database_config(&settings("postgres://msw@db.example:6543/plane", None))
        .expect("the URL parses");
    assert_eq!(config.get_user(), Some("msw"));
    assert_eq!(config.get_dbname(), Some("plane"));
    assert_eq!(config.get_ports(), &[6543]);
}

#[test]
fn the_password_setting_replaces_the_urls() {
    let config = database_config(&settings(
        "postgres://msw:in-the-url@db/plane",
        Some("from-the-setting"),
    ))
    .expect("the URL parses");
    assert_eq!(config.get_password(), Some(&b"from-the-setting"[..]));
    let config =
        database_config(&settings("postgres://msw:in-the-url@db/plane", None)).expect("parses");
    assert_eq!(config.get_password(), Some(&b"in-the-url"[..]));
}

#[test]
fn a_malformed_url_is_refused_without_showing_it() {
    let url = format!("postgres://msw:{PLANTED}@db:notaport/plane");
    let error = database_config(&settings(&url, None)).expect_err("the port is not a number");
    let shown = format!("{error} {error:?}");
    assert!(!shown.contains(PLANTED), "{shown}");
}

#[test]
fn the_leader_lock_key_is_its_own() {
    assert_ne!(
        mintswarm_core::plane::LEADER_LOCK_KEY,
        mintswarm_core::migrations::MIGRATION_LOCK_KEY
    );
    assert_eq!(
        mintswarm_core::plane::LEADER_LOCK_KEY.to_be_bytes(),
        *b"mswmlead"
    );
}

#[cfg(feature = "pg-tests")]
mod pg_tests {
    use std::time::Duration;

    use deadpool_postgres::{Manager, Pool, Runtime};
    use mintswarm_config::{Host, ProjectEntry};
    use mintswarm_core::migrations::apply;
    use mintswarm_core::plane::{self, Intervals, LEADER_LOCK_KEY, Leader, Readiness, probe};
    use mintswarm_core::testing::pg;
    use mintswarm_domain::ids::ProjectId;
    use tokio::sync::watch;
    use tokio_postgres::{Client, Config, NoTls};

    const INTERVAL: Duration = Duration::from_millis(100);
    const BOUND: Duration = Duration::from_secs(5);

    fn fast() -> Intervals {
        Intervals {
            lock: INTERVAL,
            probe: INTERVAL,
        }
    }

    async fn connect(config: &Config) -> Client {
        let (client, connection) = config
            .connect(NoTls)
            .await
            .expect("the test database accepts connections");
        tokio::spawn(async move {
            let _ = connection.await;
        });
        client
    }

    /// Waits up to [`BOUND`] for `readiness` to satisfy `holds`.
    async fn wait_for(
        readiness: &mut watch::Receiver<Readiness>,
        holds: impl Fn(&Readiness) -> bool,
        what: &str,
    ) {
        tokio::time::timeout(BOUND, readiness.wait_for(|state| holds(state)))
            .await
            .unwrap_or_else(|_| panic!("{what} within {BOUND:?}"))
            .expect("the readiness sender is kept");
    }

    /// A config for a loopback port nothing listens on.
    fn closed_port() -> Config {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("a free port");
        let port = listener.local_addr().expect("bound").port();
        drop(listener);
        let mut config = Config::new();
        config
            .host("127.0.0.1")
            .port(port)
            .user("mintswarm")
            .dbname("mintswarm")
            .connect_timeout(Duration::from_secs(1));
        config
    }

    fn pool(config: &Config) -> Pool {
        Pool::builder(Manager::new(config.clone(), NoTls))
            .max_size(2)
            .wait_timeout(Some(Duration::from_secs(2)))
            .create_timeout(Some(Duration::from_secs(2)))
            .runtime(Runtime::Tokio1)
            .build()
            .expect("the pool builds")
    }

    fn entry(id: &str, weight: i32) -> ProjectEntry {
        ProjectEntry {
            project_id: ProjectId(id.to_owned()),
            repo: format!("mintswarm/{id}"),
            host: Host::Github,
            installation_id: 4242,
            weight,
        }
    }

    /// The `projects` rows as `(project_id, weight, row_version)`, by id.
    async fn rows(client: &Client) -> Vec<(String, i32, i64)> {
        client
            .query(
                "SELECT project_id, weight, row_version FROM mintswarm.projects \
                 ORDER BY project_id",
                &[],
            )
            .await
            .expect("the projects table is readable")
            .iter()
            .map(|row| (row.get(0), row.get(1), row.get(2)))
            .collect()
    }

    #[tokio::test]
    async fn a_standby_is_not_ready_until_the_leaders_session_ends() {
        let db = pg::database().await.expect("a test database");
        let (first_tx, mut first_rx) = watch::channel(Readiness::default());
        let mut first = plane::start(db.config(), &[], &first_tx, fast())
            .await
            .expect("the first plane starts");
        wait_for(
            &mut first_rx,
            Readiness::is_ready,
            "the first plane is ready",
        )
        .await;

        let (second_tx, mut second_rx) = watch::channel(Readiness::default());
        let config = db.config().clone();
        let second = tokio::spawn(async move {
            let leader = plane::start(&config, &[], &second_tx, fast()).await;
            (leader, second_tx)
        });
        wait_for(
            &mut second_rx,
            |state| state.database && !state.leader,
            "the standby sees the database",
        )
        .await;
        tokio::time::sleep(INTERVAL * 5).await;
        let state = *second_rx.borrow();
        assert!(!state.leader && !state.is_ready(), "{state:?}");
        assert!(!second.is_finished(), "the standby still waits");

        let admin = connect(db.config()).await;
        admin
            .execute("SELECT pg_terminate_backend($1)", &[&first.backend_pid()])
            .await
            .expect("the leader's backend is terminated");
        tokio::time::timeout(BOUND, first.lost())
            .await
            .expect("the first plane sees its lock lost");
        assert!(!first_rx.borrow().leader);

        let (leader, _second_tx) = tokio::time::timeout(BOUND, second)
            .await
            .expect("the standby takes over")
            .expect("the standby's task completes");
        let leader = leader.expect("the second plane starts");
        wait_for(
            &mut second_rx,
            Readiness::is_ready,
            "the second plane is ready",
        )
        .await;
        let holder: i32 = admin
            .query_one(
                "SELECT pid FROM pg_locks WHERE locktype = 'advisory' AND granted \
                 AND ((classid::bigint << 32) | objid::bigint) = $1",
                &[&LEADER_LOCK_KEY],
            )
            .await
            .expect("one session holds the leader lock")
            .get(0);
        assert_eq!(holder, leader.backend_pid());
    }

    #[tokio::test]
    async fn acquire_keeps_waiting_while_the_database_is_unreachable() {
        let (tx, rx) = watch::channel(Readiness {
            leader: false,
            database: true,
            migrations: false,
        });
        let config = closed_port();
        let waited =
            tokio::time::timeout(INTERVAL * 5, Leader::acquire(&config, INTERVAL, &tx)).await;
        assert!(waited.is_err(), "acquire is still pending");
        let state = *rx.borrow();
        assert!(!state.database && !state.leader, "{state:?}");
    }

    #[tokio::test]
    async fn probe_reports_an_unreachable_database() {
        let found = probe(&pool(&closed_port()), &[]).await;
        assert!(!found.database && !found.migrations, "{found:?}");
    }

    #[tokio::test]
    async fn probe_reports_pending_migrations_until_they_are_applied() {
        let first = (
            "mintswarm/migrations/20260101000000_a.up.sql",
            &b"CREATE TABLE a (id int);\n"[..],
        );
        let second = (
            "mintswarm/migrations/20260102000000_b.up.sql",
            &b"CREATE TABLE b (id int);\n"[..],
        );
        let (db, _) = pg::migrated_database(&[first])
            .await
            .expect("the first file is applied");
        let pool = pool(db.config());
        let found = probe(&pool, &[first, second]).await;
        assert!(found.database && !found.migrations, "{found:?}");

        apply(db.config(), &[first, second])
            .await
            .expect("the second file is applied");
        let found = probe(&pool, &[first, second]).await;
        assert!(found.database && found.migrations, "{found:?}");
    }

    #[tokio::test]
    async fn projects_are_reconciled_on_each_start() {
        let db = pg::database().await.expect("a test database");
        let (tx, _rx) = watch::channel(Readiness::default());
        let leader = plane::start(
            db.config(),
            &[entry("alpha", 1), entry("beta", 2)],
            &tx,
            fast(),
        )
        .await
        .expect("the plane starts");
        let admin = connect(db.config()).await;
        assert_eq!(
            rows(&admin).await,
            vec![("alpha".to_owned(), 1, 1), ("beta".to_owned(), 2, 1)]
        );
        drop(leader);

        let restarted = tokio::time::timeout(
            BOUND,
            plane::start(db.config(), &[entry("alpha", 5)], &tx, fast()),
        )
        .await
        .expect("the restart takes the released lock")
        .expect("the plane restarts");
        assert_eq!(
            rows(&admin).await,
            vec![("alpha".to_owned(), 5, 2), ("beta".to_owned(), 2, 1)],
            "the edited weight is updated, and the dropped entry's row stays"
        );
        drop(restarted);
    }

    #[tokio::test]
    async fn migrate_applies_the_embedded_tree_once() {
        let db = pg::database().await.expect("a test database");
        let outcome = plane::migrate(db.config()).await.expect("migrates");
        assert!(
            outcome
                .applied
                .iter()
                .any(|applied| applied.name == "projects"),
            "{outcome:?}"
        );
        let admin = connect(db.config()).await;
        let exists: bool = admin
            .query_one("SELECT to_regclass('mintswarm.projects') IS NOT NULL", &[])
            .await
            .expect("the catalogue is readable")
            .get(0);
        assert!(exists, "mintswarm.projects is created");
        let again = plane::migrate(db.config()).await.expect("migrates again");
        assert!(again.applied.is_empty(), "{again:?}");
    }
}
