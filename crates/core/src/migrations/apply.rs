//! Applying a checked tree to one database: a dedicated connection holding the migration
//! lock, the comparison with the ledger, and one transaction per file holding the file and its
//! ledger row.
//!
//! Until the first file's `BEGIN`, a run writes nothing but the idempotent ledger bootstrap.
//! There is no way to skip a file, force one or mark one as applied.

use std::fmt;

use tokio_postgres::{Client, Config, NoTls};

use super::ledger::{self, Drift, KIND_MIGRATION};
use super::narrowing::{self, Notice, Refusal};
use super::tree::{Migration, Report, TreeError, Warning, validate};

/// The session advisory lock key every run holds while it reads the ledger and applies files:
/// the ASCII bytes `mswmigra`. The leader lock and any other advisory lock must not reuse it.
pub const MIGRATION_LOCK_KEY: i64 = 0x6d73_776d_6967_7261;

/// One file a run applied.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Applied {
    /// The schema.
    pub schema: String,
    /// The version.
    pub version: String,
    /// The name.
    pub name: String,
    /// The up file, relative to the tree root.
    pub file: String,
}

/// Something a run noticed and went on despite.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ApplyWarning {
    /// A warning of the tree check.
    Tree(Warning),
    /// The ledger records a migration whose version has no file on disk; its row is kept.
    Orphan {
        /// The recorded schema.
        schema: String,
        /// The recorded version.
        version: String,
        /// The recorded name.
        name: String,
    },
    /// The interim narrowing rule's advisory on a pending file.
    Advisory(Notice),
}

impl fmt::Display for ApplyWarning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Tree(warning) => write!(f, "{warning}"),
            Self::Orphan {
                schema,
                version,
                name,
            } => write!(
                f,
                "{schema} {version}_{name}: the ledger records this migration but no file has its version; its row is kept"
            ),
            Self::Advisory(notice) => write!(f, "{notice}"),
        }
    }
}

/// What a successful run did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Outcome {
    /// The files applied, in the order they ran; empty when nothing was pending.
    pub applied: Vec<Applied>,
    /// The warnings, also logged.
    pub warnings: Vec<ApplyWarning>,
}

/// Why a run stopped. Errors from other crates are kept as sources only.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ApplyError {
    /// The tree is refused by its check; nothing connected.
    #[error("the migration tree is refused, and nothing was applied:{}", lines(.errors))]
    Tree {
        /// Every error of the check.
        errors: Vec<TreeError>,
    },
    /// The dedicated connection could not be made.
    #[error("could not connect to the database to migrate it; nothing was applied")]
    Connect {
        /// The client's error.
        source: tokio_postgres::Error,
    },
    /// The migration lock could not be taken.
    #[error("could not take the migration lock; nothing was applied")]
    Lock {
        /// The client's error.
        source: tokio_postgres::Error,
    },
    /// The ledger could not be created or read.
    #[error("could not create or read the migration ledger; nothing was applied")]
    Ledger {
        /// The client's error.
        source: tokio_postgres::Error,
    },
    /// The tree no longer matches the ledger.
    #[error(
        "the tree does not match the migration ledger, and nothing was applied:{}",
        lines(.drifts)
    )]
    Drift {
        /// Every drift, in version order.
        drifts: Vec<Drift>,
    },
    /// The interim narrowing rule refuses the pending files.
    #[error(
        "the pending migrations are refused by the narrowing rule, and nothing was applied:{}",
        lines(.refusals)
    )]
    Narrowing {
        /// Every refusal, in chain order.
        refusals: Vec<Refusal>,
    },
    /// A pending file's schema could not be created.
    #[error("the schema {schema:?} could not be created, and no file was applied")]
    Schema {
        /// The schema.
        schema: String,
        /// The client's error.
        source: tokio_postgres::Error,
    },
    /// A file failed and was rolled back. The files before it stay applied.
    #[error("{file}: the migration failed, nothing of it was applied, and the run stopped")]
    Failed {
        /// The up file, relative to the tree root.
        file: String,
        /// The client's error.
        source: tokio_postgres::Error,
    },
    /// A file's `COMMIT` got no clear answer. The files before it stay applied.
    #[error(
        "{file}: the outcome of the migration's commit is unknown, and the run stopped; the \
         next run decides by its ledger row"
    )]
    OutcomeUnknown {
        /// The up file, relative to the tree root.
        file: String,
        /// The client's error.
        source: tokio_postgres::Error,
    },
    /// Every file went through, but the migration lock could not be released; it goes with
    /// the session.
    #[error("the migrations were applied, but the migration lock could not be released")]
    Unlock {
        /// The client's error, or `None` when the session did not hold the lock.
        source: Option<tokio_postgres::Error>,
    },
}

