//! Parity with the dogfood floors: every config variant that `plan-approval/selftest.py` or
//! `landing/floor-selftest.py` builds and expects the floor to reject is rejected by
//! `load_repo_config`, and the variants they accept load. The variants are built as the
//! self-tests build them: from yq's JSON for the repository config, changed, and written as
//! pretty JSON.

#![cfg(test)]

use mintswarm_config::{GatesSummary, load_repo_config};
use serde_json::{Value, json};

const REPO_JSON: &str = include_str!("conformance/repo-config.json");

fn real() -> Value {
    serde_json::from_str(REPO_JSON).expect("repo-config.json is JSON")
}

/// The value at a JSON pointer, which the test expects to exist.
fn at<'a>(config: &'a mut Value, pointer: &str) -> &'a mut Value {
    config
        .pointer_mut(pointer)
        .unwrap_or_else(|| panic!("no {pointer} in the config"))
}

fn object<'a>(config: &'a mut Value, pointer: &str) -> &'a mut serde_json::Map<String, Value> {
    at(config, pointer)
        .as_object_mut()
        .unwrap_or_else(|| panic!("{pointer} is not a mapping"))
}

fn set(config: &mut Value, pointer: &str, key: &str, value: Value) {
    object(config, pointer).insert(key.to_owned(), value);
}

fn remove(config: &mut Value, pointer: &str, key: &str) {
    object(config, pointer).remove(key);
}

/// A variant of the real config, as the self-tests' `as_json` writes it.
fn variant(change: impl FnOnce(&mut Value)) -> String {
    let mut config = real();
    change(&mut config);
    serde_json::to_string_pretty(&config).expect("a JSON value serialises") + "\n"
}

const APPROVAL: &str = "/pipeline/plan_approval";
const LANDING: &str = "/pipeline/landing";

type Change = Box<dyn FnOnce(&mut Value)>;

fn approval_update(key: &'static str, value: Value) -> Change {
    Box::new(move |c| set(c, APPROVAL, key, value))
}

fn landing_update(key: &'static str, value: Value) -> Change {
    Box::new(move |c| set(c, LANDING, key, value))
}

fn factor(value: Value) -> Change {
    Box::new(move |c| set(c, &format!("{LANDING}/footprint"), "lines_factor", value))
}

fn plan_approval_rejected() -> Vec<(&'static str, Change)> {
    vec![
        (
            "no-plan-approval",
            Box::new(|c| remove(c, "/pipeline", "plan_approval")),
        ),
        ("unknown-key", approval_update("extra", json!(true))),
        (
            "mode-sometimes",
            approval_update("mode", json!("sometimes")),
        ),
        (
            "limit-negative",
            Box::new(|c| set(c, &format!("{APPROVAL}/limits"), "files", json!(-1))),
        ),
        (
            "limit-true",
            Box::new(|c| set(c, &format!("{APPROVAL}/limits"), "lines", json!(true))),
        ),
        (
            "undefined-component",
            Box::new(|c| {
                at(c, &format!("{APPROVAL}/require_human/components"))
                    .as_array_mut()
                    .expect("a list")
                    .push(json!("nowhere"));
            }),
        ),
        (
            "missing-limit",
            Box::new(|c| remove(c, &format!("{APPROVAL}/limits"), "new_files")),
        ),
        ("min-89", approval_update("min_confidence", json!(89))),
        ("min-101", approval_update("min_confidence", json!(101))),
        ("min-true", approval_update("min_confidence", json!(true))),
        ("min-string", approval_update("min_confidence", json!("95"))),
        (
            "lbc-undefined",
            approval_update("limits_by_component", json!({"nowhere": {"lines": 1}})),
        ),
        (
            "lbc-unknown-key",
            approval_update("limits_by_component", json!({"docs": {"size": 1}})),
        ),
        (
            "lbc-negative",
            approval_update("limits_by_component", json!({"docs": {"lines": -1}})),
        ),
        (
            "lbc-true",
            approval_update("limits_by_component", json!({"docs": {"lines": true}})),
        ),
        (
            "lbc-not-mapping",
            approval_update("limits_by_component", json!({"docs": 5})),
        ),
        (
            "lbc-list",
            approval_update("limits_by_component", json!([])),
        ),
        (
            "non-production-string",
            approval_update("non_production", json!("**/tests/**")),
        ),
        (
            "non-production-numbers",
            approval_update("non_production", json!([1])),
        ),
    ]
}

#[test]
fn plan_approval_selftest_variants_rejected() {
    let variants = plan_approval_rejected();
    assert_eq!(variants.len(), 19);
    for (name, change) in variants {
        let text = variant(change);
        let got = load_repo_config(&text);
        assert!(got.is_err(), "{name}: accepted");
    }
}

