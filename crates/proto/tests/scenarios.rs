#![cfg(test)]
//! Checks the protocol scenario list: every required scenario is present, the
//! names are unique, and each scenario acts and ends with an expectation.

use std::collections::BTreeSet;

use mintswarm_proto::fakes::scenarios::{SCENARIOS, Step};

const EXPECTED: [&str; 15] = [
    "reconnect_and_replay",
    "lease_lost_mid_step",
    "duplicate_assignment",
    "cancel_during_commit",
    "push_credential_expired",
    "superseded_stream",
    "plane_restart_within_grace",
    "plane_restart_beyond_grace",
    "stale_event_after_regrant",
    "need_blobs",
    "seq_gap",
    "token_refresh",
    "upgrade_required",
    "new_outbox_epoch",
    "directive_latency",
];

#[test]
fn every_expected_scenario_is_present() {
    let names: BTreeSet<_> = SCENARIOS.iter().map(|scenario| scenario.name).collect();
    let missing: Vec<_> = EXPECTED
        .iter()
        .filter(|name| !names.contains(*name))
        .collect();
    assert!(missing.is_empty(), "missing scenarios: {missing:?}");
}

#[test]
fn names_are_unique() {
    let names: BTreeSet<_> = SCENARIOS.iter().map(|scenario| scenario.name).collect();
    assert_eq!(names.len(), SCENARIOS.len());
}

#[test]
fn every_scenario_holds_an_action() {
    for scenario in SCENARIOS {
        assert!(
            scenario
                .steps
                .iter()
                .any(|step| matches!(step, Step::Action(_))),
            "{} has no action",
            scenario.name
        );
    }
}

#[test]
fn every_scenario_ends_with_an_expectation() {
    for scenario in SCENARIOS {
        assert!(
            matches!(scenario.steps.last(), Some(Step::Expect(_))),
            "{} does not end with an expectation",
            scenario.name
        );
    }
}
