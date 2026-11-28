//! The store specification, `mintswarm_core::testing::store`, run against `PgStore`, and the
//! Postgres store's own guarantees: a dropped transaction never goes back to the pool, and a
//! database error or a call dropped part-way aborts the transaction. Runs only with
//! `--features pg-tests` and a local Postgres or container image.

#![cfg(all(test, feature = "pg-tests"))]

use std::time::Duration;

use deadpool_postgres::{Manager, Pool, Runtime};
use mintswarm_core::store::{PgStore, RowVersion, Store, StoreError, Stored, Transaction};
use mintswarm_core::testing::pg;
use mintswarm_core::testing::store::{self as spec, FIXTURE_TREE, Fixture};
use tokio_postgres::NoTls;

/// A pool of up to `max_size` connections on a fresh database holding the fixture table. A
/// checkout waits at most 5 seconds, so a lost permit fails instead of hanging.
async fn pool(max_size: usize) -> Pool {
    let (db, _) = pg::migrated_database(FIXTURE_TREE).await.unwrap();
    Pool::builder(Manager::new(db.config().clone(), NoTls))
        .max_size(max_size)
        .wait_timeout(Some(Duration::from_secs(5)))
        .runtime(Runtime::Tokio1)
        .build()
        .unwrap()
}

async fn postgres() -> PgStore {
    PgStore::new(pool(4).await)
}

#[tokio::test]
async fn committed_insert_is_read_by_a_later_transaction() {
    spec::committed_insert_is_read_by_a_later_transaction(postgres).await;
}

#[tokio::test]
async fn update_bumps_the_row_version() {
    spec::update_bumps_the_row_version(postgres).await;
}

#[tokio::test]
async fn transaction_reads_its_own_writes() {
    spec::transaction_reads_its_own_writes(postgres).await;
}

#[tokio::test]
async fn two_updates_in_one_transaction_commit() {
    spec::two_updates_in_one_transaction_commit(postgres).await;
}

#[tokio::test]
async fn stale_update_conflicts_and_leaves_the_row() {
    spec::stale_update_conflicts_and_leaves_the_row(postgres).await;
}

#[tokio::test]
async fn writer_after_a_concurrent_commit_conflicts() {
    spec::writer_after_a_concurrent_commit_conflicts(postgres).await;
}

#[tokio::test]
async fn transaction_stays_usable_after_a_conflicting_insert() {
    spec::transaction_stays_usable_after_a_conflicting_insert(postgres).await;
}

#[tokio::test]
async fn dropped_transaction_leaves_no_rows() {
    spec::dropped_transaction_leaves_no_rows(postgres).await;
}

#[tokio::test]
async fn rolled_back_transaction_leaves_no_rows() {
    spec::rolled_back_transaction_leaves_no_rows(postgres).await;
}

#[tokio::test]
async fn update_of_a_missing_row_is_not_found() {
    spec::update_of_a_missing_row_is_not_found(postgres).await;
}

#[tokio::test]
async fn insert_of_an_existing_key_conflicts() {
    spec::insert_of_an_existing_key_conflicts(postgres).await;
}

#[tokio::test]
async fn one_connection_pool_starts_clean_after_a_dropped_transaction() {
    let pool = pool(1).await;
    let store = PgStore::new(pool.clone());
    let mut tx1 = store.begin().await.unwrap();
    tx1.insert(&Fixture::new(1, "dropped")).await.unwrap();
    drop(tx1);
    // The connection was taken out of the pool, not returned to it.
    assert_eq!(pool.status().size, 0);

    let mut tx2 = store.begin().await.unwrap();
    assert_eq!(tx2.get::<Fixture>(&1).await.unwrap(), None);
    assert_eq!(
        tx2.insert(&Fixture::new(1, "kept")).await.unwrap(),
        RowVersion::FIRST
    );
    tx2.commit().await.unwrap();

    let mut tx3 = store.begin().await.unwrap();
    assert_eq!(
        tx3.get::<Fixture>(&1).await.unwrap(),
        Some(Stored {
            record: Fixture::new(1, "kept"),
            version: RowVersion::FIRST,
        })
    );
    tx3.commit().await.unwrap();
}

#[tokio::test]
async fn one_connection_pool_reuses_the_connection_after_commit() {
    let pool = pool(1).await;
    let store = PgStore::new(pool.clone());
    let mut tx = store.begin().await.unwrap();
    tx.insert(&Fixture::new(1, "one")).await.unwrap();
    tx.commit().await.unwrap();
    let status = pool.status();
    assert_eq!((status.size, status.available), (1, 1));

    let mut tx = store.begin().await.unwrap();
    assert!(tx.get::<Fixture>(&1).await.unwrap().is_some());
    tx.rollback().await.unwrap();
    let status = pool.status();
    assert_eq!((status.size, status.available), (1, 1));
}

#[tokio::test]
async fn a_postgres_error_aborts_the_transaction() {
    let store = postgres().await;
    let mut tx = store.begin().await.unwrap();
    tx.insert(&Fixture::new(1, "one")).await.unwrap();
    let error = tx
        .insert(&Fixture::new(2, "nul \0 inside"))
        .await
        .unwrap_err();
    assert!(matches!(error, StoreError::Postgres(_)), "{error:?}");
    let error = tx.get::<Fixture>(&1).await.unwrap_err();
    assert!(matches!(error, StoreError::Aborted), "{error:?}");
    let error = tx.commit().await.unwrap_err();
    assert!(matches!(error, StoreError::Aborted), "{error:?}");

    let mut tx = store.begin().await.unwrap();
    assert_eq!(tx.get::<Fixture>(&1).await.unwrap(), None);
    tx.commit().await.unwrap();
}

#[tokio::test]
async fn a_call_dropped_part_way_aborts_the_transaction() {
    let store = postgres().await;
    let mut tx = store.begin().await.unwrap();
    tx.insert(&Fixture::new(1, "one")).await.unwrap();
    tx.commit().await.unwrap();

    let mut a = store.begin().await.unwrap();
    a.update(&Fixture::new(1, "a"), RowVersion::FIRST)
        .await
        .unwrap();
    let mut b = store.begin().await.unwrap();
    // B's update waits on A's row lock, so the timeout drops its future part-way.
    let record = Fixture::new(1, "b");
    let update = b.update(&record, RowVersion::FIRST);
    tokio::time::timeout(Duration::from_millis(200), update)
        .await
        .unwrap_err();
    // Bounded, so a transaction left usable fails here instead of waiting on A's lock.
    let error = tokio::time::timeout(Duration::from_secs(2), b.get::<Fixture>(&1))
        .await
        .expect("a poisoned transaction answers without the server")
        .unwrap_err();
    assert!(matches!(error, StoreError::Aborted), "{error:?}");

    // B's update is still queued on its connection behind A's lock, so A ends first.
    a.rollback().await.unwrap();
    let error = b.commit().await.unwrap_err();
    assert!(matches!(error, StoreError::Aborted), "{error:?}");

    let mut tx = store.begin().await.unwrap();
    assert_eq!(
        tx.get::<Fixture>(&1).await.unwrap(),
        Some(Stored {
            record: Fixture::new(1, "one"),
            version: RowVersion::FIRST,
        })
    );
    tx.commit().await.unwrap();
}
