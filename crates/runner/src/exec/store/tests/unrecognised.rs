//! Tests of the record of unrecognised failures on a manual clock: counting, first and last
//! seen times, separate keys and the reader's order.

use super::super::{NewUnrecognised, OutboxMode, Store};
use super::{ManualClock, fresh_runner_dir};

/// The clock's starting time.
const START: i64 = 1_000_000;

fn failure(adapter: &str, signature: &str, excerpt: &str) -> NewUnrecognised {
    NewUnrecognised {
        adapter: adapter.to_owned(),
        signature: signature.to_owned(),
        excerpt: excerpt.to_owned(),
        status: "exit code 128".to_owned(),
    }
}

#[test]
fn a_repeated_failure_is_counted_in_one_row() {
    let (_tmp, runner_dir) = fresh_runner_dir();
    let clock = ManualClock::at(START);
    let mut store = Store::open(&runner_dir, clock.clone(), OutboxMode::Suppressed).unwrap();

    store
        .transaction(|tx| tx.record_unrecognised(&failure("git", "s1", "first")))
        .unwrap();
    let rows = store.transaction(|tx| tx.unrecognised("git")).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].count, 1);
    assert_eq!(rows[0].first_seen_at, START);
    assert_eq!(rows[0].last_seen_at, START);

    clock.set(START + 500);
    store
        .transaction(|tx| tx.record_unrecognised(&failure("git", "s1", "second")))
        .unwrap();
    let rows = store.transaction(|tx| tx.unrecognised("git")).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].count, 2);
    assert_eq!(rows[0].first_seen_at, START);
    assert_eq!(rows[0].last_seen_at, START + 500);
    assert_eq!(rows[0].excerpt, "second");
}

#[test]
fn other_adapters_and_signatures_get_their_own_rows_read_by_count() {
    let (_tmp, runner_dir) = fresh_runner_dir();
    let mut store =
        Store::open(&runner_dir, ManualClock::at(START), OutboxMode::Suppressed).unwrap();
    store
        .transaction(|tx| {
            tx.record_unrecognised(&failure("git", "rare", "r"))?;
            tx.record_unrecognised(&failure("git", "often", "o"))?;
            tx.record_unrecognised(&failure("git", "often", "o"))?;
            tx.record_unrecognised(&failure("other", "often", "o"))
        })
        .unwrap();
    let git = store.transaction(|tx| tx.unrecognised("git")).unwrap();
    let seen: Vec<(&str, i64)> = git
        .iter()
        .map(|row| (row.signature.as_str(), row.count))
        .collect();
    assert_eq!(seen, [("often", 2), ("rare", 1)]);
    let other = store.transaction(|tx| tx.unrecognised("other")).unwrap();
    assert_eq!(other.len(), 1);
    assert_eq!(other[0].count, 1);
    assert!(
        store
            .transaction(|tx| tx.unrecognised("none"))
            .unwrap()
            .is_empty()
    );
}
