//! The migration ledger: its names and shape, the read of its rows, and the comparison of a
//! checked tree with them before anything is written.
//!
//! The ledger is keyed by (schema, version), so a migration is applied once whatever its
//! position in the chain. A row records the up file's name, its kind and the sha256 of its
//! bytes; the floor columns stay empty until floors land.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use sha2::{Digest, Sha256};
use tokio_postgres::Client;

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

/// One ledger row, as far as the comparison needs it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LedgerRow {
    pub(crate) schema: String,
    pub(crate) version: String,
    pub(crate) name: String,
    pub(crate) kind: String,
    pub(crate) sha256: Vec<u8>,
}

/// Reads every ledger row, sorted by schema and version.
pub(crate) async fn read(client: &Client) -> Result<Vec<LedgerRow>, tokio_postgres::Error> {
    let rows = client
        .query(
            &format!(
                "SELECT schema, version, name, kind, sha256 FROM {LEDGER_SCHEMA}.{LEDGER_TABLE} \
                 ORDER BY schema, version"
            ),
            &[],
        )
        .await?;
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

/// What the ledger leaves to do for a tree that matches it.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Comparison<'tree, 'rows> {
    /// The migrations with no ledger row, in the report's version order.
    pub(crate) pending: Vec<&'tree Migration>,
    /// The rows whose version has no file on disk.
    pub(crate) orphans: Vec<&'rows LedgerRow>,
}

/// The sha256 of a file's bytes.
pub(crate) fn digest(bytes: &[u8]) -> Vec<u8> {
    Sha256::digest(bytes).to_vec()
}

/// Compares a checked tree with the ledger's rows.
///
/// `tree` holds the bytes `report` was checked from. Gives the pending migrations and the
/// orphan rows, or every drift at once, in version order.
pub(crate) fn compare<'tree, 'rows>(
    report: &'tree Report,
    tree: &[(&str, &[u8])],
    rows: &'rows [LedgerRow],
) -> Result<Comparison<'tree, 'rows>, Vec<Drift>> {
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
        .collect();
    Ok(Comparison { pending, orphans })
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
        let comparison = compare(&report, tree, &[]).unwrap();
        let pending: Vec<&str> = comparison.pending.iter().map(|m| m.up.as_str()).collect();
        assert_eq!(pending, [A, B]);
        assert!(comparison.orphans.is_empty());
    }

    #[test]
    fn every_drift_is_reported() {
        let tree: &[(&str, &[u8])] = &[(A, b"SELECT 10"), (B, b"SELECT 2")];
        let report = validate(tree).unwrap();
        let rows = [
            row("20260101000000", "a", KIND_MIGRATION, b"SELECT 1"),
            row("20260102000000", "old_b", KIND_MIGRATION, b"SELECT 2"),
        ];
        let drifts = compare(&report, tree, &rows).unwrap_err();
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
        let drifts = compare(&report, tree, &rows).unwrap_err();
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
        let comparison = compare(&report, tree, &rows).unwrap();
        assert!(comparison.pending.is_empty());
        let orphans: Vec<&str> = comparison
            .orphans
            .iter()
            .map(|row| row.version.as_str())
            .collect();
        assert_eq!(orphans, ["20260102000000"]);
    }
}
