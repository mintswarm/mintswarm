//! The embedded default activity rules parse and classify the common calls and every gate.

#![cfg(test)]

use mintswarm_config::load_repo_config;
use mintswarm_domain::facts::catalogue::{ActivitySource, Harness};
use mintswarm_rules::activity::{BASE_ACTIVITIES, Call, OTHER, Rules};
use mintswarm_runner::activity::default_activity_rules;

fn defaults() -> Rules {
    let rules = default_activity_rules().expect("the embedded defaults parse");
    Rules::new(&rules, &[])
}

/// Asserts that the defaults give `call` a base activity other than `other`, and returns it.
fn assert_classified(rules: &Rules, call: &Call) -> String {
    let classification = rules.classify(Harness::Claude, call);
    assert_eq!(classification.source, ActivitySource::Default, "{call:?}");
    assert_ne!(classification.activity, OTHER, "{call:?}");
    assert!(
        BASE_ACTIVITIES.contains(&classification.activity.as_str()),
        "{call:?}: {}",
        classification.activity
    );
    classification.activity
}

fn bash(command: &str) -> Call {
    Call::from_harness(Harness::Claude, "Bash", Some(command))
}

#[test]
fn the_embedded_defaults_parse() {
    let rules = default_activity_rules().expect("the embedded defaults parse");
    assert!(!rules.is_empty());
    for rule in &rules {
        assert!(
            BASE_ACTIVITIES.contains(&rule.activity.as_str()),
            "{rule:?}"
        );
    }
}

#[test]
fn the_common_calls_get_a_base_activity() {
    let rules = defaults();
    for tool in ["Read", "Write", "Edit"] {
        assert_classified(&rules, &Call::from_harness(Harness::Claude, tool, None));
    }
    for command in [
        "grep -n x y",
        "sed -n 1,5p f",
        "cat f",
        "ls -la",
        "cargo build",
    ] {
        assert_classified(&rules, &bash(command));
    }
    assert_eq!(
        assert_classified(&rules, &bash("git commit -m x")),
        "committing"
    );
    assert_eq!(
        assert_classified(&rules, &bash("python3 .mintswarm/dogfood/run-gates.py")),
        "testing"
    );
}

#[test]
fn every_live_gate_command_gets_a_base_activity() {
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.mintswarm/config.yaml");
    let text = std::fs::read_to_string(&path).expect("the repository config is readable");
    let config = load_repo_config(&text).expect("the repository config loads");
    let gates = config.value()["gates"]["run"]
        .as_array()
        .expect("gates.run is a list");
    assert!(!gates.is_empty());
    let rules = defaults();
    for gate in gates {
        let command = gate["command"]
            .as_str()
            .expect("a gate command is a string");
        assert_classified(&rules, &bash(command));
    }
}