/// Each item on a line of its own.
fn lines<T: fmt::Display>(items: &[T]) -> String {
    items.iter().map(|item| format!("\n{item}")).collect()
}

/// Applies the tree, given as `(path relative to the tree root, bytes)` pairs, to the database
/// `config` names.
///
/// The tree is checked first. A dedicated connection then takes the migration lock, waiting
/// for any other run, creates the ledger if it is missing, and compares the tree with it: an
/// edited or renamed applied file, or a narrowing the interim rule refuses, stops the run
/// before anything is written. Each pending file then runs in its own transaction with its
/// ledger row, in version order, and the first failure stops the run.
pub async fn apply(config: &Config, tree: &[(&str, &[u8])]) -> Result<Outcome, ApplyError> {
    let report = validate(tree).map_err(|errors| ApplyError::Tree { errors })?;
    let mut warnings: Vec<ApplyWarning> = report
        .warnings
        .iter()
        .cloned()
        .map(ApplyWarning::Tree)
        .collect();

    let (client, connection) = config
        .connect(NoTls)
        .await
        .map_err(|source| ApplyError::Connect { source })?;
    let task = tokio::spawn(async move {
        // The connection ends when the client is dropped; the run reports its own errors.
        let _ = connection.await;
    });

    let result = match client
        .execute("SELECT pg_advisory_lock($1)", &[&MIGRATION_LOCK_KEY])
        .await
    {
        Err(source) => Err(ApplyError::Lock { source }),
        Ok(_) => {
            let (run, unlockable) = locked(&client, &report, tree, &mut warnings).await;
            let unlock = if unlockable {
                Some(unlock(&client).await)
            } else {
                None
            };
            match (run, unlock) {
                (Ok(_), Some(Err(error))) => Err(error),
                (run, _) => run,
            }
        }
    };
    drop(client);
    let _ = task.await;
    for warning in &warnings {
        tracing::warn!(%warning, "migration warning");
    }
    result.map(|applied| Outcome { applied, warnings })
}

/// Releases the migration lock. A `false` answer means the session did not hold it.
async fn unlock(client: &Client) -> Result<(), ApplyError> {
    let row = client
        .query_one("SELECT pg_advisory_unlock($1)", &[&MIGRATION_LOCK_KEY])
        .await
        .map_err(|source| ApplyError::Unlock {
            source: Some(source),
        })?;
    match row.try_get::<_, bool>(0) {
        Ok(true) => Ok(()),
        Ok(false) => Err(ApplyError::Unlock { source: None }),
        Err(source) => Err(ApplyError::Unlock {
            source: Some(source),
        }),
    }
}

/// The part of a run under the lock. Gives the applied files or the error, and whether the
/// session can still release the lock: not after a failed `ROLLBACK`, which may leave the
/// transaction open.
async fn locked(
    client: &Client,
    report: &Report,
    tree: &[(&str, &[u8])],
    warnings: &mut Vec<ApplyWarning>,
) -> (Result<Vec<Applied>, ApplyError>, bool) {
    let pending = match plan(client, report, tree, warnings).await {
        Ok(pending) => pending,
        Err(error) => return (Err(error), true),
    };
    if let Err(error) = create_schemas(client, &pending).await {
        return (Err(error), true);
    }
    let insert = ledger::insert_statement();
    let mut applied = Vec::new();
    for (migration, bytes) in pending {
        match run_file(client, &insert, migration, bytes).await {
            Ok(()) => applied.push(Applied {
                schema: migration.schema.clone(),
                version: migration.version.clone(),
                name: migration.name.clone(),
                file: migration.up.clone(),
            }),
            Err((error, unlockable)) => return (Err(error), unlockable),
        }
    }
    (Ok(applied), true)
}