#[test]
fn landing_selftest_variants_rejected() {
    let footprint = || format!("{LANDING}/footprint");
    let variants: Vec<(&str, Change)> = vec![
        (
            "no-landing",
            Box::new(|c| remove(c, "/pipeline", "landing")),
        ),
        (
            "landing-null",
            Box::new(|c| set(c, "/pipeline", "landing", Value::Null)),
        ),
        (
            "landing-unknown-key",
            landing_update("post_merge_watch", json!(true)),
        ),
        (
            "require-human-unknown-key",
            Box::new(|c| set(c, &format!("{LANDING}/require_human"), "limits", json!([]))),
        ),
        (
            "footprint-unknown-key",
            Box::new(move |c| set(c, &footprint(), "files_factor", json!(2))),
        ),
        ("mode-sometimes", landing_update("mode", json!("sometimes"))),
        ("mode-missing", Box::new(|c| remove(c, LANDING, "mode"))),
        ("factor-0.9", factor(json!(0.9))),
        ("factor-true", factor(json!(true))),
        ("factor-string", factor(json!("1.5"))),
        ("factor-1e400", factor(json!("__RAW__1e400"))),
        ("factor-inf", factor(json!("__RAW__.inf"))),
        (
            "factor-missing",
            Box::new(|c| remove(c, &format!("{LANDING}/footprint"), "lines_factor")),
        ),
        (
            "footprint-missing",
            Box::new(|c| remove(c, LANDING, "footprint")),
        ),
        (
            "undefined-component",
            landing_update("require_human", json!({"components": ["nowhere"]})),
        ),
        (
            "plan-approval-invalid",
            approval_update("mode", json!("sometimes")),
        ),
        (
            "min-confidence-89",
            landing_update("min_confidence", json!(89)),
        ),
        (
            "min-confidence-101",
            landing_update("min_confidence", json!(101)),
        ),
        (
            "min-confidence-true",
            landing_update("min_confidence", json!(true)),
        ),
        (
            "min-confidence-string",
            landing_update("min_confidence", json!("95")),
        ),
        (
            "min-confidence-float",
            landing_update("min_confidence", json!(95.5)),
        ),
    ];
    assert_eq!(variants.len(), 21);
    for (name, change) in variants {
        let mut text = variant(change);
        // As the self-test splices raw text in place of a placeholder string.
        for raw in ["1e400", ".inf"] {
            text = text.replace(&format!("\"__RAW__{raw}\""), raw);
        }
        assert!(!text.contains("__RAW__"), "{name}: placeholder left");
        let got = load_repo_config(&text);
        assert!(got.is_err(), "{name}: accepted");
    }
}

#[test]
fn unusable_input_rejected() {
    for (name, text) in [
        ("not a mapping", "[1, 2]\n"),
        ("does not convert", ".inf\n"),
        ("empty", ""),
    ] {
        assert!(load_repo_config(text).is_err(), "{name}: accepted");
    }
}

#[test]
fn selftest_accepted_variants_load() {
    let variants: Vec<(&str, Change)> = vec![
        ("always", approval_update("mode", json!("always"))),
        ("judged", approval_update("mode", json!("judged"))),
        ("never", approval_update("mode", json!("never"))),
        (
            "judged-95",
            Box::new(|c| {
                set(c, APPROVAL, "mode", json!("judged"));
                set(c, APPROVAL, "min_confidence", json!(95));
            }),
        ),
        (
            "min-100",
            Box::new(|c| {
                set(c, APPROVAL, "mode", json!("judged"));
                set(c, APPROVAL, "min_confidence", json!(100));
            }),
        ),
        (
            "component-limits",
            approval_update("limits_by_component", json!({"docs": {"lines": 400}})),
        ),
        (
            "tested",
            Box::new(|c| set(c, "/gates", "tests", json!("cargo test"))),
        ),
        (
            "untested",
            Box::new(|c| set(c, "/gates", "tests", json!("none"))),
        ),
        (
            "cd-gates",
            Box::new(|c| {
                let run = at(c, "/gates/run").as_array_mut().expect("a list");
                run.push(json!({"name": "web-script", "command": "cd ./web && bash x.sh"}));
                run.push(json!({"name": "nested-script", "command": "cd ./web && cd ./apps && bash y.sh"}));
            }),
        ),
        (
            "dropped",
            Box::new(|c| {
                at(c, &format!("{APPROVAL}/require_human/paths"))
                    .as_array_mut()
                    .expect("a list")
                    .retain(|path| path != ".mintswarm/dogfood/github/**");
            }),
        ),
        ("shadow", landing_update("mode", json!("shadow"))),
        ("factor-1.15", factor(json!(1.15))),
        ("factor-1", factor(json!(1))),
        (
            "rh-web",
            landing_update("require_human", json!({"components": ["web"]})),
        ),
        ("no-rh", landing_update("require_human", json!({}))),
        (
            "min-confidence-90",
            landing_update("min_confidence", json!(90)),
        ),
        (
            "min-confidence-100",
            landing_update("min_confidence", json!(100)),
        ),
    ];
    for (name, change) in variants {
        let text = variant(change);
        if let Err(error) = load_repo_config(&text) {
            panic!("{name}: rejected: {error}");
        }
    }
    let config = load_repo_config(&variant(factor(json!(1.15)))).expect("factor-1.15 loads");
    assert_eq!(config.landing().lines_factor, 1.15);
    let config = load_repo_config(&variant(approval_update(
        "limits_by_component",
        json!({"docs": {"lines": 400}}),
    )))
    .expect("component-limits loads");
    assert_eq!(
        config.plan_approval().limits_by_component["docs"].lines,
        Some(400)
    );
}

