//! The `msw-state` rule against the rows of design §4: an exhaustive table over the configured
//! values, the fixpoint, the unlisted-blocker count and the purity of `rule.rs`.
// Not redundant: clippy's `allow-unwrap-in-tests` covers a `tests/` crate only under `cfg(test)`.
#![cfg(test)]

use std::collections::BTreeSet;
use std::path::PathBuf;

use mintswarm_config::{AWAITING_APPROVAL, BLOCKED, DONE, NONE, QUEUED, load_repo_config};
use mintswarm_core::state_sync::rule::{
    BlockerChange, BlockerChangeKind, Cause, Decision, Facts, decide, open_blockers,
};
use mintswarm_domain::ids::IssueNumber;
use mintswarm_domain::tracker::Dependency;

/// A configured value the pipeline sets with no constant of its own.
const ESCALATED: &str = "escalated";
/// A value a later config may add; the rule must treat it as active.
const LATER_VALUE: &str = "a value added later";

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Every `tracker.state.options` name of the checked-in config.
fn configured_names() -> Vec<String> {
    let path = manifest_dir().join("../../.mintswarm/config.yaml");
    let text = std::fs::read_to_string(&path).expect("the repository config reads");
    let config = load_repo_config(&text).expect("the repository config loads");
    let state = config
        .state()
        .expect("the repository config has tracker.state");
    state.options.iter().map(|o| o.name.clone()).collect()
}

/// The names the table runs over: every configured one, plus one added later.
fn names() -> Vec<String> {
    let mut names = configured_names();
    names.push(LATER_VALUE.to_owned());
    names
}

fn dependency(number: u64, open: bool) -> Dependency {
    Dependency {
        repository: "mintswarm/mintswarm".to_owned(),
        number: IssueNumber(number),
        open,
    }
}

fn change() -> Cause {
    Cause::Triggered {
        blocker_changes: vec![BlockerChange {
            blocker: IssueNumber(9),
            repository: "mintswarm/mintswarm".to_owned(),
            change: BlockerChangeKind::Closed,
        }],
    }
}

fn empty_trigger() -> Cause {
    Cause::Triggered {
        blocker_changes: Vec::new(),
    }
}

fn has_change(cause: &Cause) -> bool {
    matches!(cause, Cause::Triggered { blocker_changes } if !blocker_changes.is_empty())
}

/// The four values the rule treats specially; every other name is active.
fn is_active(name: &str) -> bool {
    ![NONE, QUEUED, BLOCKED, DONE].contains(&name)
}

