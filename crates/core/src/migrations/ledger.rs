//! The migration ledger: its names and shape, the read of its rows in one read-only snapshot,
//! and the planner, which gives the pending migrations of a checked tree from a set of applied
//! rows before anything is written.
//!
//! The ledger is keyed by (schema, version), so a migration is applied once whatever its
//! position in the chain. A row records the up file's name, its kind and the sha256 of its
//! bytes; the floor columns stay empty until floors land. [`plan`] is the one definition of
//! what is pending: [`apply`](super::apply) runs it under its lock, [`plan_database`] runs it on
//! a database's ledger without writing, and a caller without a database passes its own rows.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use sha2::{Digest, Sha256};
use tokio_postgres::{Client, IsolationLevel};

use super::tree::{Migration, Report};

/// The schema that holds the ledger, apart from every schema the files create objects in.
pub const LEDGER_SCHEMA: &str = "mintswarm_migrations";

/// The ledger table in [`LEDGER_SCHEMA`].
pub const LEDGER_TABLE: &str = "ledger";

/// The kind of a row written for an up file of the tree.
pub(crate) const KIND_MIGRATION: &str = "migration";

/// Creates the ledger's schema and table when they are missing. Idempotent; it runs only under
/// the migration lock, so two runs never race on the catalogue.
pub(crate) async fn bootstrap(client: &Client) -> Result<(), tokio_postgres::Error> {
    client
        .batch_execute(&format!(
            "CREATE SCHEMA IF NOT EXISTS {LEDGER_SCHEMA};
             CREATE TABLE IF NOT EXISTS {LEDGER_SCHEMA}.{LEDGER_TABLE} (
                 schema text NOT NULL,
                 version text NOT NULL,
                 name text NOT NULL,
                 kind text NOT NULL,
                 sha256 bytea NOT NULL,
                 applied_at timestamptz NOT NULL DEFAULT now(),
                 compacted boolean NOT NULL DEFAULT false,
                 floor_change bigint,
                 floor_commit text,
                 floor_source text,
                 PRIMARY KEY (schema, version)
             )"
        ))
        .await
}

/// The statement that records an applied file, with the parameters schema, version, name,
/// kind and sha256.
pub(crate) fn insert_statement() -> String {
    format!(
        "INSERT INTO {LEDGER_SCHEMA}.{LEDGER_TABLE} (schema, version, name, kind, sha256) \
         VALUES ($1, $2, $3, $4, $5)"
    )
}

/// One ledger row, as the planner reads it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LedgerRow {
    /// The schema.
    pub schema: String,
    /// The version.
    pub version: String,
    /// The up file's name after the version.
    pub name: String,
    /// The kind of the row, such as `migration`.
    pub kind: String,
    /// The sha256 of the up file's bytes.
    pub sha256: Vec<u8>,
}

/// Reads every ledger row, sorted by schema and version, in one read-only `REPEATABLE READ`
/// snapshot. A missing ledger gives no rows, and nothing is created.
///
/// The transaction rolls back when it is dropped, so an early return leaves the client outside
/// a transaction.
pub(crate) async fn read(client: &mut Client) -> Result<Vec<LedgerRow>, tokio_postgres::Error> {
    let transaction = client
        .build_transaction()
        .isolation_level(IsolationLevel::RepeatableRead)
        .read_only(true)
        .start()
        .await?;
    let exists: bool = transaction
        .query_one(
            &format!("SELECT to_regclass('{LEDGER_SCHEMA}.{LEDGER_TABLE}') IS NOT NULL"),
            &[],
        )
        .await?
        .try_get(0)?;
    let rows = if exists {
        transaction
            .query(
                &format!(
                    "SELECT schema, version, name, kind, sha256 FROM {LEDGER_SCHEMA}.{LEDGER_TABLE} \
                     ORDER BY schema, version"
                ),
                &[],
            )
            .await?
    } else {
        Vec::new()
    };
    transaction.commit().await?;
    rows.iter()
        .map(|row| {
            Ok(LedgerRow {
                schema: row.try_get(0)?,
                version: row.try_get(1)?,
                name: row.try_get(2)?,
                kind: row.try_get(3)?,
                sha256: row.try_get(4)?,
            })
        })
        .collect()
}

