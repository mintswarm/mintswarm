//! Tests of the migrations: every table of design §12 with its columns, the
//! `unrecognised_failures` table of migration 2 and the `jobs.components` rename of migration 3
//! beyond §12, re-running them, and migrating a database at version 2 that holds a job.

use rusqlite::Connection;

use super::super::schema::{MIGRATIONS, migrate};
use super::super::{OutboxMode, SCHEMA_VERSION, StoreError};
use super::{fresh_runner_dir, open};

/// The tables of design §12 and `unrecognised_failures`, beyond it, and their columns, in order.
const TABLES: &[(&str, &[&str])] = &[
    (
        "artefacts",
        &[
            "job_id",
            "name",
            "revision",
            "sha256",
            "blob",
            "published_url",
        ],
    ),
    (
        "attempts",
        &[
            "attempt_id",
            "job_id",
            "step",
            "n",
            "harness",
            "model",
            "effort",
            "instructions_commit",
            "instructions_sha256",
            "session_id",
            "pgid",
            "started_at",
            "ended_at",
            "outcome",
            "output_sha256",
            "transcript_blob",
        ],
    ),
    (
        "inputs",
        &["job_id", "attempt_id", "path", "sha256", "source"],
    ),
    (
        "jobs",
        &[
            "job_id",
            "issue",
            "kind",
            "components",
            "state",
            "paused",
            "base_commit",
            "branch",
            "pr",
            "plan_sha256",
            "approval",
            "counters",
            "row_version",
        ],
    ),
    ("leases", &["resource", "holder", "expires_at"]),
    (
        "outbox",
        &[
            "effect_id",
            "job_id",
            "kind",
            "effect_key",
            "payload_blob",
            "state",
            "attempts",
            "next_at",
            "result",
        ],
    ),
    (
        "poll_cursors",
        &["url", "etag", "last_modified", "cursor", "polled_at"],
    ),
    ("probes", &["id", "result", "source", "observed_at"]),
    (
        "records",
        &["job_id", "kind", "version", "sha256", "blob", "bound"],
    ),
    (
        "rounds",
        &["job_id", "kind", "n", "head", "round_sha256", "verdict"],
    ),
    (
        "shadow",
        &[
            "job_id",
            "item",
            "ours_sha256",
            "reference_sha256",
            "field_diffs",
        ],
    ),
    ("snapshots", &["commit", "path", "refcount"]),
    (
        "transitions",
        &[
            "job_id",
            "seq",
            "from_state",
            "event",
            "guard_inputs",
            "to_state",
            "at",
        ],
    ),
    (
        "unrecognised_failures",
        &[
            "adapter",
            "signature",
            "excerpt",
            "status",
            "count",
            "first_seen_at",
            "last_seen_at",
        ],
    ),
    (
        "usage",
        &[
            "attempt_id",
            "seq",
            "tokens",
            "cost",
            "limits",
            "observed_at",
        ],
    ),
    (
        "worktrees",
        &["job_id", "path", "start_commit", "head", "pushed_head"],
    ),
];

/// The user tables of `conn`, by name.
fn tables(conn: &Connection) -> Vec<String> {
    let mut statement = conn
        .prepare(
            "SELECT name FROM sqlite_schema \
             WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name",
        )
        .unwrap();
    statement
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}

