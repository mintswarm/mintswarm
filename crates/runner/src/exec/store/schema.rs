//! The database's migrations: an append-only list of SQL scripts, tracked by
//! `PRAGMA user_version`.
//!
//! Migration 1 creates the tables of design §12 with the sketch's columns. It adds only column
//! types (the tables are `STRICT`), primary and unique keys, `NOT NULL` on the columns the
//! store's own code reads, and `CHECK` lists for the outbox's `kind` and `state` and the
//! records' `kind`. Every `*_at` column holds integer unix milliseconds, and every JSON column
//! is `TEXT`.
//!
//! Migration 2 adds `unrecognised_failures`, beyond design §12: the failures of an outside
//! command, such as `git fetch`, that the runner's classifier did not recognise, counted per
//! adapter and signature for a later step to learn from.
//!
//! Migration 3 renames `jobs.component` to `components`, beyond design §12: an issue carries one
//! or more components, held as a JSON array of names in `TEXT`; a name already there becomes a
//! one-name array.

use rusqlite::{Connection, TransactionBehavior};

use super::{StoreError, sql};

/// The migrations, in order; migration `n` is `MIGRATIONS[n - 1]`. Only ever append to it.
pub(super) const MIGRATIONS: &[&str] = &[MIGRATION_1, MIGRATION_2, MIGRATION_3];

/// The schema version a fully migrated database has.
pub const SCHEMA_VERSION: i64 = MIGRATIONS.len() as i64;

/// The tables of design §12.
const MIGRATION_1: &str = r#"
CREATE TABLE jobs (
    job_id TEXT NOT NULL PRIMARY KEY,
    issue INTEGER,
    kind TEXT,
    component TEXT,
    state TEXT,
    paused INTEGER,
    base_commit TEXT,
    branch TEXT,
    pr INTEGER,
    plan_sha256 TEXT,
    approval TEXT,
    counters TEXT,
    row_version INTEGER
) STRICT;

CREATE TABLE transitions (
    job_id TEXT NOT NULL,
    seq INTEGER NOT NULL,
    from_state TEXT,
    event TEXT,
    guard_inputs TEXT,
    to_state TEXT,
    at INTEGER,
    PRIMARY KEY (job_id, seq)
) STRICT;

CREATE TABLE poll_cursors (
    url TEXT NOT NULL PRIMARY KEY,
    etag TEXT,
    last_modified TEXT,
    cursor TEXT,
    polled_at INTEGER
) STRICT;

CREATE TABLE attempts (
    attempt_id TEXT NOT NULL PRIMARY KEY,
    job_id TEXT,
    step TEXT,
    n INTEGER,
    harness TEXT,
    model TEXT,
    effort TEXT,
    instructions_commit TEXT,
    instructions_sha256 TEXT,
    session_id TEXT,
    pgid INTEGER,
    started_at INTEGER,
    ended_at INTEGER,
    outcome TEXT,
    output_sha256 TEXT,
    transcript_blob TEXT
) STRICT;

CREATE TABLE leases (
    resource TEXT NOT NULL PRIMARY KEY,
    holder TEXT NOT NULL,
    expires_at INTEGER NOT NULL
) STRICT;

CREATE TABLE inputs (
    job_id TEXT NOT NULL,
    attempt_id TEXT NOT NULL,
    path TEXT NOT NULL,
    sha256 TEXT,
    source TEXT,
    PRIMARY KEY (job_id, attempt_id, path)
) STRICT;

CREATE TABLE records (
    job_id TEXT NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN (
        'floor', 'decision', 'round', 'facts', 'landing_floor', 'landing_decision',
        'bundle', 'packages', 'prefetch', 'ctrf', 'merge_ready'
    )),
    version INTEGER NOT NULL,
    sha256 TEXT,
    blob TEXT,
    bound TEXT,
    PRIMARY KEY (job_id, kind, version)
) STRICT;

CREATE TABLE rounds (
    job_id TEXT NOT NULL,
    kind TEXT NOT NULL,
    n INTEGER NOT NULL,
    head TEXT,
    round_sha256 TEXT,
    verdict TEXT,
    PRIMARY KEY (job_id, kind, n)
) STRICT;

