//! The store specification, `mintswarm_core::testing::store`, run against `MemoryStore`, and
//! the memory store's own guarantees.

#![cfg(test)]

use mintswarm_core::store::{MemoryStore, RowVersion, Store, StoreError, Stored, Transaction};
use mintswarm_core::testing::store::{self as spec, Fixture};

async fn memory() -> MemoryStore {
    MemoryStore::new()
}

#[tokio::test]
async fn committed_insert_is_read_by_a_later_transaction() {
    spec::committed_insert_is_read_by_a_later_transaction(memory).await;
}

#[tokio::test]
async fn update_bumps_the_row_version() {
    spec::update_bumps_the_row_version(memory).await;
}

#[tokio::test]
async fn transaction_reads_its_own_writes() {
    spec::transaction_reads_its_own_writes(memory).await;
}

#[tokio::test]
async fn two_updates_in_one_transaction_commit() {
    spec::two_updates_in_one_transaction_commit(memory).await;
}

#[tokio::test]
async fn stale_update_conflicts_and_leaves_the_row() {
    spec::stale_update_conflicts_and_leaves_the_row(memory).await;
}

#[tokio::test]
async fn writer_after_a_concurrent_commit_conflicts() {
    spec::writer_after_a_concurrent_commit_conflicts(memory).await;
}

#[tokio::test]
async fn transaction_stays_usable_after_a_conflicting_insert() {
    spec::transaction_stays_usable_after_a_conflicting_insert(memory).await;
}

#[tokio::test]
async fn dropped_transaction_leaves_no_rows() {
    spec::dropped_transaction_leaves_no_rows(memory).await;
}

#[tokio::test]
async fn rolled_back_transaction_leaves_no_rows() {
    spec::rolled_back_transaction_leaves_no_rows(memory).await;
}

#[tokio::test]
async fn update_of_a_missing_row_is_not_found() {
    spec::update_of_a_missing_row_is_not_found(memory).await;
}

#[tokio::test]
async fn insert_of_an_existing_key_conflicts() {
    spec::insert_of_an_existing_key_conflicts(memory).await;
}

/// Inserts and commits `fixture`.
async fn seed(store: &MemoryStore, fixture: &Fixture) {
    let mut tx = store.begin().await.unwrap();
    tx.insert(fixture).await.unwrap();
    tx.commit().await.unwrap();
}

async fn read(store: &MemoryStore, id: i64) -> Option<Stored<Fixture>> {
    let mut tx = store.begin().await.unwrap();
    let row = tx.get::<Fixture>(&id).await.unwrap();
    tx.rollback().await.unwrap();
    row
}

#[tokio::test]
async fn concurrent_updates_conflict_at_the_second_commit() {
    let store = MemoryStore::new();
    seed(&store, &Fixture::new(1, "one")).await;
    let mut a = store.begin().await.unwrap();
    let mut b = store.begin().await.unwrap();
    a.update(&Fixture::new(1, "a"), RowVersion::FIRST)
        .await
        .unwrap();
    b.update(&Fixture::new(1, "b"), RowVersion::FIRST)
        .await
        .unwrap();
    a.commit().await.unwrap();
    let error = b.commit().await.unwrap_err();
    assert!(matches!(error, StoreError::Conflict), "{error:?}");
    assert_eq!(
        read(&store, 1).await,
        Some(Stored {
            record: Fixture::new(1, "a"),
            version: RowVersion(2),
        })
    );
}

#[tokio::test]
async fn concurrent_inserts_of_one_key_conflict_at_the_second_commit() {
    let store = MemoryStore::new();
    let mut a = store.begin().await.unwrap();
    let mut b = store.begin().await.unwrap();
    a.insert(&Fixture::new(1, "a")).await.unwrap();
    b.insert(&Fixture::new(1, "b")).await.unwrap();
    a.commit().await.unwrap();
    let error = b.commit().await.unwrap_err();
    assert!(matches!(error, StoreError::Conflict), "{error:?}");
    assert_eq!(
        read(&store, 1).await,
        Some(Stored {
            record: Fixture::new(1, "a"),
            version: RowVersion::FIRST,
        })
    );
}
