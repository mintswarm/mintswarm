//! Applying a checked tree to one database: a dedicated connection holding the migration
//! lock, the plan against the ledger, and each pending file in version order. A transactional
//! file runs in one transaction with its ledger row. A file with the no-transaction header runs
//! statement by statement outside a transaction, and its ledger row is written after its last
//! statement succeeded.
//!
//! The lock is taken by polling, so that no statement is open while a run waits: a waiting
//! `pg_advisory_lock` would hold a snapshot that a running `CREATE INDEX CONCURRENTLY` waits
//! for. Until the first file runs, a run writes nothing but the idempotent ledger bootstrap.
//! There is no way to skip a file, force one or mark one as applied.

use std::fmt;
use std::time::Duration;

use tokio_postgres::{Client, Config, NoTls};

use super::ledger::{self, Drift, KIND_MIGRATION};
use super::narrowing::{self, Notice, Refusal};
use super::sql::{self, Token};
use super::tree::{Migration, Report, TreeError, Warning, validate};

/// The session advisory lock key every run holds while it reads the ledger and applies files:
/// the ASCII bytes `mswmigra`. The leader lock and any other advisory lock must not reuse it.
pub const MIGRATION_LOCK_KEY: i64 = 0x6d73_776d_6967_7261;

/// How long a run waits between tries for the migration lock.
const LOCK_RETRY: Duration = Duration::from_millis(100);

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
    /// A file holds its own transaction control, which may have ended the runner's
    /// transaction, so the file may not have run as one transaction with its ledger row. Its
    /// ledger row is not written, and the files before it stay applied.
    #[error(
        "{file}: the migration holds its own transaction control, which may have ended the \
         runner's transaction, so part of it may have been applied; no ledger row was written \
         for it, and the run stopped"
    )]
    Escaped {
        /// The up file, relative to the tree root.
        file: String,
        /// The client's error, when a statement of the file failed.
        source: Option<tokio_postgres::Error>,
    },
    /// A file's `COMMIT`, or the ledger insert after a no-transaction file, got no clear
    /// answer, so whether the file's ledger row was written is unknown. The files before it
    /// stay applied.
    #[error(
        "{file}: whether the migration and its ledger row were committed is unknown, and the \
         run stopped; the next run decides by its ledger row"
    )]
    OutcomeUnknown {
        /// The up file, relative to the tree root.
        file: String,
        /// The client's error.
        source: tokio_postgres::Error,
    },
    /// A file with the no-transaction header stopped; the reason says how much of it ran. Its
    /// ledger row is not written, and the files before it stay applied.
    #[error("{file}: {reason}; no ledger row was written for it, and the run stopped")]
    OutsideTransaction {
        /// The up file, relative to the tree root.
        file: String,
        /// Where the file stopped.
        reason: OutsideFailure,
        /// The client's error, when a statement failed.
        source: Option<tokio_postgres::Error>,
    },
    /// Every file went through, but the migration lock could not be released; it goes with
    /// the session.
    #[error("the migrations were applied, but the migration lock could not be released")]
    Unlock {
        /// The client's error, or `None` when the session did not hold the lock.
        source: Option<tokio_postgres::Error>,
    },
}

/// Where a file with the no-transaction header stopped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum OutsideFailure {
    /// The file holds its own transaction control, or cannot be split; none of it ran.
    TransactionControl,
    /// A statement failed; the statements before it stay applied.
    Statement {
        /// The line of the failing statement, counted from 1.
        line: usize,
    },
    /// Every statement ran, but the session's search path could not be reset.
    Reset,
    /// Every statement ran, but the ledger refused the file's row.
    Record,
}

impl fmt::Display for OutsideFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TransactionControl => write!(
                f,
                "the migration runs outside a transaction but holds its own transaction control, \
                 so none of it was run"
            ),
            Self::Statement { line } => write!(
                f,
                "the statement at line {line} failed outside a transaction, so part of the \
                 migration may have been applied"
            ),
            Self::Reset => write!(
                f,
                "every statement of the migration ran, but the search path could not be reset"
            ),
            Self::Record => write!(
                f,
                "every statement of the migration ran, but its ledger row could not be written"
            ),
        }
    }
}

/// Each item on a line of its own.
fn lines<T: fmt::Display>(items: &[T]) -> String {
    items.iter().map(|item| format!("\n{item}")).collect()
}