CREATE TABLE artefacts (
    job_id TEXT NOT NULL,
    name TEXT NOT NULL,
    revision INTEGER NOT NULL,
    sha256 TEXT,
    blob TEXT,
    published_url TEXT,
    PRIMARY KEY (job_id, name, revision)
) STRICT;

CREATE TABLE outbox (
    effect_id INTEGER PRIMARY KEY AUTOINCREMENT,
    job_id TEXT NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN (
        'comment', 'artifact', 'assign', 'release', 'review_request', 'open_pr',
        'mark_ready', 'set_issue_type', 'file_child', 'push', 'link_event'
    )),
    effect_key TEXT NOT NULL,
    payload_blob TEXT NOT NULL,
    state TEXT NOT NULL CHECK (state IN ('pending', 'sent', 'acked', 'failed', 'suppressed')),
    attempts INTEGER NOT NULL DEFAULT 0,
    next_at INTEGER,
    result TEXT,
    UNIQUE (job_id, effect_key)
) STRICT;

CREATE TABLE snapshots (
    "commit" TEXT NOT NULL PRIMARY KEY,
    path TEXT,
    refcount INTEGER
) STRICT;

CREATE TABLE worktrees (
    job_id TEXT NOT NULL PRIMARY KEY,
    path TEXT,
    start_commit TEXT,
    head TEXT,
    pushed_head TEXT
) STRICT;

CREATE TABLE probes (
    id TEXT NOT NULL PRIMARY KEY,
    result TEXT,
    source TEXT,
    observed_at INTEGER
) STRICT;

CREATE TABLE usage (
    attempt_id TEXT NOT NULL,
    seq INTEGER NOT NULL,
    tokens TEXT,
    cost REAL,
    limits TEXT,
    observed_at INTEGER,
    PRIMARY KEY (attempt_id, seq)
) STRICT;

CREATE TABLE shadow (
    job_id TEXT NOT NULL,
    item TEXT NOT NULL,
    ours_sha256 TEXT,
    reference_sha256 TEXT,
    field_diffs TEXT,
    PRIMARY KEY (job_id, item)
) STRICT;
"#;

/// The failures the runner's classifiers did not recognise, beyond design §12: one row per
/// adapter and signature, with the latest redacted excerpt and status, and how often and when
/// it was seen.
const MIGRATION_2: &str = r#"
CREATE TABLE unrecognised_failures (
    adapter TEXT NOT NULL,
    signature TEXT NOT NULL,
    excerpt TEXT NOT NULL,
    status TEXT NOT NULL,
    count INTEGER NOT NULL,
    first_seen_at INTEGER NOT NULL,
    last_seen_at INTEGER NOT NULL,
    PRIMARY KEY (adapter, signature)
) STRICT;
"#;

/// `jobs.component` becomes `components`, a JSON array of names.
const MIGRATION_3: &str = r#"
ALTER TABLE jobs RENAME COLUMN component TO components;
UPDATE jobs SET components = json_array(components) WHERE components IS NOT NULL;
"#;

/// Apply the migrations `conn` has not had, in one transaction, and set its `user_version`.
///
/// A database already at [`SCHEMA_VERSION`] is left unchanged. One with a newer version is
/// refused with [`StoreError::SchemaTooNew`], and one with a negative version with
/// [`StoreError::SchemaInvalid`].
pub(super) fn migrate(conn: &mut Connection) -> Result<(), StoreError> {
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(sql("cannot begin the migration"))?;
    let found: i64 = tx
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(sql("cannot read the schema version"))?;
    if found < 0 {
        return Err(StoreError::SchemaInvalid(found));
    }
    if found > SCHEMA_VERSION {
        return Err(StoreError::SchemaTooNew {
            found,
            known: SCHEMA_VERSION,
        });
    }
    if found == SCHEMA_VERSION {
        return Ok(());
    }
    // `found` is between 0 and SCHEMA_VERSION here, so it indexes the list.
    let applied = usize::try_from(found).map_err(|_| StoreError::SchemaInvalid(found))?;
    for (index, script) in MIGRATIONS.iter().enumerate().skip(applied) {
        tx.execute_batch(script)
            .map_err(sql(&format!("cannot apply migration {}", index + 1)))?;
    }
    tx.pragma_update(None, "user_version", SCHEMA_VERSION)
        .map_err(sql("cannot set the schema version"))?;
    tx.commit().map_err(sql("cannot commit the migration"))
}
