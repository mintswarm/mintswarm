//! The store contract, [`Store`] and [`Transaction`]: everything the plane keeps goes through a
//! transaction of a store (design 5.2), kept in memory by [`MemoryStore`] or in Postgres by
//! [`PgStore`].
//!
//! Records are [`Versioned`]: each row has a key and a [`RowVersion`] that starts at 1 and rises
//! by one with every update, and an update names the version it expects to replace.
//!
//! The contract both stores keep:
//!
//! - Isolation is READ COMMITTED: each read sees what was committed when it ran, together with
//!   the transaction's own writes.
//! - A write that the committed state already contradicts, an update whose expected version is
//!   not the row's or an insert of a key that is taken, gets [`StoreError::Conflict`] at the
//!   [`insert`](Transaction::insert) or [`update`](Transaction::update) call.
//! - In the memory store, a write that clashes only with another transaction's uncommitted
//!   write succeeds at the call and gets [`StoreError::Conflict`] at
//!   [`commit`](Transaction::commit), which then applies nothing.
//! - In the Postgres store, that write blocks at the call until the other transaction ends. It
//!   then gets [`StoreError::Conflict`] at that call if the other transaction committed, and
//!   goes ahead if it rolled back.
//! - So one task must not hold two open transactions that write the same row or key: on
//!   Postgres the second write would wait for a transaction its own task never ends, a
//!   deadlock.
//! - An update of a key that has no row gets [`StoreError::NotFound`], never
//!   [`StoreError::Conflict`].
//! - [`StoreError::Conflict`] and [`StoreError::NotFound`] leave the transaction usable.
//! - Any other error poisons the transaction: every later call returns
//!   [`StoreError::Aborted`], and [`commit`](Transaction::commit) rolls back and returns
//!   [`StoreError::Aborted`]. A call whose future is dropped before it finishes poisons it too.
//! - A transaction dropped without [`commit`](Transaction::commit) rolls back.

mod memory;
mod postgres;

use std::future::Future;

use tokio_postgres::Row;
use tokio_postgres::types::ToSql;

pub use memory::{MemoryStore, MemoryTransaction};
pub use postgres::{PgStore, PgTransaction};

/// The version of a stored row: 1 when it is inserted, one more with each update.
///
/// It has no `ToSql` or `FromSql` impl: the Postgres store binds and reads the `i64` itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RowVersion(pub i64);

impl RowVersion {
    /// The version of a freshly inserted row.
    pub const FIRST: Self = Self(1);

    /// The version an update of a row at this version gives it.
    #[must_use]
    pub fn next(self) -> Self {
        Self(self.0 + 1)
    }
}

/// A record as stored, with its row version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stored<R> {
    /// The record.
    pub record: R,
    /// The row's version.
    pub version: RowVersion,
}

/// A record type kept in one table of the `mintswarm` schema, with a key column, its other
/// columns and a `row_version bigint` column.
///
/// Exactly one `Versioned` type is kept in each table. [`TABLE`](Self::TABLE),
/// [`KEY_COLUMN`](Self::KEY_COLUMN) and [`COLUMNS`](Self::COLUMNS) are unqualified plain
/// identifiers, lowercase ASCII letters, digits and underscores: the Postgres store quotes them
/// and puts them into its SQL.
pub trait Versioned: Clone + Send + Sync + 'static {
    /// The table, in the `mintswarm` schema.
    const TABLE: &'static str;
    /// The key column, unique in the table.
    const KEY_COLUMN: &'static str;
    /// The other columns, in the order [`values`](Self::values) gives them, without
    /// `row_version`.
    const COLUMNS: &'static [&'static str];

    /// The key's type.
    type Key: Clone + Ord + Send + Sync + ToSql + 'static;

    /// The record's key.
    fn key(&self) -> Self::Key;

    /// The values of [`COLUMNS`](Self::COLUMNS), in that order.
    fn values(&self) -> Vec<&(dyn ToSql + Sync)>;

    /// The record read back from a row that holds the key column and every column of
    /// [`COLUMNS`](Self::COLUMNS), by name.
    fn from_row(row: &Row) -> Result<Self, tokio_postgres::Error>;
}

/// The errors of a [`Store`] and its transactions.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    /// The row's version is not the expected one, or the key is already taken.
    #[error("the row was changed by another transaction, or its key is already taken")]
    Conflict,
    /// No row has the key.
    #[error("no row has the key")]
    NotFound,
    /// An earlier error poisoned the transaction, which was or will be rolled back.
    #[error("the transaction was aborted by an earlier error and is rolled back")]
    Aborted,
    /// Postgres or its client failed.
    #[error("the database failed")]
    Postgres(#[source] tokio_postgres::Error),
    /// No connection could be taken from the pool.
    #[error("no database connection could be taken from the pool")]
    Pool(#[source] deadpool_postgres::PoolError),
}

/// A store: the source of [`Transaction`]s.
pub trait Store: Send + Sync {
    /// The store's transaction.
    type Transaction: Transaction;

    /// Begins a transaction.
    fn begin(&self) -> impl Future<Output = Result<Self::Transaction, StoreError>> + Send;
}

/// An open transaction of a [`Store`]. Dropping it without [`commit`](Self::commit) rolls it
/// back.
pub trait Transaction: Send {
    /// The record with `key`, with its version, or `None` when no row has the key.
    fn get<R: Versioned>(
        &mut self,
        key: &R::Key,
    ) -> impl Future<Output = Result<Option<Stored<R>>, StoreError>> + Send;

    /// Inserts `record` at [`RowVersion::FIRST`], which it returns; an insert of a key that is
    /// taken gets [`StoreError::Conflict`].
    fn insert<R: Versioned>(
        &mut self,
        record: &R,
    ) -> impl Future<Output = Result<RowVersion, StoreError>> + Send;

    /// Replaces the row with `record`'s key by `record`, if the row is at `expected`, and
    /// returns its new version; otherwise gets [`StoreError::Conflict`], or
    /// [`StoreError::NotFound`] when no row has the key.
    fn update<R: Versioned>(
        &mut self,
        record: &R,
        expected: RowVersion,
    ) -> impl Future<Output = Result<RowVersion, StoreError>> + Send;

    /// Commits the transaction's writes.
    fn commit(self) -> impl Future<Output = Result<(), StoreError>> + Send;

    /// Rolls the transaction's writes back.
    fn rollback(self) -> impl Future<Output = Result<(), StoreError>> + Send;
}
