//! The Postgres test harness: databases per call, durability off, and removal after the test
//! process ends. Runs only with `--features pg-tests` and a local Postgres or container image.

#![cfg(all(test, feature = "pg-tests"))]

use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use mintswarm_core::testing::pg::{self, Backend};
use tokio_postgres::{Client, Config, NoTls};

/// The marker of the line `child_holds_instance` prints for its parent.
const MARKER: &str = "MSW_PG_INSTANCE";

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

#[tokio::test]
async fn database_is_connectable() {
    let db = pg::database().await.unwrap();
    let client = connect(db.config()).await;
    let row = client
        .query_one("SELECT current_database()", &[])
        .await
        .unwrap();
    assert_eq!(row.get::<_, String>(0), db.name());
}

#[tokio::test]
async fn two_calls_get_different_databases() {
    let first = pg::database().await.unwrap();
    let second = pg::database().await.unwrap();
    assert_ne!(first.name(), second.name());

    let one = connect(first.config()).await;
    one.batch_execute("CREATE TABLE only_here (id int)")
        .await
        .unwrap();
    let two = connect(second.config()).await;
    let row = two
        .query_one(
            "SELECT count(*) FROM pg_tables WHERE tablename = 'only_here'",
            &[],
        )
        .await
        .unwrap();
    assert_eq!(row.get::<_, i64>(0), 0);
}

#[tokio::test]
async fn durability_settings_are_off() {
    let db = pg::database().await.unwrap();
    let client = connect(db.config()).await;
    for setting in ["fsync", "synchronous_commit", "full_page_writes"] {
        let row = client
            .query_one(&format!("SHOW {setting}"), &[])
            .await
            .unwrap();
        assert_eq!(row.get::<_, String>(0), "off", "{setting}");
    }
}

/// Run by `instance_is_removed_after_process_exit` in a child process: starts the instance
/// and prints where it lives, tab-separated.
#[tokio::test]
#[ignore = "a helper run by instance_is_removed_after_process_exit"]
async fn child_holds_instance() {
    let instance = pg::instance().await.unwrap();
    match instance.backend() {
        Backend::Binaries { .. } => println!(
            "{MARKER}\tbinaries\t{}\t{}",
            instance.dir().unwrap().display(),
            instance.server_pid().unwrap()
        ),
        Backend::Container { runtime, .. } => println!(
            "{MARKER}\tcontainer\t{}\t{}",
            runtime.display(),
            instance.container_id().unwrap()
        ),
        other => panic!("unexpected backend {other:?}"),
    }
}

#[test]
fn instance_is_removed_after_process_exit() {
    let output = Command::new(std::env::current_exe().unwrap())
        .args([
            "child_holds_instance",
            "--exact",
            "--ignored",
            "--nocapture",
        ])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "the child failed: {stdout}");
    let line = stdout
        .lines()
        .find(|line| line.starts_with(MARKER))
        .unwrap_or_else(|| panic!("no {MARKER} line in: {stdout}"));
    let fields: Vec<&str> = line.split('\t').collect();

    let deadline = Instant::now() + Duration::from_secs(30);
    let removed = || match fields[1] {
        "binaries" => {
            let pid: i32 = fields[3].parse().unwrap();
            let pid = rustix::process::Pid::from_raw(pid).unwrap();
            !Path::new(fields[2]).exists() && rustix::process::test_kill_process(pid).is_err()
        }
        "container" => {
            let ps = Command::new(fields[2])
                .args(["ps", "-aq", "--filter", &format!("id={}", fields[3])])
                .output()
                .unwrap();
            assert!(ps.status.success());
            ps.stdout.iter().all(u8::is_ascii_whitespace)
        }
        other => panic!("unexpected backend {other}"),
    };
    while !removed() {
        assert!(
            Instant::now() < deadline,
            "the instance was not removed: {line}"
        );
        std::thread::sleep(Duration::from_millis(200));
    }
}
