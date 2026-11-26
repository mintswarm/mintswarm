//! `tracker.state` loads into [`StateSettings`], and each kind of bad `options` list is rejected
//! at its key path.

#![cfg(test)]

use mintswarm_config::{
    AWAITING_APPROVAL, AWAITING_MERGE, PLAN_APPROVED, PLANNING, REQUIRED_OPTIONS, RepoConfig,
    StateOption, StatusMode, load_repo_config,
};
use mintswarm_domain::field::FieldSource;
use serde_json::{Value, json};

const REPO_JSON: &str = include_str!("conformance/repo-config.json");
const REPO_YAML: &str = include_str!("conformance/repo-config.yaml");

const STATE: &str = "/tracker/state";

/// A variant of the real config, written as pretty JSON.
fn variant(change: impl FnOnce(&mut Value)) -> Result<RepoConfig, mintswarm_config::Error> {
    let mut config: Value = serde_json::from_str(REPO_JSON).expect("repo-config.json is JSON");
    change(&mut config);
    load_repo_config(&(serde_json::to_string_pretty(&config).expect("JSON serialises") + "\n"))
}

fn state(config: &mut Value) -> &mut serde_json::Map<String, Value> {
    config
        .pointer_mut(STATE)
        .and_then(Value::as_object_mut)
        .expect("tracker.state is a mapping")
}

/// The real config with `options` set to the eight required names, then `extra` appended.
fn with_options(extra: Value) -> Result<RepoConfig, mintswarm_config::Error> {
    variant(|c| {
        let mut options = vec![
            json!({"name": "none"}),
            json!({"name": "queued", "status": "Todo"}),
            json!({"name": "blocked", "status": "Todo"}),
            json!({"name": "planning", "status": "In progress"}),
            json!({"name": "awaiting approval", "status": "In review"}),
            json!({"name": "plan approved", "status": "In progress"}),
            json!({"name": "awaiting merge", "status": "In review"}),
            json!({"name": "done", "status": "Done"}),
        ];
        options.extend(extra.as_array().expect("a list").iter().cloned());
        state(c).insert("options".to_owned(), Value::Array(options));
    })
}

#[test]
fn the_repository_config_loads_its_state_settings() {
    let config = load_repo_config(REPO_YAML).expect("the snapshot loads");
    let settings = config.state().expect("tracker.state is present");
    assert_eq!(settings.status, StatusMode::IssueStatus);
    assert_eq!(
        settings.source,
        FieldSource::Field {
            name: "msw-state".to_owned()
        }
    );
    assert_eq!(settings.options.len(), 13);
    assert_eq!(
        settings.option("none"),
        Some(&StateOption {
            name: "none".to_owned(),
            status: None
        })
    );
    assert_eq!(
        settings
            .option("awaiting approval")
            .and_then(|o| o.status.as_deref()),
        Some("In review")
    );
    assert_eq!(settings.option("Done"), None, "names match exactly");
    let names: Vec<String> = settings.options.iter().map(|o| o.name.clone()).collect();
    let field = settings.field_ref();
    assert_eq!(field.options, names);
    assert_eq!(field.source, settings.source);
}

#[test]
fn labels_mode_gives_the_label_pattern() {
    let config = variant(|c| {
        let block = state(c);
        block.insert("source".to_owned(), json!("labels"));
        block.insert("status".to_owned(), json!("off"));
        block.remove("field");
    })
    .expect("labels mode loads");
    let settings = config.state().expect("tracker.state is present");
    assert_eq!(
        settings.source,
        FieldSource::Labels {
            label: "msw-state:{option}".to_owned()
        }
    );
    assert_eq!(settings.status, StatusMode::Off);
}

#[test]
fn an_absent_state_block_gives_none() {
    let config = variant(|c| {
        c.pointer_mut("/tracker")
            .and_then(Value::as_object_mut)
            .expect("tracker is a mapping")
            .remove("state");
    })
    .expect("a config without tracker.state loads");
    assert!(config.state().is_none());
}

#[test]
fn the_required_names() {
    assert_eq!(
        REQUIRED_OPTIONS,
        [
            "none",
            "queued",
            "blocked",
            "planning",
            "awaiting approval",
            "plan approved",
            "awaiting merge",
            "done"
        ]
    );
}

#[test]
fn bad_options_are_rejected_at_their_path() {
    let options = "tracker.state.options";
    let cases = [
        (
            "exact duplicate",
            json!([{"name": "parked"}, {"name": "parked"}]),
            format!("{options}[9].name"),
        ),
        (
            "duplicate in another case",
            json!([{"name": "parked"}, {"name": "PARKED"}]),
            format!("{options}[9].name"),
        ),
        (
            "empty name",
            json!([{"name": ""}]),
            format!("{options}[8].name"),
        ),
        (
            "blank name",
            json!([{"name": " \t"}]),
            format!("{options}[8].name"),
        ),
        (
            "number status",
            json!([{"name": "parked", "status": 1}]),
            format!("{options}[8].status"),
        ),
        (
            "null status",
            json!([{"name": "parked", "status": null}]),
            format!("{options}[8].status"),
        ),
        (
            "list status",
            json!([{"name": "parked", "status": ["Todo"]}]),
            format!("{options}[8].status"),
        ),
    ];
    for (label, extra, path) in cases {
        let error = with_options(extra).expect_err(label);
        assert_eq!(error.path(), path, "{label}: {error}");
    }
}

#[test]
fn each_required_name_is_needed() {
    for required in REQUIRED_OPTIONS {
        let error = variant(|c| {
            let block = state(c);
            let options = block["options"].as_array_mut().expect("a list");
            options.retain(|o| o["name"] != required);
            options.push(json!({"name": "parked"}));
        })
        .expect_err(required);
        assert_eq!(error.path(), "tracker.state.options", "{required}: {error}");
        assert!(error.message().contains(required), "{required}: {error}");
    }
}

#[test]
fn approval_states_are_required() {
    let approval = [AWAITING_APPROVAL, PLAN_APPROVED, PLANNING, AWAITING_MERGE];
    for required in approval {
        let error = variant(|c| {
            let options = state(c)["options"].as_array_mut().expect("a list");
            options.retain(|o| o["name"] != required);
        })
        .expect_err(required);
        assert_eq!(error.path(), "tracker.state.options", "{required}: {error}");
        assert!(error.message().contains(required), "{required}: {error}");
    }
    let error = variant(|c| {
        let options = state(c)["options"].as_array_mut().expect("a list");
        options.retain(|o| !approval.iter().any(|name| o["name"] == *name));
    })
    .expect_err("all four missing");
    assert_eq!(error.path(), "tracker.state.options", "{error}");
    for required in approval {
        assert!(error.message().contains(required), "{required}: {error}");
    }
}

#[test]
fn missing_options_are_rejected() {
    let error = variant(|c| drop(state(c).remove("options"))).expect_err("no options");
    assert_eq!(error.path(), "tracker.state.options", "{error}");
}
