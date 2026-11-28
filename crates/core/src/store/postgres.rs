//! [`PgStore`], the [`Store`] on a deadpool-postgres pool.
//!
//! A [`PgTransaction`] owns its pooled connection and issues `BEGIN`, `COMMIT` and `ROLLBACK`
//! itself. It counts as open from before anything is sent on the connection until a `COMMIT`
//! or `ROLLBACK` succeeds. Dropped while open, it takes the connection out of the pool and
//! closes it, so the server rolls the transaction back and the pool opens a fresh connection:
//! an open transaction never goes back to the pool.
//!
//! Conflicts raise no server error: an insert runs `ON CONFLICT DO NOTHING` and a stale update
//! matches no row, so both leave the transaction usable. Any error of the client or the server
//! poisons it, and so does a call whose future is dropped before it finishes.

use deadpool_postgres::{Object, Pool};
use tokio_postgres::Row;
use tokio_postgres::types::ToSql;

use super::{RowVersion, Store, StoreError, Stored, Transaction, Versioned};

/// The schema every [`Versioned`] table is in.
const SCHEMA: &str = "mintswarm";

/// A [`Store`] in Postgres, taking a connection from `pool` for each transaction.
#[derive(Clone)]
pub struct PgStore {
    pool: Pool,
}

impl PgStore {
    /// A store on `pool`, whose database holds the tables of the records it keeps.
    pub fn new(pool: Pool) -> Self {
        Self { pool }
    }
}

impl std::fmt::Debug for PgStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PgStore")
            .field("pool", &self.pool.status())
            .finish()
    }
}

impl Store for PgStore {
    type Transaction = PgTransaction;

    async fn begin(&self) -> Result<PgTransaction, StoreError> {
        let object = self.pool.get().await.map_err(StoreError::Pool)?;
        // Open before `BEGIN` is sent: a failed or cancelled begin detaches the connection.
        let mut transaction = PgTransaction {
            object: Some(object),
            open: true,
            poisoned: false,
        };
        transaction.statement("BEGIN").await?;
        Ok(transaction)
    }
}

/// A transaction of a [`PgStore`], owning its pooled connection.
pub struct PgTransaction {
    /// Always `Some` until `Drop` takes it.
    object: Option<Object>,
    /// Set from before `BEGIN` is sent until `COMMIT` or `ROLLBACK` succeeds.
    open: bool,
    /// Set while a call runs, and kept when it fails or its future is dropped.
    poisoned: bool,
}

impl std::fmt::Debug for PgTransaction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PgTransaction")
            .field("open", &self.open)
            .field("poisoned", &self.poisoned)
            .finish_non_exhaustive()
    }
}

impl Drop for PgTransaction {
    fn drop(&mut self) {
        if self.open
            && let Some(object) = self.object.take()
        {
            // Out of the pool for good: dropping the client aborts its connection task, the
            // server sees the disconnect and rolls back, and the pool gets its permit back.
            drop(Object::take(object));
        }
    }
}

impl PgTransaction {
    fn object(&self) -> &Object {
        self.object
            .as_ref()
            .expect("the connection is taken only by Drop")
    }

    /// Marks a call as running, or returns [`StoreError::Aborted`] once poisoned.
    fn enter(&mut self) -> Result<(), StoreError> {
        if self.poisoned {
            return Err(StoreError::Aborted);
        }
        self.poisoned = true;
        Ok(())
    }

    /// Runs `sql` with `params` and returns the row it gives, if any; an error poisons the
    /// transaction.
    async fn query_opt(
        &mut self,
        sql: &str,
        params: &[&(dyn ToSql + Sync)],
    ) -> Result<Option<Row>, StoreError> {
        self.enter()?;
        let object = self.object();
        let statement = object
            .prepare_cached(sql)
            .await
            .map_err(StoreError::Postgres)?;
        let row = object
            .query_opt(&statement, params)
            .await
            .map_err(StoreError::Postgres)?;
        self.poisoned = false;
        Ok(row)
    }

    /// Sends a statement without parameters; an error poisons the transaction.
    async fn statement(&mut self, sql: &str) -> Result<(), StoreError> {
        self.enter()?;
        self.object()
            .batch_execute(sql)
            .await
            .map_err(StoreError::Postgres)?;
        self.poisoned = false;
        Ok(())
    }

    /// Sends `COMMIT` or `ROLLBACK` and, once it succeeds, closes the transaction so that
    /// dropping it returns the connection to the pool.
    async fn finish(&mut self, sql: &str) -> Result<(), StoreError> {
        self.object()
            .batch_execute(sql)
            .await
            .map_err(StoreError::Postgres)?;
        self.open = false;
        Ok(())
    }
}

impl Transaction for PgTransaction {
    async fn get<R: Versioned>(&mut self, key: &R::Key) -> Result<Option<Stored<R>>, StoreError> {
        let sql = select_sql::<R>();
        let row = self.query_opt(&sql, &[key]).await?;
        row.map(|row| read_stored(&row))
            .transpose()
            .map_err(|error| {
                self.poisoned = true;
                StoreError::Postgres(error)
            })
    }

