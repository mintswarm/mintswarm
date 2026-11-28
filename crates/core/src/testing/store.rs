//! The store specification: the cases every [`Store`] passes, whatever its backend.
//!
//! Each case is a `pub async fn` that takes a factory making an empty store that holds the
//! [`Fixture`] table, and panics when the store breaks the contract. Every backend's test file
//! runs every case, one test each:
//!
//! - [`committed_insert_is_read_by_a_later_transaction`]
//! - [`update_bumps_the_row_version`]
//! - [`transaction_reads_its_own_writes`]
//! - [`two_updates_in_one_transaction_commit`]
//! - [`stale_update_conflicts_and_leaves_the_row`]
//! - [`writer_after_a_concurrent_commit_conflicts`]
//! - [`transaction_stays_usable_after_a_conflicting_insert`]
//! - [`dropped_transaction_leaves_no_rows`]
//! - [`rolled_back_transaction_leaves_no_rows`]
//! - [`update_of_a_missing_row_is_not_found`]
//! - [`insert_of_an_existing_key_conflicts`]
//!
//! A Postgres store builds its table from [`FIXTURE_TREE`].

use std::future::Future;

use tokio_postgres::Row;
use tokio_postgres::types::ToSql;

use crate::store::{RowVersion, Store, StoreError, Stored, Transaction, Versioned};

/// The record of the cases: a key and a body, in the table `store_fixture`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fixture {
    /// The key.
    pub id: i64,
    /// The body.
    pub body: String,
}

impl Fixture {
    /// A fixture with `id` and `body`.
    pub fn new(id: i64, body: &str) -> Self {
        Self {
            id,
            body: body.to_owned(),
        }
    }
}

impl Versioned for Fixture {
    const TABLE: &'static str = "store_fixture";
    const KEY_COLUMN: &'static str = "id";
    const COLUMNS: &'static [&'static str] = &["body"];

    type Key = i64;

    fn key(&self) -> i64 {
        self.id
    }

    fn values(&self) -> Vec<&(dyn ToSql + Sync)> {
        vec![&self.body]
    }

    fn from_row(row: &Row) -> Result<Self, tokio_postgres::Error> {
        Ok(Self {
            id: row.try_get("id")?,
            body: row.try_get("body")?,
        })
    }
}

/// The migration tree that creates the [`Fixture`] table, as `(path, bytes)` pairs.
pub const FIXTURE_TREE: &[(&str, &[u8])] = &[(
    "mintswarm/migrations/20260101000000_store_fixture.up.sql",
    b"CREATE TABLE store_fixture (\n    id bigint PRIMARY KEY,\n    body text NOT NULL,\n    \
      row_version bigint NOT NULL\n);\n",
)];

/// `fixture` at `version`.
fn stored(fixture: Fixture, version: i64) -> Stored<Fixture> {
    Stored {
        record: fixture,
        version: RowVersion(version),
    }
}

async fn begin<S: Store>(store: &S) -> S::Transaction {
    store.begin().await.expect("begin")
}

/// The row with `id`, read in a transaction of its own.
async fn read<S: Store>(store: &S, id: i64) -> Option<Stored<Fixture>> {
    let mut tx = begin(store).await;
    let row = tx.get::<Fixture>(&id).await.expect("get");
    tx.commit().await.expect("commit of a read");
    row
}

/// Inserts and commits `fixture`.
async fn seed<S: Store>(store: &S, fixture: &Fixture) {
    let mut tx = begin(store).await;
    assert_eq!(
        tx.insert(fixture).await.expect("insert of a new key"),
        RowVersion::FIRST
    );
    tx.commit().await.expect("commit of a seed");
}

/// Panics unless `error` is a conflict.
fn assert_conflict(error: StoreError) {
    assert!(
        matches!(error, StoreError::Conflict),
        "expected a conflict, got {error:?}"
    );
}

/// A committed insert is read, at version 1, by a later transaction.
pub async fn committed_insert_is_read_by_a_later_transaction<S, F, Fut>(make: F)
where
    S: Store,
    F: FnOnce() -> Fut,
    Fut: Future<Output = S>,
{
    let store = make().await;
    seed(&store, &Fixture::new(1, "one")).await;
    assert_eq!(
        read(&store, 1).await,
        Some(stored(Fixture::new(1, "one"), 1))
    );
}