/// Why the tree on disk no longer matches what the ledger records as applied.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Drift {
    /// An applied file's bytes changed since it was applied.
    Edited {
        /// The up file, relative to the tree root.
        file: String,
    },
    /// The ledger records the file's version under another name, or as another kind.
    Renamed {
        /// The up file on disk, relative to the tree root.
        file: String,
        /// The name the ledger records for the version.
        recorded_name: String,
        /// The kind the ledger records for the version.
        recorded_kind: String,
    },
}

impl fmt::Display for Drift {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Edited { file } => write!(
                f,
                "{file}: the file changed after it was applied; its sha256 differs from the ledger's"
            ),
            Self::Renamed {
                file,
                recorded_name,
                recorded_kind,
            } => write!(
                f,
                "{file}: the ledger records this version as {recorded_kind} {recorded_name:?}; \
                 an applied migration cannot be renamed or change its kind"
            ),
        }
    }
}

/// What a set of applied rows leaves to do for a tree that matches it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan<'tree> {
    /// The migrations with no row, in the order they are applied: the report's version order.
    pub pending: Vec<&'tree Migration>,
    /// The rows whose version has no file in the tree.
    pub orphans: Vec<LedgerRow>,
}

/// Why the plan of a database could not be made.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum PlanError {
    /// The ledger could not be read.
    #[error("could not read the migration ledger")]
    Ledger {
        /// The client's error.
        source: tokio_postgres::Error,
    },
    /// The tree no longer matches the ledger.
    #[error("the tree does not match the migration ledger:{}", lines(.drifts))]
    Drift {
        /// Every drift, in version order.
        drifts: Vec<Drift>,
    },
}

/// Each item on a line of its own.
pub(super) fn lines<T: fmt::Display>(items: &[T]) -> String {
    items.iter().map(|item| format!("\n{item}")).collect()
}

/// The sha256 of a file's bytes.
pub(crate) fn digest(bytes: &[u8]) -> Vec<u8> {
    Sha256::digest(bytes).to_vec()
}

/// The rows [`apply`](super::apply) records for every migration of a checked tree, in version
/// order: the applied set of a tree such as a merge base, without a database.
///
/// `tree` holds the bytes `report` was checked from.
pub fn recorded(report: &Report, tree: &[(&str, &[u8])]) -> Vec<LedgerRow> {
    let bytes: BTreeMap<&str, &[u8]> = tree.iter().copied().collect();
    report
        .migrations
        .iter()
        .map(|migration| LedgerRow {
            schema: migration.schema.clone(),
            version: migration.version.clone(),
            name: migration.name.clone(),
            kind: KIND_MIGRATION.to_owned(),
            sha256: digest(
                bytes
                    .get(migration.up.as_str())
                    .copied()
                    .unwrap_or_default(),
            ),
        })
        .collect()
}

/// Plans a checked tree against a set of applied rows: the one definition of what is pending.
///
/// `tree` holds the bytes `report` was checked from, and `rows` the applied set: the ledger's
/// rows, none for an empty database, or [`recorded`] for another tree. Gives the pending
/// migrations in the order they are applied and the orphan rows, or every drift at once, in
/// version order. It reads and writes nothing.
pub fn plan<'tree>(
    report: &'tree Report,
    tree: &[(&str, &[u8])],
    rows: &[LedgerRow],
) -> Result<Plan<'tree>, Vec<Drift>> {
    let bytes: BTreeMap<&str, &[u8]> = tree.iter().copied().collect();
    let recorded: BTreeMap<(&str, &str), &LedgerRow> = rows
        .iter()
        .map(|row| ((row.schema.as_str(), row.version.as_str()), row))
        .collect();

    let mut drifts = Vec::new();
    let mut pending = Vec::new();
    for migration in &report.migrations {
        let key = (migration.schema.as_str(), migration.version.as_str());
        let Some(row) = recorded.get(&key) else {
            pending.push(migration);
            continue;
        };
        if row.name != migration.name || row.kind != KIND_MIGRATION {
            drifts.push(Drift::Renamed {
                file: migration.up.clone(),
                recorded_name: row.name.clone(),
                recorded_kind: row.kind.clone(),
            });
            continue;
        }
        let file = bytes
            .get(migration.up.as_str())
            .copied()
            .unwrap_or_default();
        if digest(file) != row.sha256 {
            drifts.push(Drift::Edited {
                file: migration.up.clone(),
            });
        }
    }
    if !drifts.is_empty() {
        return Err(drifts);
    }

    let on_disk: BTreeSet<(&str, &str)> = report
        .migrations
        .iter()
        .map(|migration| (migration.schema.as_str(), migration.version.as_str()))
        .collect();
    let orphans = rows
        .iter()
        .filter(|row| !on_disk.contains(&(row.schema.as_str(), row.version.as_str())))
        .cloned()
        .collect();
    Ok(Plan { pending, orphans })
}

