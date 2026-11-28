//! The project setup file: `load_project_setup` and `BranchName::is_valid` against the case table
//! `conformance/project-setup.json`, which `.mintswarm/dogfood/github/selftest.sh` also runs
//! through `.mintswarm/dogfood/project-setup.py`, and the live setup file.

#![cfg(test)]

use mintswarm_config::{Error, load_project_setup};
use mintswarm_domain::vcs::BranchName;
use serde_json::Value;

const TABLE: &str = include_str!("conformance/project-setup.json");

fn table() -> Value {
    serde_json::from_str(TABLE).expect("project-setup.json is JSON")
}

fn names(list: &str) -> Vec<String> {
    let names: Vec<String> = table()[list]
        .as_array()
        .expect("a list of names")
        .iter()
        .map(|name| name.as_str().expect("a name").to_owned())
        .collect();
    assert!(!names.is_empty(), "{list} is empty");
    names
}

/// `text` as a JSON string with every character outside printable ASCII, and `"` and `\`, as
/// `\uXXXX` (with UTF-16 surrogate pairs above U+FFFF), so that a YAML reader takes it unchanged.
fn json_string(text: &str) -> String {
    let mut out = String::from("\"");
    for c in text.chars() {
        if (' '..='~').contains(&c) && c != '"' && c != '\\' {
            out.push(c);
        } else {
            let mut units = [0u16; 2];
            for unit in c.encode_utf16(&mut units) {
                out.push_str(&format!("\\u{unit:04x}"));
            }
        }
    }
    out.push('"');
    out
}

/// The setup document naming `name` as the work branch, in JSON, which is valid YAML.
fn document(name: &str) -> String {
    format!("{{\"branches\":{{\"work\":{}}}}}", json_string(name))
}

#[test]
fn the_helper_escapes_every_character_outside_printable_ascii() {
    assert_eq!(json_string("a\u{85}b"), r#""a\u0085b""#);
    assert_eq!(json_string("a\"\\\u{7f}"), "\"a\\u0022\\u005c\\u007f\"");
    assert_eq!(json_string("\u{1f600}"), "\"\\ud83d\\ude00\"");
    for name in ["a\u{85}b", "a\"\\\u{7f}", "\u{1f600}", "é"] {
        let back: String = serde_json::from_str(&json_string(name)).unwrap();
        assert_eq!(back, name);
    }
}

#[test]
fn valid_names_are_valid_and_load() {
    for name in names("valid_names") {
        assert!(BranchName::is_valid(&name), "{name:?}");
        let setup =
            load_project_setup(&document(&name)).unwrap_or_else(|e| panic!("{name:?}: {e}"));
        assert_eq!(setup.work_branch, BranchName(name));
    }
}

#[test]
fn invalid_names_are_refused_at_the_work_branch() {
    for list in ["invalid_names", "invalid_by_rule"] {
        for name in names(list) {
            assert!(!BranchName::is_valid(&name), "{list}: {name:?}");
            let error = load_project_setup(&document(&name)).unwrap_err();
            assert!(matches!(error, Error::Schema { .. }), "{name:?}: {error:?}");
            assert_eq!(error.path(), "branches.work", "{name:?}");
        }
    }
}

#[test]
fn documents_fail_at_their_paths() {
    let documents = table()["documents"].as_array().expect("documents").clone();
    assert!(!documents.is_empty());
    for case in documents {
        let name = case["name"].as_str().expect("a name");
        let yaml = case["yaml"].as_str().expect("a YAML text");
        let path = case["path"].as_str().expect("a path");
        let error = load_project_setup(yaml).unwrap_err();
        assert!(matches!(error, Error::Schema { .. }), "{name}: {error:?}");
        assert_eq!(error.path(), path, "{name}");
    }
}

#[test]
fn the_live_setup_names_master() {
    let file = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../.mintswarm/dogfood/project-setup.yaml");
    let text = std::fs::read_to_string(&file).expect("the live setup file reads");
    let setup = load_project_setup(&text).unwrap();
    assert_eq!(setup.work_branch, BranchName("master".to_owned()));
}