/// A committed update replaces the record and raises its version by one.
pub async fn update_bumps_the_row_version<S, F, Fut>(make: F)
where
    S: Store,
    F: FnOnce() -> Fut,
    Fut: Future<Output = S>,
{
    let store = make().await;
    seed(&store, &Fixture::new(1, "one")).await;
    let mut tx = begin(&store).await;
    let version = tx
        .update(&Fixture::new(1, "two"), RowVersion::FIRST)
        .await
        .expect("update at the current version");
    assert_eq!(version, RowVersion(2));
    tx.commit().await.expect("commit");
    assert_eq!(
        read(&store, 1).await,
        Some(stored(Fixture::new(1, "two"), 2))
    );
}

/// A transaction reads its own uncommitted insert and update.
pub async fn transaction_reads_its_own_writes<S, F, Fut>(make: F)
where
    S: Store,
    F: FnOnce() -> Fut,
    Fut: Future<Output = S>,
{
    let store = make().await;
    let mut tx = begin(&store).await;
    tx.insert(&Fixture::new(1, "one")).await.expect("insert");
    assert_eq!(
        tx.get::<Fixture>(&1).await.expect("get"),
        Some(stored(Fixture::new(1, "one"), 1))
    );
    tx.update(&Fixture::new(1, "two"), RowVersion::FIRST)
        .await
        .expect("update of the own insert");
    assert_eq!(
        tx.get::<Fixture>(&1).await.expect("get"),
        Some(stored(Fixture::new(1, "two"), 2))
    );
    tx.commit().await.expect("commit");
    assert_eq!(
        read(&store, 1).await,
        Some(stored(Fixture::new(1, "two"), 2))
    );
}

/// Two updates in one transaction, each with the version the one before returned, commit with
/// the row at version 3.
pub async fn two_updates_in_one_transaction_commit<S, F, Fut>(make: F)
where
    S: Store,
    F: FnOnce() -> Fut,
    Fut: Future<Output = S>,
{
    let store = make().await;
    seed(&store, &Fixture::new(1, "one")).await;
    let mut tx = begin(&store).await;
    let second = tx
        .update(&Fixture::new(1, "two"), RowVersion::FIRST)
        .await
        .expect("first update");
    let third = tx
        .update(&Fixture::new(1, "three"), second)
        .await
        .expect("second update");
    assert_eq!(third, RowVersion(3));
    tx.commit().await.expect("commit");
    assert_eq!(
        read(&store, 1).await,
        Some(stored(Fixture::new(1, "three"), 3))
    );
}

/// An update whose expected version is stale gets a conflict, and the row is unchanged.
pub async fn stale_update_conflicts_and_leaves_the_row<S, F, Fut>(make: F)
where
    S: Store,
    F: FnOnce() -> Fut,
    Fut: Future<Output = S>,
{
    let store = make().await;
    seed(&store, &Fixture::new(1, "one")).await;
    let mut tx = begin(&store).await;
    tx.update(&Fixture::new(1, "two"), RowVersion::FIRST)
        .await
        .expect("update at the current version");
    tx.commit().await.expect("commit");

    let mut tx = begin(&store).await;
    let error = tx
        .update(&Fixture::new(1, "stale"), RowVersion::FIRST)
        .await
        .expect_err("an update at a stale version");
    assert_conflict(error);
    tx.commit()
        .await
        .expect("commit after a conflict, with nothing written");
    assert_eq!(
        read(&store, 1).await,
        Some(stored(Fixture::new(1, "two"), 2))
    );
}

/// Two open transactions read the row at version 1; after the first updates it and commits,
/// the second's update at version 1 gets a conflict, and the row holds the first's body.
pub async fn writer_after_a_concurrent_commit_conflicts<S, F, Fut>(make: F)
where
    S: Store,
    F: FnOnce() -> Fut,
    Fut: Future<Output = S>,
{
    let store = make().await;
    seed(&store, &Fixture::new(1, "one")).await;
    let mut a = begin(&store).await;
    let mut b = begin(&store).await;
    let seen_by_a = a.get::<Fixture>(&1).await.expect("get").expect("the row");
    let seen_by_b = b.get::<Fixture>(&1).await.expect("get").expect("the row");
    assert_eq!(seen_by_a.version, RowVersion::FIRST);
    assert_eq!(seen_by_b.version, RowVersion::FIRST);

    a.update(&Fixture::new(1, "a"), seen_by_a.version)
        .await
        .expect("update of the first writer");
    a.commit().await.expect("commit of the first writer");
    let error = b
        .update(&Fixture::new(1, "b"), seen_by_b.version)
        .await
        .expect_err("an update after a concurrent commit");
    assert_conflict(error);
    b.rollback().await.expect("rollback");
    assert_eq!(read(&store, 1).await, Some(stored(Fixture::new(1, "a"), 2)));
}