    async fn insert<R: Versioned>(&mut self, record: &R) -> Result<RowVersion, StoreError> {
        let sql = insert_sql::<R>();
        let key = record.key();
        let mut params: Vec<&(dyn ToSql + Sync)> = vec![&key];
        params.extend(record.values());
        match self.query_opt(&sql, &params).await? {
            Some(row) => self.version(&row),
            None => Err(StoreError::Conflict),
        }
    }

    async fn update<R: Versioned>(
        &mut self,
        record: &R,
        expected: RowVersion,
    ) -> Result<RowVersion, StoreError> {
        let sql = update_sql::<R>();
        let key = record.key();
        let expected = expected.0;
        let mut params: Vec<&(dyn ToSql + Sync)> = vec![&key];
        params.extend(record.values());
        params.push(&expected);
        if let Some(row) = self.query_opt(&sql, &params).await? {
            return self.version(&row);
        }
        let exists = self.query_opt(&exists_sql::<R>(), &[&key]).await?;
        Err(if exists.is_some() {
            StoreError::Conflict
        } else {
            StoreError::NotFound
        })
    }

    async fn commit(mut self) -> Result<(), StoreError> {
        if self.poisoned {
            // The server may have aborted the transaction already, and would answer `COMMIT`
            // with a rollback. If the `ROLLBACK` fails, the drop closes the connection.
            let _ = self.finish("ROLLBACK").await;
            return Err(StoreError::Aborted);
        }
        self.finish("COMMIT").await
    }

    async fn rollback(mut self) -> Result<(), StoreError> {
        self.finish("ROLLBACK").await
    }
}

impl PgTransaction {
    /// The `row_version` of `row`; a failure to read it poisons the transaction.
    fn version(&mut self, row: &Row) -> Result<RowVersion, StoreError> {
        row.try_get::<_, i64>("row_version")
            .map(RowVersion)
            .map_err(|error| {
                self.poisoned = true;
                StoreError::Postgres(error)
            })
    }
}

fn read_stored<R: Versioned>(row: &Row) -> Result<Stored<R>, tokio_postgres::Error> {
    Ok(Stored {
        record: R::from_row(row)?,
        version: RowVersion(row.try_get("row_version")?),
    })
}

/// Whether `name` is a plain identifier: lowercase ASCII letters, digits and underscores, not
/// starting with a digit.
fn is_plain_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|first| first.is_ascii_lowercase() || first == '_')
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

/// `name` as a quoted identifier, with any `"` in it doubled. A `Versioned` identifier is
/// plain, which the debug assertion checks, so the escape only guards the quoting.
fn quoted(name: &str) -> String {
    debug_assert!(
        is_plain_identifier(name),
        "a Versioned identifier is plain: {name:?}"
    );
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// The schema-qualified, quoted table of `R`.
fn table<R: Versioned>() -> String {
    format!("{SCHEMA}.{}", quoted(R::TABLE))
}

fn select_sql<R: Versioned>() -> String {
    let columns: Vec<String> = std::iter::once(R::KEY_COLUMN)
        .chain(R::COLUMNS.iter().copied())
        .map(quoted)
        .collect();
    format!(
        "SELECT {}, row_version FROM {} WHERE {} = $1",
        columns.join(", "),
        table::<R>(),
        quoted(R::KEY_COLUMN)
    )
}

fn insert_sql<R: Versioned>() -> String {
    let key = quoted(R::KEY_COLUMN);
    let mut columns = vec![key.clone()];
    columns.extend(R::COLUMNS.iter().copied().map(quoted));
    let values: Vec<String> = (1..=columns.len()).map(|n| format!("${n}")).collect();
    format!(
        "INSERT INTO {} ({}, row_version) VALUES ({}, 1) ON CONFLICT ({key}) DO NOTHING \
         RETURNING row_version",
        table::<R>(),
        columns.join(", "),
        values.join(", ")
    )
}

fn update_sql<R: Versioned>() -> String {
    let mut sets: Vec<String> = R::COLUMNS
        .iter()
        .enumerate()
        .map(|(i, column)| format!("{} = ${}", quoted(column), i + 2))
        .collect();
    sets.push("row_version = row_version + 1".to_owned());
    format!(
        "UPDATE {} SET {} WHERE {} = $1 AND row_version = ${} RETURNING row_version",
        table::<R>(),
        sets.join(", "),
        quoted(R::KEY_COLUMN),
        R::COLUMNS.len() + 2
    )
}

fn exists_sql<R: Versioned>() -> String {
    format!(
        "SELECT 1 FROM {} WHERE {} = $1",
        table::<R>(),
        quoted(R::KEY_COLUMN)
    )
}

#[cfg(test)]
mod tests {
    use super::is_plain_identifier;

    #[test]
    fn plain_identifiers_are_lowercase_words() {
        assert!(is_plain_identifier("store_fixture"));
        assert!(is_plain_identifier("_a1"));
        assert!(!is_plain_identifier(""));
        assert!(!is_plain_identifier("1a"));
        assert!(!is_plain_identifier("Body"));
        assert!(!is_plain_identifier("a\"b"));
    }
}