#[test]
fn unknown_key_names_path() {
    let cases: Vec<(&str, Change)> = vec![
        ("extra", Box::new(|c| set(c, "", "extra", json!(1)))),
        (
            "tracker.kinds.design.extra",
            Box::new(|c| set(c, "/tracker/kinds/design", "extra", json!("x"))),
        ),
        (
            "components.domain.extra",
            Box::new(|c| set(c, "/components/domain", "extra", json!("x"))),
        ),
        (
            "pipeline.plan_approval.extra",
            approval_update("extra", json!(true)),
        ),
        (
            "pipeline.plan_approval.limits_by_component.docs.size",
            approval_update("limits_by_component", json!({"docs": {"size": 1}})),
        ),
        (
            "pipeline.landing.footprint.files_factor",
            Box::new(|c| set(c, &format!("{LANDING}/footprint"), "files_factor", json!(2))),
        ),
        (
            "gates.run[0].nme",
            Box::new(|c| set(c, "/gates/run/0", "nme", json!("x"))),
        ),
        (
            "gates.report.converters.x.extra",
            Box::new(|c| {
                set(
                    c,
                    "/gates/report",
                    "converters",
                    json!({"x": {"command": ["x"], "extension": "x", "extra": 1}}),
                );
            }),
        ),
    ];
    for (path, change) in cases {
        let error = load_repo_config(&variant(change)).expect_err(path);
        assert_eq!(error.path(), path, "{error}");
        assert_eq!(error.to_string(), format!("{path}: unknown key"));
    }
}

#[test]
fn gates_summary_values() {
    let summary = |value: Option<Value>| {
        load_repo_config(&variant(|c| {
            if let Some(value) = value {
                set(c, "/gates", "summary", value);
            }
        }))
    };
    let off = summary(None).expect("absent loads");
    assert_eq!(off.gates_summary(), GatesSummary::Off);
    let agent = summary(Some(json!("agent"))).expect("agent loads");
    assert_eq!(agent.gates_summary(), GatesSummary::Agent);
    let off = summary(Some(json!("off"))).expect("off loads");
    assert_eq!(off.gates_summary(), GatesSummary::Off);
    for value in [json!("on"), json!(true), json!("Agent"), json!(1)] {
        let error = summary(Some(value.clone())).expect_err("rejected");
        assert_eq!(error.path(), "gates.summary", "{value}: {error}");
    }
    let error = load_repo_config(SUMMARY_ON).expect_err("on is a string, not agent or off");
    assert_eq!(error.path(), "gates.summary", "{error}");
}

/// A config with `summary: on`, which yq reads as the string "on" (YAML 1.2), not a bool.
const SUMMARY_ON: &str = "components: {}\npipeline:\n  plan_approval:\n    mode: never\n    limits: {files: 1, lines: 1, new_files: 1, new_dependencies: 0}\n  landing:\n    mode: never\n    footprint: {lines_factor: 1}\ngates:\n  summary: on\n";

#[test]
fn max_rounds_values() {
    for (name, change) in [
        ("zero", Some(json!(0))),
        ("true", Some(json!(true))),
        ("missing", None),
    ] {
        let error = load_repo_config(&variant(|c| match change {
            Some(value) => set(c, "/pipeline/review", "max_rounds", value),
            None => remove(c, "/pipeline/review", "max_rounds"),
        }))
        .expect_err(name);
        assert_eq!(
            error.path(),
            "pipeline.review.max_rounds",
            "{name}: {error}"
        );
    }
    let config = load_repo_config(&variant(|_| {})).expect("the real config loads");
    assert_eq!(config.review_max_rounds(), Some(3));
}

#[test]
fn dependencies_and_report_rejected() {
    let cases: Vec<(&str, Change)> = vec![
        (
            "pipeline.dependencies.review",
            Box::new(|c| set(c, "/pipeline/dependencies", "review", json!("sometimes"))),
        ),
        (
            "gates.report.converters.ctrf",
            Box::new(|c| {
                set(
                    c,
                    "/gates/report",
                    "converters",
                    json!({"ctrf": {"command": ["x"], "extension": "json"}}),
                );
            }),
        ),
        (
            "gates.report.converters.sarif.extension",
            Box::new(|c| {
                set(
                    c,
                    "/gates/report",
                    "converters",
                    json!({"sarif": {"command": ["x"], "extension": "x.y"}}),
                );
            }),
        ),
    ];
    for (path, change) in cases {
        let error = load_repo_config(&variant(change)).expect_err(path);
        assert_eq!(error.path(), path, "{error}");
    }
}

#[test]
fn live_repository_config_loads() {
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.mintswarm/config.yaml");
    let text = std::fs::read_to_string(&path).expect("the repository config is readable");
    if let Err(error) = load_repo_config(&text) {
        panic!("{}: {error}", path.display());
    }
}
