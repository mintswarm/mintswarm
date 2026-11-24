//! `gates.format`: the formatters, each with its paths and the `gates.run` entry that checks it.
//! The variants are built from yq's JSON for the repository config, whose `gates.run` holds
//! `rust-format` and `rust-test`.

#![cfg(test)]

use mintswarm_config::{Formatter, RepoConfig, load_repo_config};
use serde_json::{Value, json};

const REPO_JSON: &str = include_str!("conformance/repo-config.json");

fn real() -> Value {
    serde_json::from_str(REPO_JSON).expect("repo-config.json is JSON")
}

/// The real config with `gates.format` set to `format`.
fn with_format(format: Value) -> Value {
    let mut config = real();
    config
        .pointer_mut("/gates")
        .and_then(Value::as_object_mut)
        .expect("the config has a gates mapping")
        .insert("format".to_owned(), format);
    config
}

fn load(config: Value) -> Result<RepoConfig, mintswarm_config::Error> {
    let text = serde_json::to_string_pretty(&config).expect("a JSON value serialises");
    load_repo_config(&text)
}

fn rustfmt() -> Value {
    json!({
        "name": "rustfmt",
        "command": "cargo fmt --all",
        "paths": ["**/*.rs"],
        "gate": "rust-format",
    })
}

/// `rustfmt()` with `change` applied to its mapping.
fn entry(change: impl FnOnce(&mut serde_json::Map<String, Value>)) -> Value {
    let mut entry = rustfmt();
    change(entry.as_object_mut().expect("the entry is a mapping"));
    entry
}

#[test]
fn formatters_load_with_their_fields() {
    let config = with_format(json!([
        rustfmt(),
        // The gate only has to be a `gates.run` name; the snapshot's are rust-format and rust-test.
        {
            "name": "taplo",
            "command": "taplo fmt",
            "paths": ["**/Cargo.toml", "rustfmt.toml"],
            "gate": "rust-test",
        },
    ]));
    let loaded = load(config).expect("the config loads");
    assert_eq!(
        loaded.formatters(),
        [
            Formatter {
                name: "rustfmt".to_owned(),
                command: "cargo fmt --all".to_owned(),
                paths: vec!["**/*.rs".to_owned()],
                gate: "rust-format".to_owned(),
            },
            Formatter {
                name: "taplo".to_owned(),
                command: "taplo fmt".to_owned(),
                paths: vec!["**/Cargo.toml".to_owned(), "rustfmt.toml".to_owned()],
                gate: "rust-test".to_owned(),
            },
        ]
    );
}

#[test]
fn missing_gate_names_entry_and_gate() {
    let config = with_format(json!([entry(|e| {
        e.insert("gate".to_owned(), json!("no-such-gate"));
    })]));
    let error = load(config).expect_err("an unknown gate is rejected");
    assert_eq!(error.path(), "gates.format[0].gate");
    assert!(error.message().contains("rustfmt"), "{error}");
    assert!(error.message().contains("no-such-gate"), "{error}");
}

#[test]
fn invalid_entries_are_rejected() {
    type Case = (&'static str, Value, &'static str);
    let set = |key: &'static str, value: Value| {
        entry(move |e| {
            e.insert(key.to_owned(), value);
        })
    };
    let without = |key: &'static str| {
        entry(move |e| {
            e.remove(key);
        })
    };
    let cases: Vec<Case> = vec![
        (
            "unknown key",
            json!([set("extra", json!(true))]),
            "gates.format[0].extra",
        ),
        (
            "missing command",
            json!([without("command")]),
            "gates.format[0].command",
        ),
        (
            "empty command",
            json!([set("command", json!(""))]),
            "gates.format[0].command",
        ),
        (
            "whitespace-only command",
            json!([set("command", json!("  \t"))]),
            "gates.format[0].command",
        ),
        (
            "unsplittable command",
            json!([set("command", json!("echo 'a"))]),
            "gates.format[0].command",
        ),
        (
            "missing paths",
            json!([without("paths")]),
            "gates.format[0].paths",
        ),
        (
            "empty paths",
            json!([set("paths", json!([]))]),
            "gates.format[0].paths",
        ),
        (
            "non-string path",
            json!([set("paths", json!(["**/*.rs", 3]))]),
            "gates.format[0].paths",
        ),
        (
            "empty path",
            json!([set("paths", json!([""]))]),
            "gates.format[0].paths",
        ),
        (
            "missing name",
            json!([without("name")]),
            "gates.format[0].name",
        ),
        (
            "empty name",
            json!([set("name", json!(""))]),
            "gates.format[0].name",
        ),
        (
            "whitespace-only name",
            json!([set("name", json!("  \t"))]),
            "gates.format[0].name",
        ),
        (
            "non-string name",
            json!([set("name", json!(1))]),
            "gates.format[0].name",
        ),
        (
            "missing gate",
            json!([without("gate")]),
            "gates.format[0].gate",
        ),
        (
            "non-string gate",
            json!([set("gate", json!(1))]),
            "gates.format[0].gate",
        ),
        ("entry not a mapping", json!(["rustfmt"]), "gates.format[0]"),
        (
            "duplicate name",
            json!([rustfmt(), set("command", json!("rustfmt"))]),
            "gates.format[1].name",
        ),
    ];
    for (label, format, path) in cases {
        let error = load(with_format(format)).expect_err(label);
        assert_eq!(error.path(), path, "{label}: {error}");
    }
}

#[test]
fn duplicate_name_names_the_first_entry() {
    let error = load(with_format(json!([rustfmt(), rustfmt()]))).expect_err("a duplicate");
    assert_eq!(
        error.message(),
        "rustfmt is already the name of gates.format[0]"
    );
}

#[test]
fn absent_null_or_empty_format_reads_as_none() {
    let unchanged = load(real()).expect("the real config loads");
    assert!(unchanged.formatters().is_empty());
    assert_eq!(unchanged.value(), &real());
    for format in [Value::Null, json!([])] {
        let loaded = load(with_format(format.clone())).expect("the config loads");
        assert!(loaded.formatters().is_empty(), "{format}");
    }
}

#[test]
fn non_list_format_is_rejected() {
    let error = load(with_format(json!({"name": "rustfmt"}))).expect_err("a mapping");
    assert_eq!(error.path(), "gates.format");
    assert_eq!(error.message(), "must be a list, not a mapping");
}
