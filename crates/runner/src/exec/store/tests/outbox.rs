//! Tests of the outbox: suppressed mode, the sender-facing `due` read and idempotent inserts.

use mintswarm_domain::ids::JobId;

use super::super::{BlobId, EffectKind, EffectState, NewEffect, OutboxMode};
use super::{fresh_runner_dir, open};

/// A comment effect of `job` with `key`.
fn comment(job: &str, key: &str) -> NewEffect {
    NewEffect {
        job_id: JobId(job.to_owned()),
        kind: EffectKind::Comment,
        effect_key: key.to_owned(),
        payload: format!("comment {key} of {job}").into_bytes(),
    }
}

#[test]
fn a_suppressed_effect_is_stored_as_suppressed_and_never_due() {
    let (_tmp, runner_dir) = fresh_runner_dir();
    let mut store = open(&runner_dir, OutboxMode::Suppressed).unwrap();
    let effect = comment("j1", "k1");
    let id = store.insert_effect(&effect).unwrap();

    let row = store.effect(id).unwrap().unwrap();
    assert_eq!(row.state, EffectState::Suppressed);
    assert_eq!(row.job_id, effect.job_id);
    assert_eq!(row.kind, EffectKind::Comment);
    assert_eq!(row.effect_key, "k1");
    assert_eq!(row.attempts, 0);
    assert_eq!(row.next_at, None);
    assert_eq!(row.payload_blob, BlobId::of(&effect.payload));
    assert_eq!(store.get_blob(&row.payload_blob).unwrap(), effect.payload);
    assert!(store.due(i64::MAX, u32::MAX).unwrap().is_empty());

    drop(store);
    let store = open(&runner_dir, OutboxMode::Live).unwrap();
    assert_eq!(
        store.effect(id).unwrap().unwrap().state,
        EffectState::Suppressed
    );
    assert!(store.due(i64::MAX, u32::MAX).unwrap().is_empty());
}

#[test]
fn a_live_effect_is_pending_and_due() {
    let (_tmp, runner_dir) = fresh_runner_dir();
    let mut store = open(&runner_dir, OutboxMode::Live).unwrap();
    let id = store.insert_effect(&comment("j1", "k1")).unwrap();
    let due = store.due(i64::MAX, u32::MAX).unwrap();
    assert_eq!(due.len(), 1);
    assert_eq!(due[0].effect_id, id);
    assert_eq!(due[0].state, EffectState::Pending);
}

#[test]
fn re_inserting_an_effect_returns_the_existing_row() {
    let (_tmp, runner_dir) = fresh_runner_dir();
    let mut store = open(&runner_dir, OutboxMode::Suppressed).unwrap();
    let first = store.insert_effect(&comment("j1", "k1")).unwrap();
    let before = store.effect(first).unwrap();
    let mut again = comment("j1", "k1");
    again.payload = b"rendered differently".to_vec();
    assert_eq!(store.insert_effect(&again).unwrap(), first);
    assert_eq!(store.effect(first).unwrap(), before);
    let other_job = store.insert_effect(&comment("j2", "k1")).unwrap();
    assert_ne!(other_job, first);
}

#[test]
fn due_orders_rows_by_job_then_insertion() {
    let (_tmp, runner_dir) = fresh_runner_dir();
    let mut store = open(&runner_dir, OutboxMode::Live).unwrap();
    let b1 = store.insert_effect(&comment("b", "1")).unwrap();
    let a1 = store.insert_effect(&comment("a", "1")).unwrap();
    let b2 = store.insert_effect(&comment("b", "2")).unwrap();
    let a2 = store.insert_effect(&comment("a", "2")).unwrap();
    let ids: Vec<i64> = store
        .due(0, u32::MAX)
        .unwrap()
        .into_iter()
        .map(|effect| effect.effect_id)
        .collect();
    assert_eq!(ids, [a1, a2, b1, b2]);
    assert_eq!(store.due(0, 2).unwrap().len(), 2);
}

#[test]
fn due_skips_rows_not_yet_due_and_rows_done_with() {
    let (_tmp, runner_dir) = fresh_runner_dir();
    let mut store = open(&runner_dir, OutboxMode::Live).unwrap();
    let later = store.insert_effect(&comment("j", "later")).unwrap();
    let acked = store.insert_effect(&comment("j", "acked")).unwrap();
    let sent = store.insert_effect(&comment("j", "sent")).unwrap();
    store
        .conn
        .execute(
            "UPDATE outbox SET next_at = 1000 WHERE effect_id = ?1",
            [later],
        )
        .unwrap();
    store
        .conn
        .execute(
            "UPDATE outbox SET state = 'acked' WHERE effect_id = ?1",
            [acked],
        )
        .unwrap();
    store
        .conn
        .execute(
            "UPDATE outbox SET state = 'sent' WHERE effect_id = ?1",
            [sent],
        )
        .unwrap();
    let ids = |now| -> Vec<i64> {
        store
            .due(now, u32::MAX)
            .unwrap()
            .into_iter()
            .map(|effect| effect.effect_id)
            .collect()
    };
    assert_eq!(ids(999), [sent]);
    assert_eq!(ids(1000), [later, sent]);
}

#[test]
fn a_failed_transaction_leaves_no_effect() {
    let (_tmp, runner_dir) = fresh_runner_dir();
    let mut store = open(&runner_dir, OutboxMode::Live).unwrap();
    let result: Result<(), _> = store.transaction(|tx| {
        tx.insert_effect(&comment("j", "k"))?;
        Err(super::super::StoreError::LeaseTtl(0))
    });
    assert!(result.is_err());
    assert!(store.due(i64::MAX, u32::MAX).unwrap().is_empty());
}
