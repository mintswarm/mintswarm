//! The yq conformance suite: `parse_yaml` gives what yq v4.45.1 gives.
//!
//! The expected output was recorded once with the real yq, `yq (https://github.com/mikefarah/yq/)
//! version v4.45.1`, never with the PyYAML stand-in in `.mintswarm/dogfood/github/linux/`:
//!
//! ```sh
//! printf '%s' "$yaml" | yq -p=yaml -o=json .
//! ```
//!
//! For the cases in `conformance/cases.json`, `accepted` holds yq's output as JSON and `rejected`
//! holds yq's exit status with its output or error. `conformance/repo-config.yaml` is a byte copy
//! of `.mintswarm/config.yaml` and `conformance/repo-config.json` is yq's output for it.
//! `cargo test` never runs yq.

#![cfg(test)]

use mintswarm_config::parse_yaml;
use serde_json::Value;

const CASES: &str = include_str!("conformance/cases.json");
const REPO_YAML: &str = include_str!("conformance/repo-config.yaml");
const REPO_JSON: &str = include_str!("conformance/repo-config.json");
const LIVE_YAML: &str = include_str!("../../../.mintswarm/config.yaml");

fn cases(list: &str) -> Vec<Value> {
    let cases: Value = serde_json::from_str(CASES).expect("cases.json is JSON");
    cases[list].as_array().expect("a list of cases").clone()
}

fn text<'a>(case: &'a Value, key: &str) -> &'a str {
    case[key].as_str().expect("a string field")
}

#[test]
fn accepted_fixtures_match_yq() {
    let accepted = cases("accepted");
    assert!(!accepted.is_empty());
    for case in accepted {
        let name = text(&case, "name");
        let got = parse_yaml(text(&case, "yaml"));
        assert_eq!(got.as_ref().ok(), Some(&case["yq"]), "{name}: {got:?}");
    }
}

#[test]
fn repository_config_matches_yq() {
    let expected: Value = serde_json::from_str(REPO_JSON).expect("repo-config.json is JSON");
    assert_eq!(parse_yaml(REPO_YAML), Ok(expected));
}

/// The copy is the live config, so a change to `.mintswarm/config.yaml` fails here until
/// `repo-config.yaml` is copied again and `repo-config.json` re-recorded with yq.
#[test]
fn repository_config_copy_is_current() {
    assert!(
        LIVE_YAML == REPO_YAML,
        "conformance/repo-config.yaml differs from .mintswarm/config.yaml: copy it and re-record \
         repo-config.json with yq"
    );
}

#[test]
fn rejected_fixtures_are_rejected() {
    let rejected = cases("rejected");
    assert!(!rejected.is_empty());
    for case in rejected {
        let name = text(&case, "name");
        let got = parse_yaml(text(&case, "yaml"));
        assert!(got.is_err(), "{name}: accepted as {got:?}");
    }
}

#[test]
fn yq_errors_are_rejected() {
    for case in cases("accepted") {
        assert!(case.get("yq_exit").is_none(), "{}", text(&case, "name"));
    }
    let failing: Vec<Value> = cases("rejected")
        .into_iter()
        .filter(|case| case["yq_exit"] != 0)
        .collect();
    assert!(!failing.is_empty());
    for case in failing {
        assert!(
            parse_yaml(text(&case, "yaml")).is_err(),
            "{}",
            text(&case, "name")
        );
    }
}

#[test]
fn duplicate_key_names_path() {
    let error = parse_yaml("pipeline:\n  review:\n    max_rounds: 3\n    max_rounds: 4\n")
        .expect_err("a duplicate key");
    assert_eq!(error.path(), "pipeline.review.max_rounds");
    assert!(
        error
            .to_string()
            .starts_with("pipeline.review.max_rounds: duplicate key")
    );
}