/// Applies the tree, given as `(path relative to the tree root, bytes)` pairs, to the database
/// `config` names.
///
/// The tree is checked first. A dedicated connection then takes the migration lock, polling
/// every 100 ms while another run holds it, creates the ledger if it is missing, and plans the
/// tree against it: an edited or renamed applied file, or a narrowing the interim rule refuses,
/// stops the run before anything is written. The pending files then run in version order, and
/// the first failure stops the run. A transactional file runs in one transaction with its
/// ledger row; a file with the no-transaction header runs statement by statement outside a
/// transaction, and its ledger row is written after it.
pub async fn apply(config: &Config, tree: &[(&str, &[u8])]) -> Result<Outcome, ApplyError> {
    let report = validate(tree).map_err(|errors| ApplyError::Tree { errors })?;
    let mut warnings: Vec<ApplyWarning> = report
        .warnings
        .iter()
        .cloned()
        .map(ApplyWarning::Tree)
        .collect();

    let (mut client, connection) = config
        .connect(NoTls)
        .await
        .map_err(|source| ApplyError::Connect { source })?;
    let task = tokio::spawn(async move {
        // The connection ends when the client is dropped; the run reports its own errors.
        let _ = connection.await;
    });

    let result = match lock(&client).await {
        Err(source) => Err(ApplyError::Lock { source }),
        Ok(_) => {
            let (run, unlockable) = locked(&mut client, &report, tree, &mut warnings).await;
            let unlock = if unlockable {
                Some(unlock(&client).await)
            } else {
                None
            };
            match (run, unlock) {
                (Ok(_), Some(Err(error))) => Err(error),
                (Err(error), Some(Err(unlock))) => {
                    tracing::warn!(
                        error = %unlock,
                        "after a failed migration run the lock could not be released; it goes with the session"
                    );
                    Err(error)
                }
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

/// Takes the migration lock, trying again every [`LOCK_RETRY`] while another session holds it.
/// No statement is open between tries.
async fn lock(client: &Client) -> Result<(), tokio_postgres::Error> {
    loop {
        let taken: bool = client
            .query_one("SELECT pg_try_advisory_lock($1)", &[&MIGRATION_LOCK_KEY])
            .await?
            .try_get(0)?;
        if taken {
            return Ok(());
        }
        tokio::time::sleep(LOCK_RETRY).await;
    }
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
    client: &mut Client,
    report: &Report,
    tree: &[(&str, &[u8])],
    warnings: &mut Vec<ApplyWarning>,
) -> (Result<Vec<Applied>, ApplyError>, bool) {
    let pending = match prepare(client, report, tree, warnings).await {
        Ok(pending) => pending,
        Err(error) => return (Err(error), true),
    };
    if let Err(error) = create_schemas(client, &pending).await {
        return (Err(error), true);
    }
    let insert = ledger::insert_statement();
    let mut applied = Vec::new();
    for (migration, bytes) in pending {
        let run = if migration.no_transaction {
            run_outside(client, &insert, migration, bytes).await
        } else {
            run_file(client, &insert, migration, bytes).await
        };
        match run {
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

/// Creates the ledger if it is missing, plans the tree against it and runs the interim rule
/// over the pending files. Gives the pending migrations with their bytes, in version order.
async fn prepare<'a>(
    client: &mut Client,
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
    let plan = ledger::plan(report, tree, &rows).map_err(|drifts| ApplyError::Drift { drifts })?;
    warnings.extend(plan.orphans.into_iter().map(|row| ApplyWarning::Orphan {
        schema: row.schema,
        version: row.version,
        name: row.name,
    }));
    let pending: Vec<(&Migration, &[u8])> = plan
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
///
/// The transaction's id is read before and after the file: a file whose own transaction
/// control ended the runner's transaction gets no ledger row.
async fn run_file(
    client: &Client,
    insert: &str,
    migration: &Migration,
    bytes: &[u8],
) -> Result<(), (ApplyError, bool)> {
    let file = &migration.up;
    // The tree check refused every file that is not UTF-8.
    let text = std::str::from_utf8(bytes).unwrap_or_default();
    let failed = |source| ApplyError::Failed {
        file: file.clone(),
        source,
    };
    let escaped = |source| ApplyError::Escaped {
        file: file.clone(),
        source,
    };

    let txid = match open(client, &migration.schema).await {
        Ok(txid) => txid,
        Err(source) => return Err(abandon(client, failed(source)).await),
    };
    if let Err(source) = client.batch_execute(text).await {
        let rolled_back = client.batch_execute("ROLLBACK").await.is_ok();
        let error = if has_transaction_control(text) || committed(client, txid).await {
            escaped(Some(source))
        } else {
            failed(source)
        };
        return Err((error, rolled_back));
    }
    match current_txid(client).await {
        Ok(now) if now == txid => {}
        Ok(_) => return Err(abandon(client, escaped(None)).await),
        Err(source) if has_transaction_control(text) => {
            return Err(abandon(client, escaped(Some(source))).await);
        }
        Err(source) => return Err(abandon(client, failed(source)).await),
    }
    let recorded = client
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
        .await;
    if let Err(source) = recorded {
        return Err(abandon(client, failed(source)).await);
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

/// Runs one file with the no-transaction header: each statement on its own, outside a
/// transaction, in the file's schema, and then its ledger row. On an error, gives it with
/// whether the session can still release the lock, which it always can.
///
/// A file holding its own transaction control is refused before any statement runs.
async fn run_outside(
    client: &Client,
    insert: &str,
    migration: &Migration,
    bytes: &[u8],
) -> Result<(), (ApplyError, bool)> {
    let file = &migration.up;
    // The tree check refused every file that is not UTF-8.
    let text = std::str::from_utf8(bytes).unwrap_or_default();
    let outside = |reason, source| {
        (
            ApplyError::OutsideTransaction {
                file: file.clone(),
                reason,
                source,
            },
            true,
        )
    };
    let statements = match sql::split(text) {
        Ok(statements) if !has_transaction_control(text) => statements,
        _ => return Err(outside(OutsideFailure::TransactionControl, None)),
    };
    if let Err(source) = client
        .batch_execute(&format!("SET search_path TO {}", quoted(&migration.schema)))
        .await
    {
        return Err((
            ApplyError::Failed {
                file: file.clone(),
                source,
            },
            true,
        ));
    }
    for statement in &statements {
        let source_text = text.get(statement.span.clone()).unwrap_or_default();
        if let Err(source) = client.batch_execute(source_text).await {
            let _ = client.batch_execute("RESET search_path").await;
            return Err(outside(
                OutsideFailure::Statement {
                    line: statement.line,
                },
                Some(source),
            ));
        }
    }
    if let Err(source) = client.batch_execute("RESET search_path").await {
        return Err(outside(OutsideFailure::Reset, Some(source)));
    }
    let recorded = client
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
        .await;
    match recorded {
        Ok(_) => Ok(()),
        Err(source) if commit_rolled_back(client, &source) => {
            Err(outside(OutsideFailure::Record, Some(source)))
        }
        Err(source) => Err((
            ApplyError::OutcomeUnknown {
                file: file.clone(),
                source,
            },
            true,
        )),
    }
}

/// Opens the file's transaction in its schema and gives the transaction's id.
async fn open(client: &Client, schema: &str) -> Result<i64, tokio_postgres::Error> {
    client.batch_execute("BEGIN").await?;
    client
        .batch_execute(&format!("SET LOCAL search_path TO {}", quoted(schema)))
        .await?;
    current_txid(client).await
}

/// The id of the session's current transaction. Outside a transaction block, each call runs in
/// a transaction of its own and so gives a new id.
async fn current_txid(client: &Client) -> Result<i64, tokio_postgres::Error> {
    client
        .query_one("SELECT txid_current()", &[])
        .await?
        .try_get(0)
}

/// Whether the transaction `txid` committed, as far as the server can still tell.
async fn committed(client: &Client, txid: i64) -> bool {
    client
        .query_one("SELECT txid_status($1)", &[&txid])
        .await
        .and_then(|row| row.try_get::<_, Option<String>>(0))
        .is_ok_and(|status| status.as_deref() == Some("committed"))
}

/// Rolls the file's transaction back after `error`, and gives the error with whether the
/// rollback succeeded, and so whether the session can still release the lock.
async fn abandon(client: &Client, error: ApplyError) -> (ApplyError, bool) {
    let rolled_back = client.batch_execute("ROLLBACK").await.is_ok();
    (error, rolled_back)
}

/// Whether a file holds a top-level statement that starts or ends a transaction, through which
/// it may leave the runner's transaction. A file that cannot be split counts as holding one.
fn has_transaction_control(text: &str) -> bool {
    let Ok(statements) = sql::split(text) else {
        return true;
    };
    statements.iter().any(|statement| {
        let word = |at: usize| match statement.tokens.get(at) {
            Some(Token::Word(word)) => Some(word.as_str()),
            _ => None,
        };
        match word(0) {
            Some("begin" | "start" | "commit" | "end" | "abort") => true,
            Some("rollback") => word(1) != Some("to"),
            Some("prepare") => word(1) == Some("transaction"),
            _ => false,
        }
    })
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
