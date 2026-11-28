//! `tenets` is accepted as any mapping while its format settles in mintswarm/tenets, which
//! defines and validates the section.

#![cfg(test)]

use mintswarm_config::load_repo_config;
use serde_json::{Value, json};

const REPO_JSON: &str = include_str!("conformance/repo-config.json");

fn load_with(tenets: Value) -> Result<(), mintswarm_config::Error> {
    let mut config: Value = serde_json::from_str(REPO_JSON).expect("repo-config.json is JSON");
    config
        .as_object_mut()
        .expect("the config is a mapping")
        .insert("tenets".to_owned(), tenets);
    let text = serde_json::to_string_pretty(&config).expect("a JSON value serialises");
    load_repo_config(&text).map(|_| ())
}

#[test]
fn any_mapping_is_accepted() {
    load_with(json!({})).expect("an empty mapping loads");
    load_with(json!({ "sources": [{ "pack": "@mintswarm/tenets:rust", "anything": [1, 2] }] }))
        .expect("the section's contents are not checked");
}

#[test]
fn anything_but_a_mapping_is_refused() {
    for tenets in [json!([]), json!("rust"), json!(1)] {
        let Err(error) = load_with(tenets.clone()) else {
            panic!("{tenets} loads");
        };
        assert_eq!(error.path(), "tenets", "{tenets}");
    }
}
