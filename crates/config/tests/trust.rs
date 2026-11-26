//! `tracker.ingest` loads into a [`Trust`], and [`Trust::trusts`] combines the trusted labels
//! and authors as `match` says. `tracker.actions` loads into an [`ActionTable`] with a row for
//! every [`Action`], and the loader refuses a table that is not one.

#![cfg(test)]

use mintswarm_config::{
    Action, ActionTable, ActionTrust, People, Trust, TrustMatch, load_repo_config,
};
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

const TRACKER: &str = "/tracker";

/// The App the repository trusts to merge, and only to merge.
const STEWARD: &str = "mintswarm-dev-steward";

/// The row the repository's own config gives `action`.
fn repository_row(action: Action) -> ActionTrust {
    match action {
        Action::ApprovePlan | Action::ApproveDesign | Action::ApproveDecomposition => ActionTrust {
            people: People::IssueOwner,
            apps: Vec::new(),
        },
        Action::ApprovePr => ActionTrust {
            people: People::Codeowners,
            apps: Vec::new(),
        },
        Action::Merge => ActionTrust {
            people: People::None,
            apps: vec![STEWARD.to_owned()],
        },
    }
}

fn people_name(people: People) -> &'static str {
    People::NAMES
        .iter()
        .find(|(_, known)| *known == people)
        .map(|(name, _)| *name)
        .expect("every rule has a name")
}

/// `tracker.actions` as JSON, one row per action from `row`.
fn actions_value(mut row: impl FnMut(Action) -> ActionTrust) -> Value {
    let rows = Action::ALL
        .iter()
        .map(|&action| {
            let ActionTrust { people, apps } = row(action);
            (
                action.as_str().to_owned(),
                json!({ "people": people_name(people), "apps": apps }),
            )
        })
        .collect::<serde_json::Map<_, _>>();
    Value::Object(rows)
}

/// A variant of the real config whose `tracker.actions` is `actions`.
fn with_actions(change: impl FnOnce(&mut serde_json::Map<String, Value>)) -> String {
    variant(|c| {
        let mut actions = actions_value(repository_row);
        change(actions.as_object_mut().expect("the table is a mapping"));
        c.pointer_mut(TRACKER)
            .and_then(Value::as_object_mut)
            .expect("tracker is a mapping")
            .insert("actions".to_owned(), actions);
    })
}

fn row(
    actions: &mut serde_json::Map<String, Value>,
    action: Action,
) -> &mut serde_json::Map<String, Value> {
    actions
        .get_mut(action.as_str())
        .and_then(Value::as_object_mut)
        .expect("the row is a mapping")
}

fn refused_at(text: &str) -> String {
    load_repo_config(text)
        .expect_err("the config is refused")
        .path()
        .to_owned()
}

fn actions_path(action: Action) -> String {
    format!("tracker.actions.{}", action.as_str())
}

#[test]
fn actions_load_into_the_table() {
    let config = load_repo_config(&with_actions(|_| {})).expect("the config loads");
    for action in Action::ALL {
        assert_eq!(
            config.actions().get(action),
            &repository_row(action),
            "{action:?}"
        );
    }
    assert_eq!(config.actions(), &ActionTable::from_fn(repository_row));
}

#[test]
fn absent_actions_trust_no_one() {
    let config = load_repo_config(REPO_JSON).expect("the config loads");
    assert_eq!(config.actions(), &ActionTable::default());
    for action in Action::ALL {
        assert_eq!(
            ActionTable::default().get(action),
            &ActionTrust {
                people: People::None,
                apps: Vec::new(),
            }
        );
    }
}

#[test]
fn missing_action_is_refused() {
    for action in Action::ALL {
        let text = with_actions(|actions| {
            actions.remove(action.as_str());
        });
        assert_eq!(refused_at(&text), actions_path(action));
    }
}

#[test]
fn unknown_action_is_refused() {
    let text = with_actions(|actions| {
        actions.insert(
            "approve_release".to_owned(),
            json!({ "people": "none", "apps": [] }),
        );
    });
    assert_eq!(refused_at(&text), "tracker.actions.approve_release");
}

#[test]
fn row_keys_are_checked() {
    for action in Action::ALL {
        for key in ["people", "apps"] {
            let text = with_actions(|actions| {
                row(actions, action).remove(key);
            });
            assert_eq!(refused_at(&text), format!("{}.{key}", actions_path(action)));
        }
        let text = with_actions(|actions| {
            row(actions, action).insert("logins".to_owned(), json!([]));
        });
        assert_eq!(
            refused_at(&text),
            format!("{}.logins", actions_path(action))
        );
        let text = with_actions(|actions| {
            actions.insert(action.as_str().to_owned(), json!("none"));
        });
        assert_eq!(refused_at(&text), actions_path(action));
        let text = with_actions(|actions| {
            row(actions, action).insert("apps".to_owned(), json!([1]));
        });
        assert_eq!(refused_at(&text), format!("{}.apps", actions_path(action)));
    }
}

#[test]
fn app_outside_merge_is_refused() {
    for action in Action::ALL {
        let text = with_actions(|actions| {
            row(actions, action).insert("apps".to_owned(), json!([STEWARD]));
        });
        if action == Action::Merge {
            load_repo_config(&text).expect("merge may trust an App");
        } else {
            assert_eq!(refused_at(&text), format!("{}.apps", actions_path(action)));
        }
    }
}

#[test]
fn people_rule_outside_its_rows_is_refused() {
    for action in Action::ALL {
        for (name, people) in People::NAMES {
            let text = with_actions(|actions| {
                row(actions, action).insert("people".to_owned(), json!(name));
            });
            if people.allowed_in(action) {
                load_repo_config(&text).expect("the rule is allowed in the row");
            } else {
                assert_eq!(
                    refused_at(&text),
                    format!("{}.people", actions_path(action))
                );
            }
        }
        for value in [json!("maintainers"), json!(["none"]), Value::Null] {
            let text = with_actions(|actions| {
                row(actions, action).insert("people".to_owned(), value.clone());
            });
            assert_eq!(
                refused_at(&text),
                format!("{}.people", actions_path(action))
            );
        }
    }
    assert!(People::IssueOwner.allowed_in(Action::ApprovePlan));
    assert!(!People::Codeowners.allowed_in(Action::ApprovePlan));
    assert!(!People::IssueOwner.allowed_in(Action::ApprovePr));
    assert!(People::Codeowners.allowed_in(Action::ApprovePr));
    assert!(!People::Codeowners.allowed_in(Action::Merge));
}

#[test]
fn action_names_follow_the_variants() {
    for (n, (name, action)) in Action::NAMES.into_iter().enumerate() {
        assert_eq!(Action::ALL[n], action);
        assert_eq!(action.as_str(), name);
    }
}

#[test]
fn live_actions_trust_only_the_steward_to_merge() {
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.mintswarm/config.yaml");
    let text = std::fs::read_to_string(&path).expect("the repository config is readable");
    let config = load_repo_config(&text).expect("the repository config loads");
    for action in Action::ALL {
        let row = config.actions().get(action);
        if action == Action::Merge {
            assert_eq!(row.apps, [STEWARD], "{}", action.as_str());
        } else {
            assert!(row.apps.is_empty(), "{}", action.as_str());
        }
        assert_eq!(row, &repository_row(action), "{}", action.as_str());
    }
}