/// One row of design §4: when it matches, and what it gives.
struct Row {
    number: u8,
    matches: fn(&Facts<'_>, &Cause) -> bool,
    next: fn(&Facts<'_>) -> Option<&'static str>,
    report: bool,
}

/// The rows of design §4, in order.
fn rows() -> [Row; 7] {
    [
        Row {
            number: 1,
            matches: |f, _| f.closed && f.current != DONE,
            next: |_| Some(DONE),
            report: false,
        },
        Row {
            number: 2,
            matches: |f, _| f.closed,
            next: |_| None,
            report: false,
        },
        Row {
            number: 3,
            matches: |f, _| f.current == DONE,
            next: |f| Some(if f.open_blockers > 0 { BLOCKED } else { QUEUED }),
            report: false,
        },
        Row {
            number: 4,
            matches: |f, _| f.current == QUEUED && f.open_blockers > 0,
            next: |_| Some(BLOCKED),
            report: false,
        },
        Row {
            number: 5,
            matches: |f, _| f.current == BLOCKED && f.open_blockers == 0 && !f.has_sub_issues,
            next: |_| Some(QUEUED),
            report: false,
        },
        Row {
            number: 6,
            matches: |f, c| is_active(f.current) && has_change(c),
            next: |_| None,
            report: true,
        },
        Row {
            number: 7,
            matches: |_, _| true,
            next: |_| None,
            report: false,
        },
    ]
}

/// The first matching row and the decision it gives.
fn oracle(facts: &Facts<'_>, cause: &Cause) -> (u8, Decision) {
    let rows = rows();
    let row = rows
        .iter()
        .find(|row| (row.matches)(facts, cause))
        .expect("row 7 matches everything");
    (
        row.number,
        Decision {
            next: (row.next)(facts),
            report: row.report,
        },
    )
}

fn facts(current: &str, closed: bool, open_blockers: u64, has_sub_issues: bool) -> Facts<'_> {
    Facts {
        closed,
        open_blockers,
        has_sub_issues,
        current,
    }
}

#[test]
fn table_matches_design_rows() {
    let names = names();
    let blocker_counts = [
        open_blockers(&[dependency(2, false)], 1),
        open_blockers(&[dependency(2, true)], 1),
        open_blockers(&[], 1),
    ];
    assert_eq!(blocker_counts, [0, 1, 1]);
    let causes = [Cause::Sweep, change()];
    let mut hit = BTreeSet::new();
    let mut cases = 0;
    for name in &names {
        for closed in [false, true] {
            for &blockers in &blocker_counts {
                for has_sub_issues in [false, true] {
                    for cause in &causes {
                        let f = facts(name, closed, blockers, has_sub_issues);
                        let (row, expected) = oracle(&f, cause);
                        assert_eq!(decide(f, cause), expected, "row {row}: {f:?}, {cause:?}");
                        hit.insert(row);
                        cases += 1;
                    }
                }
            }
        }
    }
    assert_eq!(cases, names.len() * 2 * 3 * 2 * 2);
    assert_eq!(hit, (1..=7).collect::<BTreeSet<u8>>(), "every row is hit");
    pin_named_cases();
}

/// Pins named cases of design §4: B2, B10, B14 and B15, a reopened `done` parent
/// and a trigger without blocker changes.
fn pin_named_cases() {
    // A closed `none` issue becomes `done` (B2).
    assert_eq!(
        decide(facts(NONE, true, 0, false), &Cause::Sweep),
        Decision {
            next: Some(DONE),
            report: false
        }
    );
    // A reopened `done` parent with sub-issues and no blockers becomes `queued`.
    assert_eq!(
        decide(facts(DONE, false, 0, true), &Cause::Sweep),
        Decision {
            next: Some(QUEUED),
            report: false
        }
    );
    // Active values with a blocker change are reported and left alone (B14, B15).
    let configured = configured_names();
    assert!(configured.iter().any(|n| n == ESCALATED));
    for name in [ESCALATED, AWAITING_APPROVAL, LATER_VALUE] {
        assert_eq!(
            decide(facts(name, false, 1, false), &change()),
            Decision {
                next: None,
                report: true
            },
            "{name}"
        );
    }
    // A trigger without blocker changes reports nothing.
    assert_eq!(
        decide(facts(ESCALATED, false, 1, false), &empty_trigger()),
        Decision {
            next: None,
            report: false
        }
    );
    // Sub-issues keep a `blocked` parent blocked (B10).
    assert_eq!(
        decide(facts(BLOCKED, false, 0, true), &Cause::Sweep),
        Decision {
            next: None,
            report: false
        }
    );
}

#[test]
fn open_blockers_counts_unlisted_as_open() {
    assert_eq!(open_blockers(&[], 0), 0);
    assert_eq!(
        open_blockers(&[dependency(2, false), dependency(3, false)], 2),
        0
    );
    assert_eq!(
        open_blockers(&[dependency(2, true), dependency(3, false)], 2),
        1
    );
    assert_eq!(open_blockers(&[dependency(2, false)], 3), 2);
    assert_eq!(open_blockers(&[dependency(2, true)], 3), 3);
    // A total below the listed count saturates: only the listed open ones count.
    assert_eq!(
        open_blockers(&[dependency(2, true), dependency(3, true)], 1),
        2
    );
}

#[test]
fn decide_is_a_fixpoint() {
    let names = names();
    let blocker_counts = [0, 1, 2, 3, open_blockers(&[], 1)];
    let causes = [Cause::Sweep, empty_trigger(), change()];
    for name in &names {
        for closed in [false, true] {
            for &blockers in &blocker_counts {
                for has_sub_issues in [false, true] {
                    for cause in &causes {
                        let first = decide(facts(name, closed, blockers, has_sub_issues), cause);
                        if let Some(next) = first.next {
                            let again =
                                decide(facts(next, closed, blockers, has_sub_issues), cause);
                            assert_eq!(
                                again.next, None,
                                "{name} -> {next}: closed {closed}, blockers {blockers}, \
                                 sub-issues {has_sub_issues}, {cause:?}"
                            );
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn rule_source_is_pure() {
    let path = manifest_dir().join("src/state_sync/rule.rs");
    let source = std::fs::read_to_string(&path).expect("rule.rs reads");
    let code: String = source
        .lines()
        .map(|line| line.split_once("//").map_or(line, |(code, _)| code))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(!code.contains('"'), "rule.rs has a string literal");
    for forbidden in [
        "std::fs",
        "std::io",
        "std::net",
        "std::process",
        "std::env",
        "tokio",
        "tracing",
        "File",
        "include_str",
    ] {
        assert!(!code.contains(forbidden), "rule.rs names {forbidden}");
    }
    for constant in ["NONE", "QUEUED", "BLOCKED", "DONE"] {
        assert!(code.contains(constant), "rule.rs does not name {constant}");
    }
    for constant in [
        "PLANNING",
        "AWAITING_APPROVAL",
        "PLAN_APPROVED",
        "AWAITING_MERGE",
        "REQUIRED_OPTIONS",
    ] {
        assert!(!code.contains(constant), "rule.rs names {constant}");
    }
}
