//! The JSON-in-YAML result block form: each block under `tests/fixtures/result-blocks/` parses
//! with `yaml-rust2`, and its `msw-result` mapping equals the case's `result.json`.
//!
//! Each directory there is one case: `block.yaml` holds a result block in the JSON-in-YAML form,
//! and `result.json` holds what it means.
//!
//! `result.json` is the block in its one JSON form: the `msw-result` mapping after it is read as
//! a `StageOutcome` and written back, so every key is present, optional fields the block leaves
//! out or sets to `null` appear as `null`, and absent or `null` lists as `[]`. The raw YAML tree
//! equals `result.json` only after those defaults are applied; every other reader of these
//! fixtures, such as the Python self-test, must apply the same defaults before it compares.

use std::path::{Path, PathBuf};

use mintswarm_domain::work::StageOutcome;
use serde_json::{Map, Number, Value};
use yaml_rust2::{Yaml, YamlLoader};

fn cases() -> Vec<PathBuf> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/result-blocks");
    let mut cases: Vec<PathBuf> = std::fs::read_dir(&root)
        .expect("the result-block fixtures are readable")
        .map(|entry| entry.expect("the fixture entry is readable").path())
        .filter(|path| path.is_dir())
        .collect();
    cases.sort();
    cases
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{} is readable: {e}", path.display()))
}

/// A YAML node as JSON; panics on anything a result block cannot hold.
fn to_json(yaml: &Yaml) -> Value {
    match yaml {
        Yaml::Hash(entries) => {
            let mut map = Map::new();
            for (key, value) in entries {
                let Yaml::String(key) = key else {
                    panic!("a mapping key is not a string: {key:?}");
                };
                map.insert(key.clone(), to_json(value));
            }
            Value::Object(map)
        }
        Yaml::Array(items) => Value::Array(items.iter().map(to_json).collect()),
        Yaml::String(text) => Value::String(text.clone()),
        Yaml::Integer(number) => Value::Number(Number::from(*number)),
        Yaml::Real(text) => {
            let number: f64 = text.parse().expect("the real is a number");
            Value::Number(Number::from_f64(number).expect("the real is finite"))
        }
        Yaml::Boolean(flag) => Value::Bool(*flag),
        Yaml::Null => Value::Null,
        other => panic!("a result block cannot hold {other:?}"),
    }
}

/// Whether the block carries `questions` as a multi-line array, every continuation line,
/// the closing `]` included, indented by four spaces.
fn has_multi_line_questions(block: &str) -> bool {
    let mut lines = block
        .lines()
        .skip_while(|line| !line.starts_with("  questions: ["));
    lines
        .next()
        .is_some_and(|first| first.trim_end() == "  questions: [")
        && lines
            .take_while(|line| line.starts_with("    "))
            .any(|line| line.trim() == "]")
}

#[test]
fn result_blocks_match_their_result_json() {
    let cases = cases();
    assert!(cases.len() >= 3, "too few result-block cases: {cases:?}");
    let mut multi_line = 0;
    for case in &cases {
        let block = read(&case.join("block.yaml"));
        if has_multi_line_questions(&block) {
            multi_line += 1;
        }
        let documents = YamlLoader::load_from_str(&block)
            .unwrap_or_else(|e| panic!("{}: the block is YAML: {e}", case.display()));
        assert_eq!(documents.len(), 1, "{}: one document", case.display());
        let result = &documents[0]["msw-result"];
        assert!(
            matches!(result, Yaml::Hash(_)),
            "{}: msw-result is a mapping",
            case.display()
        );
        let parsed = to_json(result);
        let outcome: StageOutcome = serde_json::from_value(parsed.clone()).unwrap_or_else(|e| {
            panic!(
                "{}: the block is a StageOutcome: {e}\n{parsed}",
                case.display()
            )
        });

        let expected_text = read(&case.join("result.json"));
        let expected: StageOutcome = serde_json::from_str(&expected_text)
            .unwrap_or_else(|e| panic!("{}: result.json is a StageOutcome: {e}", case.display()));
        let expected_tree: Value =
            serde_json::from_str(&expected_text).expect("result.json is JSON");

        assert_eq!(outcome, expected, "{}", case.display());
        assert_eq!(
            serde_json::to_value(&outcome).expect("the outcome serialises"),
            expected_tree,
            "{}: result.json is the outcome's serde form",
            case.display()
        );
        assert_eq!(outcome.check(), Ok(()), "{}", case.display());
    }
    assert!(
        multi_line >= 1,
        "no case has a multi-line questions array indented by four spaces"
    );
}
