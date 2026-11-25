#![cfg(test)]
//! Checks the protocol scenario list: every required scenario is present, the
//! names are unique, and each scenario acts and ends with an expectation.

use std::collections::BTreeSet;

use mintswarm_proto::fakes::scenarios::{SCENARIOS, Scenario, Step};

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

fn scenario(name: &str) -> &'static Scenario {
    SCENARIOS
        .iter()
        .find(|scenario| scenario.name == name)
        .unwrap_or_else(|| panic!("no scenario named {name}"))
}

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

#[test]
fn needs_assignments_tells_work_from_stream_only_scenarios() {
    assert!(scenario("lease_lost_mid_step").needs_assignments());
    assert!(!scenario("reconnect_and_replay").needs_assignments());
}