/// A conflicting insert leaves the transaction usable: a later insert in it commits.
pub async fn transaction_stays_usable_after_a_conflicting_insert<S, F, Fut>(make: F)
where
    S: Store,
    F: FnOnce() -> Fut,
    Fut: Future<Output = S>,
{
    let store = make().await;
    seed(&store, &Fixture::new(1, "one")).await;
    let mut tx = begin(&store).await;
    let error = tx
        .insert(&Fixture::new(1, "again"))
        .await
        .expect_err("an insert of a taken key");
    assert_conflict(error);
    tx.insert(&Fixture::new(2, "two"))
        .await
        .expect("an insert after a conflict");
    tx.commit().await.expect("commit after a conflict");
    assert_eq!(
        read(&store, 2).await,
        Some(stored(Fixture::new(2, "two"), 1))
    );
    assert_eq!(
        read(&store, 1).await,
        Some(stored(Fixture::new(1, "one"), 1))
    );
}

/// Writes, in `tx`, an update of row 1 and an insert of row 2.
async fn write_both<T: Transaction>(tx: &mut T) {
    tx.update(&Fixture::new(1, "changed"), RowVersion::FIRST)
        .await
        .expect("update");
    tx.insert(&Fixture::new(2, "two")).await.expect("insert");
}

/// A transaction dropped without commit leaves neither its insert nor its update.
pub async fn dropped_transaction_leaves_no_rows<S, F, Fut>(make: F)
where
    S: Store,
    F: FnOnce() -> Fut,
    Fut: Future<Output = S>,
{
    let store = make().await;
    seed(&store, &Fixture::new(1, "one")).await;
    let mut tx = begin(&store).await;
    write_both(&mut tx).await;
    drop(tx);
    assert_eq!(read(&store, 2).await, None);
    assert_eq!(
        read(&store, 1).await,
        Some(stored(Fixture::new(1, "one"), 1))
    );
}

/// A rolled-back transaction leaves neither its insert nor its update.
pub async fn rolled_back_transaction_leaves_no_rows<S, F, Fut>(make: F)
where
    S: Store,
    F: FnOnce() -> Fut,
    Fut: Future<Output = S>,
{
    let store = make().await;
    seed(&store, &Fixture::new(1, "one")).await;
    let mut tx = begin(&store).await;
    write_both(&mut tx).await;
    tx.rollback().await.expect("rollback");
    assert_eq!(read(&store, 2).await, None);
    assert_eq!(
        read(&store, 1).await,
        Some(stored(Fixture::new(1, "one"), 1))
    );
}

/// An update of a key with no row gets not-found, not a conflict, and the transaction still
/// commits.
pub async fn update_of_a_missing_row_is_not_found<S, F, Fut>(make: F)
where
    S: Store,
    F: FnOnce() -> Fut,
    Fut: Future<Output = S>,
{
    let store = make().await;
    let mut tx = begin(&store).await;
    let error = tx
        .update(&Fixture::new(1, "one"), RowVersion::FIRST)
        .await
        .expect_err("an update of a missing row");
    assert!(
        matches!(error, StoreError::NotFound),
        "expected not-found, got {error:?}"
    );
    tx.commit().await.expect("commit after not-found");
    assert_eq!(read(&store, 1).await, None);
}

/// An insert of a committed key gets a conflict, and the row is unchanged.
pub async fn insert_of_an_existing_key_conflicts<S, F, Fut>(make: F)
where
    S: Store,
    F: FnOnce() -> Fut,
    Fut: Future<Output = S>,
{
    let store = make().await;
    seed(&store, &Fixture::new(1, "one")).await;
    let mut tx = begin(&store).await;
    let error = tx
        .insert(&Fixture::new(1, "again"))
        .await
        .expect_err("an insert of a taken key");
    assert_conflict(error);
    tx.commit().await.expect("commit after a conflict");
    assert_eq!(
        read(&store, 1).await,
        Some(stored(Fixture::new(1, "one"), 1))
    );
}