/// Creates the ledger if it is missing, compares the tree with it and runs the interim rule
/// over the pending files. Gives the pending migrations with their bytes, in version order.
async fn plan<'a>(
    client: &Client,
    report: &'a Report,
    tree: &[(&'a str, &'a [u8])],
    warnings: &mut Vec<ApplyWarning>,
) -> Result<Vec<(&'a Migration, &'a [u8])>, ApplyError> {
    ledger::bootstrap(client)
        .await
        .map_err(|source| ApplyError::Ledger { source })?;
    let rows = ledger::read(client)
        .await
        .map_err(|source| ApplyError::Ledger { source })?;
    let comparison =
        ledger::compare(report, tree, &rows).map_err(|drifts| ApplyError::Drift { drifts })?;
    warnings.extend(comparison.orphans.iter().map(|row| ApplyWarning::Orphan {
        schema: row.schema.clone(),
        version: row.version.clone(),
        name: row.name.clone(),
    }));
    let pending: Vec<(&Migration, &[u8])> = comparison
        .pending
        .into_iter()
        .map(|migration| {
            let bytes = tree
                .iter()
                .find(|(path, _)| *path == migration.up)
                .map(|(_, bytes)| *bytes)
                .unwrap_or_default();
            (migration, bytes)
        })
        .collect();

    // The tree check refused every file that is not UTF-8.
    let texts: Vec<(&str, &str)> = pending
        .iter()
        .map(|(migration, bytes)| {
            (
                migration.up.as_str(),
                std::str::from_utf8(bytes).unwrap_or_default(),
            )
        })
        .collect();
    let verdict = narrowing::check(&texts);
    if !verdict.is_accepted() {
        return Err(ApplyError::Narrowing {
            refusals: verdict.refusals,
        });
    }
    warnings.extend(verdict.advisories.into_iter().map(ApplyWarning::Advisory));
    Ok(pending)
}

/// Creates each pending file's schema when it is missing, just before the first `BEGIN`.
async fn create_schemas(
    client: &Client,
    pending: &[(&Migration, &[u8])],
) -> Result<(), ApplyError> {
    let mut done: Vec<&str> = Vec::new();
    for (migration, _) in pending {
        let schema = migration.schema.as_str();
        if done.contains(&schema) {
            continue;
        }
        client
            .batch_execute(&format!("CREATE SCHEMA IF NOT EXISTS {}", quoted(schema)))
            .await
            .map_err(|source| ApplyError::Schema {
                schema: schema.to_owned(),
                source,
            })?;
        done.push(schema);
    }
    Ok(())
}

/// Runs one file and its ledger row in one transaction. On an error, gives it with whether the
/// session can still release the lock.
async fn run_file(
    client: &Client,
    insert: &str,
    migration: &Migration,
    bytes: &[u8],
) -> Result<(), (ApplyError, bool)> {
    let file = &migration.up;
    let body = async {
        client.batch_execute("BEGIN").await?;
        client
            .batch_execute(&format!(
                "SET LOCAL search_path TO {}",
                quoted(&migration.schema)
            ))
            .await?;
        // The tree check refused every file that is not UTF-8.
        client
            .batch_execute(std::str::from_utf8(bytes).unwrap_or_default())
            .await?;
        client
            .execute(
                insert,
                &[
                    &migration.schema,
                    &migration.version,
                    &migration.name,
                    &KIND_MIGRATION,
                    &ledger::digest(bytes),
                ],
            )
            .await?;
        Ok::<_, tokio_postgres::Error>(())
    };
    if let Err(source) = body.await {
        let rolled_back = client.batch_execute("ROLLBACK").await.is_ok();
        return Err((
            ApplyError::Failed {
                file: file.clone(),
                source,
            },
            rolled_back,
        ));
    }
    match client.batch_execute("COMMIT").await {
        Ok(()) => Ok(()),
        Err(source) if commit_rolled_back(client, &source) => Err((
            ApplyError::Failed {
                file: file.clone(),
                source,
            },
            true,
        )),
        Err(source) => Err((
            ApplyError::OutcomeUnknown {
                file: file.clone(),
                source,
            },
            true,
        )),
    }
}

/// Whether a `COMMIT` error means the transaction was rolled back: a database error of
/// severity `ERROR` on a session that is still open. Anything else, a closed connection, an
/// I/O error, `FATAL`, `PANIC` or a missing reply, leaves the outcome unknown.
fn commit_rolled_back(client: &Client, error: &tokio_postgres::Error) -> bool {
    !client.is_closed()
        && error
            .as_db_error()
            .and_then(|db| db.parsed_severity())
            .is_some_and(|severity| severity == tokio_postgres::error::Severity::Error)
}

/// A schema name as a quoted identifier.
fn quoted(identifier: &str) -> String {
    format!("\"{}\"", identifier.replace('"', "\"\""))
}