/// Plans a checked tree against the ledger of the database `client` is connected to, without
/// writing anything.
///
/// The ledger is read in one read-only snapshot, and a missing ledger counts as no rows. It
/// takes no advisory lock, so it never waits for a running [`apply`](super::apply), and it
/// never creates the ledger. For a database known to be empty, a caller passes no rows to
/// [`plan`] instead and makes no connection.
pub async fn plan_database<'t>(
    client: &mut Client,
    report: &'t Report,
    tree: &[(&str, &[u8])],
) -> Result<Plan<'t>, PlanError> {
    let rows = read(client)
        .await
        .map_err(|source| PlanError::Ledger { source })?;
    plan(report, tree, &rows).map_err(|drifts| PlanError::Drift { drifts })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::migrations::validate;

    const A: &str = "mintswarm/migrations/20260101000000_a.up.sql";
    const B: &str = "mintswarm/migrations/20260102000000_b.up.sql";

    fn row(version: &str, name: &str, kind: &str, bytes: &[u8]) -> LedgerRow {
        LedgerRow {
            schema: "mintswarm".to_owned(),
            version: version.to_owned(),
            name: name.to_owned(),
            kind: kind.to_owned(),
            sha256: digest(bytes),
        }
    }

    #[test]
    fn unrecorded_files_are_pending_in_version_order() {
        let tree: &[(&str, &[u8])] = &[(B, b"SELECT 2"), (A, b"SELECT 1")];
        let report = validate(tree).unwrap();
        let plan = plan(&report, tree, &[]).unwrap();
        let pending: Vec<&str> = plan.pending.iter().map(|m| m.up.as_str()).collect();
        assert_eq!(pending, [A, B]);
        assert!(plan.orphans.is_empty());
    }

    #[test]
    fn every_drift_is_reported() {
        let tree: &[(&str, &[u8])] = &[(A, b"SELECT 10"), (B, b"SELECT 2")];
        let report = validate(tree).unwrap();
        let rows = [
            row("20260101000000", "a", KIND_MIGRATION, b"SELECT 1"),
            row("20260102000000", "old_b", KIND_MIGRATION, b"SELECT 2"),
        ];
        let drifts = plan(&report, tree, &rows).unwrap_err();
        assert_eq!(
            drifts,
            [
                Drift::Edited { file: A.to_owned() },
                Drift::Renamed {
                    file: B.to_owned(),
                    recorded_name: "old_b".to_owned(),
                    recorded_kind: KIND_MIGRATION.to_owned(),
                },
            ]
        );
    }

    #[test]
    fn another_kind_is_a_rename() {
        let tree: &[(&str, &[u8])] = &[(A, b"SELECT 1")];
        let report = validate(tree).unwrap();
        let rows = [row("20260101000000", "a", "compaction", b"SELECT 1")];
        let drifts = plan(&report, tree, &rows).unwrap_err();
        assert!(matches!(drifts.as_slice(), [Drift::Renamed { file, .. }] if file == A));
    }

    #[test]
    fn rows_without_files_are_orphans() {
        let tree: &[(&str, &[u8])] = &[(A, b"SELECT 1")];
        let report = validate(tree).unwrap();
        let rows = [
            row("20260101000000", "a", KIND_MIGRATION, b"SELECT 1"),
            row("20260102000000", "b", KIND_MIGRATION, b"SELECT 2"),
        ];
        let plan = plan(&report, tree, &rows).unwrap();
        assert!(plan.pending.is_empty());
        let orphans: Vec<&str> = plan
            .orphans
            .iter()
            .map(|row| row.version.as_str())
            .collect();
        assert_eq!(orphans, ["20260102000000"]);
    }
}