/// The columns of `table`, in order.
fn columns(conn: &Connection, table: &str) -> Vec<String> {
    let mut statement = conn
        .prepare("SELECT name FROM pragma_table_info(?1) ORDER BY cid")
        .unwrap();
    statement
        .query_map([table], |row| row.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}

/// The schema version and every schema entry of `conn`.
fn dump(conn: &Connection) -> (i64, Vec<(String, String, Option<String>)>) {
    let version = conn
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap();
    let mut statement = conn
        .prepare("SELECT type, name, sql FROM sqlite_schema ORDER BY type, name")
        .unwrap();
    let entries = statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    (version, entries)
}

#[test]
fn migrations_create_every_table_of_section_12_with_its_columns() {
    let (_tmp, runner_dir) = fresh_runner_dir();
    let store = open(&runner_dir, OutboxMode::Live).unwrap();
    let expected: Vec<&str> = TABLES.iter().map(|(name, _)| *name).collect();
    assert_eq!(tables(&store.conn), expected);
    for (table, expected) in TABLES {
        assert_eq!(columns(&store.conn, table), *expected, "{table}");
    }
}

#[test]
fn re_running_the_migrations_changes_nothing() {
    let (_tmp, runner_dir) = fresh_runner_dir();
    let mut store = open(&runner_dir, OutboxMode::Live).unwrap();
    let before = dump(&store.conn);
    assert_eq!(before.0, SCHEMA_VERSION);
    migrate(&mut store.conn).unwrap();
    assert_eq!(dump(&store.conn), before);
    drop(store);
    let store = open(&runner_dir, OutboxMode::Live).unwrap();
    assert_eq!(dump(&store.conn), before);
}

#[test]
fn a_job_at_version_2_keeps_its_component_under_components() {
    let (_tmp, runner_dir) = fresh_runner_dir();
    drop(open(&runner_dir, OutboxMode::Live).unwrap());
    let database = runner_dir.join("runner.sqlite");
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{suffix}", database.display()));
    }
    let conn = Connection::open(&database).unwrap();
    for script in &MIGRATIONS[..2] {
        conn.execute_batch(script).unwrap();
    }
    conn.pragma_update(None, "user_version", 2).unwrap();
    conn.execute(
        "INSERT INTO jobs (job_id, component) VALUES ('job-1', 'domain'), ('job-2', NULL)",
        [],
    )
    .unwrap();
    drop(conn);
    let store = open(&runner_dir, OutboxMode::Live).unwrap();
    let rows: Vec<(String, Option<String>)> = store
        .conn
        .prepare("SELECT job_id, components FROM jobs ORDER BY job_id")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(
        rows,
        [
            ("job-1".to_owned(), Some(r#"["domain"]"#.to_owned())),
            ("job-2".to_owned(), None),
        ]
    );
}

#[test]
fn a_newer_schema_version_is_refused() {
    let (_tmp, runner_dir) = fresh_runner_dir();
    drop(open(&runner_dir, OutboxMode::Live).unwrap());
    let conn = Connection::open(runner_dir.join("runner.sqlite")).unwrap();
    conn.pragma_update(None, "user_version", SCHEMA_VERSION + 1)
        .unwrap();
    drop(conn);
    let err = open(&runner_dir, OutboxMode::Live).unwrap_err();
    assert!(
        matches!(err, StoreError::SchemaTooNew { found, known }
            if found == SCHEMA_VERSION + 1 && known == SCHEMA_VERSION),
        "{err:?}"
    );
}

#[test]
fn a_negative_schema_version_is_refused() {
    let (_tmp, runner_dir) = fresh_runner_dir();
    drop(open(&runner_dir, OutboxMode::Live).unwrap());
    let conn = Connection::open(runner_dir.join("runner.sqlite")).unwrap();
    conn.pragma_update(None, "user_version", -1).unwrap();
    drop(conn);
    let err = open(&runner_dir, OutboxMode::Live).unwrap_err();
    assert!(matches!(err, StoreError::SchemaInvalid(-1)), "{err:?}");
}

#[test]
fn the_outbox_refuses_an_unknown_state() {
    let (_tmp, runner_dir) = fresh_runner_dir();
    let store = open(&runner_dir, OutboxMode::Live).unwrap();
    let inserted = store.conn.execute(
        "INSERT INTO outbox (job_id, kind, effect_key, payload_blob, state) \
         VALUES ('j', 'comment', 'k', 'b', 'shipped')",
        [],
    );
    assert!(inserted.is_err());
}
