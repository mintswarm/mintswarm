//! `tracker.ingest` loads into a [`Trust`], and [`Trust::trusts`] combines the trusted labels
//! and authors as `match` says.

#![cfg(test)]

use mintswarm_config::{Trust, TrustMatch, load_repo_config};
use mintswarm_domain::ids::IssueNumber;
use mintswarm_domain::tracker::{Issue, Login};
use serde_json::{Value, json};

const REPO_JSON: &str = include_str!("conformance/repo-config.json");

const INGEST: &str = "/tracker/ingest";

/// A variant of the real config, written as pretty JSON.
fn variant(change: impl FnOnce(&mut Value)) -> String {
    let mut config: Value = serde_json::from_str(REPO_JSON).expect("repo-config.json is JSON");
    change(&mut config);
    serde_json::to_string_pretty(&config).expect("a JSON value serialises") + "\n"
}

fn ingest(config: &mut Value) -> &mut serde_json::Map<String, Value> {
    config
        .pointer_mut(INGEST)
        .and_then(Value::as_object_mut)
        .expect("tracker.ingest is a mapping")
}

fn trust_of(text: &str) -> Trust {
    load_repo_config(text)
        .expect("the config loads")
        .trust()
        .clone()
}

fn issue(labels: &[&str], author: &str) -> Issue {
    Issue {
        number: IssueNumber(1),
        title: "An issue".to_owned(),
        body: String::new(),
        state: "open".to_owned(),
        labels: labels.iter().map(|label| (*label).to_owned()).collect(),
        issue_type: None,
        author: Login(author.to_owned()),
        url: "https://example.invalid/issues/1".to_owned(),
        sub_issues: 0,
        blocked_by: 0,
    }
}

fn trust(r#match: TrustMatch) -> Trust {
    Trust {
        labels: vec!["mintswarm".to_owned()],
        authors: vec![Login("zmij".to_owned())],
        r#match,
    }
}

#[test]
fn ingest_loads_into_trust() {
    assert_eq!(trust_of(REPO_JSON), trust(TrustMatch::Any));

    let all = variant(|c| {
        ingest(c).insert("match".to_owned(), json!("all"));
    });
    assert_eq!(trust_of(&all), trust(TrustMatch::All));

    let no_match = variant(|c| {
        ingest(c).remove("match");
    });
    assert_eq!(trust_of(&no_match), trust(TrustMatch::Any));

    let no_ingest = variant(|c| {
        c.pointer_mut("/tracker")
            .and_then(Value::as_object_mut)
            .expect("tracker is a mapping")
            .remove("ingest");
    });
    assert_eq!(trust_of(&no_ingest), Trust::default());
    assert_eq!(
        Trust::default(),
        Trust {
            labels: Vec::new(),
            authors: Vec::new(),
            r#match: TrustMatch::Any,
        }
    );
}

#[test]
fn match_other_than_any_or_all_is_rejected() {
    for value in [
        json!("some"),
        json!("ANY"),
        json!(true),
        json!(1),
        Value::Null,
    ] {
        let text = variant(|c| {
            ingest(c).insert("match".to_owned(), value.clone());
        });
        let error = load_repo_config(&text).expect_err("the config is rejected");
        assert_eq!(error.path(), "tracker.ingest.match", "match: {value}");
    }
}

#[test]
fn any_accepts_label_or_author() {
    let trust = trust(TrustMatch::Any);
    assert!(trust.trusts(&issue(&["mintswarm"], "someone")));
    assert!(trust.trusts(&issue(&[], "zmij")));
    assert!(!trust.trusts(&issue(&["other"], "someone")));
    assert!(trust.trusts(&issue(&["Mintswarm"], "someone")));
    assert!(trust.trusts(&issue(&[], "ZMIJ")));
    assert!(!Trust::default().trusts(&issue(&["mintswarm"], "zmij")));
}

#[test]
fn all_requires_both() {
    let trust = trust(TrustMatch::All);
    assert!(trust.trusts(&issue(&["other", "mintswarm"], "zmij")));
    assert!(!trust.trusts(&issue(&["mintswarm"], "someone")));
    assert!(!trust.trusts(&issue(&[], "zmij")));
    assert!(!trust.trusts(&issue(&["other"], "someone")));

    let no_authors = Trust {
        authors: Vec::new(),
        ..trust.clone()
    };
    assert!(!no_authors.trusts(&issue(&["mintswarm"], "zmij")));
    let no_labels = Trust {
        labels: Vec::new(),
        ..trust
    };
    assert!(!no_labels.trusts(&issue(&["mintswarm"], "zmij")));
}
