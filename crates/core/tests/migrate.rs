//! The migration runner against real databases: the ledger, the checks before any write, one
//! transaction per file, and the session lock. Runs only with `--features pg-tests` and a local
//! Postgres or container image.

#![cfg(all(test, feature = "pg-tests"))]

use std::time::Duration;

use mintswarm_core::migrations::{
    ApplyError, ApplyWarning, Drift, LEDGER_SCHEMA, LEDGER_TABLE, MIGRATION_LOCK_KEY, apply,
};
use mintswarm_core::testing::pg;
use tokio_postgres::{Client, Config, NoTls};

/// A ledger row as the tests compare it: schema, version, name, kind, sha256 in hex,
/// applied_at as text, compacted.
type Row = (String, String, String, String, String, String, bool);

/// The path of an up file of the `mintswarm` schema.
fn up(version: &str, name: &str) -> String {
    format!("mintswarm/migrations/{version}_{name}.up.sql")
}

/// A tree of `(path, text)` pairs as the runner takes it.
fn tree<'a>(files: &'a [(String, &'a str)]) -> Vec<(&'a str, &'a [u8])> {
    files
        .iter()
        .map(|(path, text)| (path.as_str(), text.as_bytes()))
        .collect()
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

async fn ledger(client: &Client) -> Vec<Row> {
    client
        .query(
            &format!(
                "SELECT schema, version, name, kind, encode(sha256, 'hex'), applied_at::text, \
                 compacted FROM {LEDGER_SCHEMA}.{LEDGER_TABLE} ORDER BY schema, version"
            ),
            &[],
        )
        .await
        .unwrap()
        .iter()
        .map(|row| {
            (
                row.get(0),
                row.get(1),
                row.get(2),
                row.get(3),
                row.get(4),
                row.get(5),
                row.get(6),
            )
        })
        .collect()
}

/// The versions the ledger records.
async fn versions(client: &Client) -> Vec<String> {
    ledger(client).await.into_iter().map(|row| row.1).collect()
}

/// Whether a relation exists, by a name `to_regclass` reads.
async fn exists(client: &Client, relation: &str) -> bool {
    client
        .query_one("SELECT to_regclass($1) IS NOT NULL", &[&relation])
        .await
        .unwrap()
        .get(0)
}

/// Whether a fresh session can take the migration lock; it releases it again at once.
async fn lock_is_free(config: &Config) -> bool {
    let probe = connect(config).await;
    let free: bool = probe
        .query_one("SELECT pg_try_advisory_lock($1)", &[&MIGRATION_LOCK_KEY])
        .await
        .unwrap()
        .get(0);
    if free {
        probe
            .execute("SELECT pg_advisory_unlock($1)", &[&MIGRATION_LOCK_KEY])
            .await
            .unwrap();
    }
    free
}

const V1: &str = "20260101000000";
const V2: &str = "20260102000000";
const V3: &str = "20260103000000";

#[tokio::test]
async fn a_second_run_applies_nothing_and_keeps_the_ledger() {
    let db = pg::database().await.unwrap();
    let files = [
        (up(V1, "first"), "CREATE TABLE first (id int);"),
        (up(V2, "second"), "INSERT INTO first VALUES (1);"),
    ];
    let first = apply(db.config(), &tree(&files)).await.unwrap();
    let applied: Vec<&str> = first.applied.iter().map(|a| a.file.as_str()).collect();
    assert_eq!(applied, [files[0].0.as_str(), files[1].0.as_str()]);

    let client = connect(db.config()).await;
    let before = ledger(&client).await;
    let second = apply(db.config(), &tree(&files)).await.unwrap();
    assert!(second.applied.is_empty());
    assert_eq!(ledger(&client).await, before);
    let rows: i64 = client
        .query_one("SELECT count(*) FROM mintswarm.first", &[])
        .await
        .unwrap()
        .get(0);
    assert_eq!(rows, 1);
}

#[tokio::test]
async fn concurrent_runs_apply_each_file_once() {
    let db = pg::database().await.unwrap();
    let files = [
        (
            up(V1, "effects"),
            "CREATE TABLE effects (id int); SELECT pg_sleep(0.3);",
        ),
        (up(V2, "effect"), "INSERT INTO effects VALUES (1);"),
    ];
    let files = tree(&files);
    let (one, two) = tokio::join!(apply(db.config(), &files), apply(db.config(), &files));
    let (one, two) = (one.unwrap(), two.unwrap());
    let mut applied: Vec<String> = one
        .applied
        .iter()
        .chain(&two.applied)
        .map(|a| a.version.clone())
        .collect();
    applied.sort();
    assert_eq!(applied, [V1, V2]);

    let client = connect(db.config()).await;
    assert_eq!(versions(&client).await, [V1, V2]);
    let rows: i64 = client
        .query_one("SELECT count(*) FROM mintswarm.effects", &[])
        .await
        .unwrap()
        .get(0);
    assert_eq!(rows, 1);
}

#[tokio::test]
async fn an_edited_file_refuses_the_run() {
    let db = pg::database().await.unwrap();
    let a = up(V1, "a");
    apply(
        db.config(),
        &tree(&[(a.clone(), "CREATE TABLE a (id int);")]),
    )
    .await
    .unwrap();

    let edited = [
        (a.clone(), "CREATE TABLE a (id bigint);"),
        (up(V2, "b"), "CREATE TABLE b (id int);"),
    ];
    let error = apply(db.config(), &tree(&edited)).await.unwrap_err();
    let ApplyError::Drift { drifts } = error else {
        panic!("expected a drift, got {error:?}");
    };
    assert_eq!(drifts, [Drift::Edited { file: a }]);

    let client = connect(db.config()).await;
    assert_eq!(versions(&client).await, [V1]);
    assert!(!exists(&client, "mintswarm.b").await);
}

#[tokio::test]
async fn a_renamed_file_refuses_the_run() {
    let db = pg::database().await.unwrap();
    let text = "CREATE TABLE a (id int);";
    apply(db.config(), &tree(&[(up(V1, "old_name"), text)]))
        .await
        .unwrap();

    let renamed = up(V1, "new_name");
    let files = [
        (renamed.clone(), text),
        (up(V2, "c"), "CREATE TABLE c (id int);"),
    ];
    let error = apply(db.config(), &tree(&files)).await.unwrap_err();
    let ApplyError::Drift { drifts } = error else {
        panic!("expected a drift, got {error:?}");
    };
    assert!(
        matches!(drifts.as_slice(), [Drift::Renamed { file, recorded_name, .. }]
            if *file == renamed && recorded_name == "old_name"),
        "{drifts:?}"
    );

    let client = connect(db.config()).await;
    assert_eq!(versions(&client).await, [V1]);
    assert!(!exists(&client, "mintswarm.c").await);
}

#[tokio::test]
async fn an_orphan_row_warns_and_the_run_proceeds() {
    let db = pg::database().await.unwrap();
    let a = (up(V1, "a"), "CREATE TABLE a (id int);");
    let b = (up(V2, "b"), "CREATE TABLE b (id int);");
    let c = (up(V3, "c"), "CREATE TABLE c (id int);");
    apply(db.config(), &tree(&[a.clone(), b])).await.unwrap();

    let outcome = apply(db.config(), &tree(&[a, c.clone()])).await.unwrap();
    assert!(
        outcome.warnings.contains(&ApplyWarning::Orphan {
            schema: "mintswarm".to_owned(),
            version: V2.to_owned(),
            name: "b".to_owned(),
        }),
        "{:?}",
        outcome.warnings
    );
    let applied: Vec<&str> = outcome.applied.iter().map(|a| a.file.as_str()).collect();
    assert_eq!(applied, [c.0.as_str()]);

    let client = connect(db.config()).await;
    assert_eq!(versions(&client).await, [V1, V2, V3]);
    assert!(exists(&client, "mintswarm.c").await);
}

#[tokio::test]
async fn a_lower_unapplied_version_is_applied() {
    let db = pg::database().await.unwrap();
    let earlier = (up(V1, "earlier"), "CREATE TABLE earlier (id int);");
    let later = (up(V2, "later"), "CREATE TABLE later (id int);");
    apply(db.config(), &tree(&[later.clone()])).await.unwrap();

    let outcome = apply(db.config(), &tree(&[earlier.clone(), later]))
        .await
        .unwrap();
    let applied: Vec<&str> = outcome.applied.iter().map(|a| a.file.as_str()).collect();
    assert_eq!(applied, [earlier.0.as_str()]);

    let client = connect(db.config()).await;
    assert_eq!(versions(&client).await, [V1, V2]);
    assert!(exists(&client, "mintswarm.earlier").await);
}

#[tokio::test]
async fn a_failing_file_stops_the_run_and_applies_nothing_of_itself() {
    let db = pg::database().await.unwrap();
    let second = up(V2, "second");
    let files = [
        (up(V1, "first"), "CREATE TABLE a (id int);"),
        (second.clone(), "CREATE TABLE b (id int); SELECT 1/0;"),
        (up(V3, "third"), "CREATE TABLE c (id int);"),
    ];
    let error = apply(db.config(), &tree(&files)).await.unwrap_err();
    assert!(
        error.to_string().contains("nothing of it was applied"),
        "{error}"
    );
    let ApplyError::Failed { file, .. } = error else {
        panic!("expected a failed file, got {error:?}");
    };
    assert_eq!(file, second);

    let client = connect(db.config()).await;
    assert_eq!(versions(&client).await, [V1]);
    assert!(exists(&client, "mintswarm.a").await);
    assert!(!exists(&client, "mintswarm.b").await);
    assert!(!exists(&client, "mintswarm.c").await);
}

#[tokio::test]
async fn narrowing_an_applied_table_is_refused() {
    let db = pg::database().await.unwrap();
    let a = (up(V1, "create_t"), "CREATE TABLE t (id int);");
    apply(db.config(), &tree(&[a.clone()])).await.unwrap();

    let drop = up(V3, "drop_t");
    let files = [
        a,
        (up(V2, "additive"), "CREATE TABLE c (id int);"),
        (drop.clone(), "DROP TABLE t;"),
    ];
    let error = apply(db.config(), &tree(&files)).await.unwrap_err();
    let ApplyError::Narrowing { refusals } = error else {
        panic!("expected a narrowing refusal, got {error:?}");
    };
    assert!(
        refusals
            .iter()
            .all(|refusal| refusal.to_string().starts_with(&drop))
            && !refusals.is_empty(),
        "{refusals:?}"
    );

    let client = connect(db.config()).await;
    assert_eq!(versions(&client).await, [V1]);
    assert!(!exists(&client, "mintswarm.c").await);
    assert!(exists(&client, "mintswarm.t").await);
}

#[tokio::test]
async fn narrowing_a_table_created_in_the_same_run_is_applied() {
    let db = pg::database().await.unwrap();
    let files = [
        (up(V1, "create_t"), "CREATE TABLE t (id int);"),
        (up(V2, "drop_t"), "DROP TABLE t;"),
    ];
    let outcome = apply(db.config(), &tree(&files)).await.unwrap();
    assert_eq!(outcome.applied.len(), 2);

    let client = connect(db.config()).await;
    assert_eq!(versions(&client).await, [V1, V2]);
    assert!(!exists(&client, "mintswarm.t").await);
}

#[tokio::test]
async fn files_land_in_their_schema_and_the_ledger_has_its_shape() {
    let db = pg::database().await.unwrap();
    apply(
        db.config(),
        &tree(&[(up(V1, "plain"), "CREATE TABLE plain (id int);")]),
    )
    .await
    .unwrap();

    let client = connect(db.config()).await;
    assert!(exists(&client, "mintswarm.plain").await);
    assert!(!exists(&client, "public.plain").await);

    let columns: Vec<String> = client
        .query(
            "SELECT column_name::text FROM information_schema.columns \
             WHERE table_schema = $1 AND table_name = $2 ORDER BY ordinal_position",
            &[&LEDGER_SCHEMA, &LEDGER_TABLE],
        )
        .await
        .unwrap()
        .iter()
        .map(|row| row.get(0))
        .collect();
    assert_eq!(
        columns,
        [
            "schema",
            "version",
            "name",
            "kind",
            "sha256",
            "applied_at",
            "compacted",
            "floor_change",
            "floor_commit",
            "floor_source",
        ]
    );
    assert!(!columns.iter().any(|column| column == "adopted"));

    let checks: i64 = client
        .query_one(
            "SELECT count(*) FROM pg_constraint \
             WHERE conrelid = to_regclass($1) AND contype = 'c'",
            &[&format!("{LEDGER_SCHEMA}.{LEDGER_TABLE}")],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(checks, 0);

    let row = &ledger(&client).await[0];
    assert_eq!(
        (
            row.0.as_str(),
            row.1.as_str(),
            row.2.as_str(),
            row.3.as_str()
        ),
        ("mintswarm", V1, "plain", "migration")
    );
    assert!(!row.6);
}

#[tokio::test]
async fn another_key_does_not_block_and_the_lock_is_released() {
    let db = pg::database().await.unwrap();
    let config = db.config();

    let holder = connect(config).await;
    holder
        .execute("SELECT pg_advisory_lock($1)", &[&MIGRATION_LOCK_KEY])
        .await
        .unwrap();
    assert!(!lock_is_free(config).await);
    holder
        .execute("SELECT pg_advisory_unlock($1)", &[&MIGRATION_LOCK_KEY])
        .await
        .unwrap();

    holder
        .execute("SELECT pg_advisory_lock($1)", &[&(MIGRATION_LOCK_KEY ^ 1)])
        .await
        .unwrap();
    let files = [(up(V1, "a"), "CREATE TABLE a (id int);")];
    let run = tokio::time::timeout(Duration::from_secs(10), apply(config, &tree(&files))).await;
    assert!(matches!(run, Ok(Ok(_))), "{run:?}");
    assert!(lock_is_free(config).await);

    let failing = [
        files[0].clone(),
        (up(V2, "bad"), "SELECT * FROM no_such_table;"),
    ];
    let error = apply(config, &tree(&failing)).await.unwrap_err();
    assert!(matches!(error, ApplyError::Failed { .. }), "{error:?}");
    assert!(lock_is_free(config).await);
}

#[tokio::test]
async fn the_helper_builds_through_apply() {
    let files = [
        (up(V1, "a"), "CREATE TABLE a (id int);"),
        (up(V2, "b"), "CREATE TABLE b (id int);"),
    ];
    let (db, outcome) = pg::migrated_database(&tree(&files)).await.unwrap();
    let applied: Vec<&str> = outcome.applied.iter().map(|a| a.file.as_str()).collect();
    assert_eq!(applied, [files[0].0.as_str(), files[1].0.as_str()]);

    let client = connect(db.config()).await;
    assert_eq!(versions(&client).await, [V1, V2]);
    assert!(exists(&client, "mintswarm.a").await);
    assert!(exists(&client, "mintswarm.b").await);
}

#[tokio::test]
async fn a_file_ending_in_rollback_gets_no_ledger_row() {
    let db = pg::database().await.unwrap();
    let file = up(V1, "rolls_back");
    let files = [(file.clone(), "CREATE TABLE x (id int); ROLLBACK;")];
    let error = apply(db.config(), &tree(&files)).await.unwrap_err();
    assert!(
        error
            .to_string()
            .contains("part of it may have been applied"),
        "{error}"
    );
    let ApplyError::Escaped { file: named, .. } = error else {
        panic!("expected an escaped transaction, got {error:?}");
    };
    assert_eq!(named, file);

    let client = connect(db.config()).await;
    assert!(versions(&client).await.is_empty());
    assert!(!exists(&client, "mintswarm.x").await);
    assert!(lock_is_free(db.config()).await);
}

#[tokio::test]
async fn a_commit_mid_file_gets_no_ledger_row() {
    let db = pg::database().await.unwrap();
    let file = up(V1, "commits");
    let files = [(
        file.clone(),
        "CREATE TABLE y (id int); COMMIT; CREATE TABLE z (id int);",
    )];
    let error = apply(db.config(), &tree(&files)).await.unwrap_err();
    let ApplyError::Escaped { file: named, .. } = error else {
        panic!("expected an escaped transaction, got {error:?}");
    };
    assert_eq!(named, file);

    let client = connect(db.config()).await;
    assert!(versions(&client).await.is_empty());
    assert!(exists(&client, "mintswarm.y").await);
    assert!(lock_is_free(db.config()).await);
}

#[tokio::test]
async fn a_commit_before_a_failure_is_not_reported_as_nothing_applied() {
    let db = pg::database().await.unwrap();
    let file = up(V1, "commits_then_fails");
    let files = [(file.clone(), "CREATE TABLE y (id int); COMMIT; SELECT 1/0;")];
    let error = apply(db.config(), &tree(&files)).await.unwrap_err();
    assert!(
        !error.to_string().contains("nothing of it was applied"),
        "{error}"
    );
    let ApplyError::Escaped { file: named, .. } = error else {
        panic!("expected an escaped transaction, got {error:?}");
    };
    assert_eq!(named, file);

    let client = connect(db.config()).await;
    assert!(versions(&client).await.is_empty());
    assert!(exists(&client, "mintswarm.y").await);
}
