//! Parity with the dogfood floors: every config variant that `plan-approval/selftest.py` or
//! `landing/floor-selftest.py` builds and expects the floor to reject is rejected by
//! `load_repo_config`, and the variants they accept load. The variants are built as the
//! self-tests build them: from yq's JSON for the repository config, changed, and written as
//! pretty JSON.

#![cfg(test)]

use mintswarm_config::{GatesSummary, ReviewClear, load_repo_config};
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

/// The `config` variants `plan-approval/selftest.py` `build_commits` builds and the floor rejects:
/// those `check_unusable_input` lists, with `MIN_CONFIDENCE_INVALID`, `SIZE_INVALID` and
/// `REMOVED_LIMIT`.
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
        (
            "limit-new-dependencies",
            Box::new(|c| {
                set(
                    c,
                    &format!("{APPROVAL}/limits"),
                    "new_dependencies",
                    json!(0),
                )
            }),
        ),
        (
            "lbc-new-dependencies",
            approval_update(
                "limits_by_component",
                json!({"docs": {"new_dependencies": 0}}),
            ),
        ),
    ]
}

#[test]
fn plan_approval_selftest_variants_rejected() {
    let variants = plan_approval_rejected();
    // Seven named in `check_unusable_input`, the four of `MIN_CONFIDENCE_INVALID`, the eight of
    // `SIZE_INVALID` and the two of `REMOVED_LIMIT`; `variant_names_match_selftests` checks the
    // names.
    assert_eq!(variants.len(), 21);
    for (name, change) in variants {
        let text = variant(change);
        let got = load_repo_config(&text);
        assert!(got.is_err(), "{name}: accepted");
    }
}

/// The `config` variants `landing/floor-selftest.py` `build_commits` builds and the floor rejects.
fn landing_rejected() -> Vec<(&'static str, Change)> {
    let footprint = || format!("{LANDING}/footprint");
    vec![
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
    ]
}

#[test]
fn landing_selftest_variants_rejected() {
    let variants = landing_rejected();
    // The 21 rejected `variant(...)` calls of `landing/floor-selftest.py` `build_commits`;
    // `variant_names_match_selftests` checks the names.
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

/// The variants `plan-approval/selftest.py` `build_commits` builds and the floor accepts.
fn plan_approval_accepted() -> Vec<(&'static str, Change)> {
    vec![
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
            approval_update(
                "limits_by_component",
                json!({"docs": {"lines": 400}, "web": {"lines": 500, "files": 12}, "cli": {"lines": 400}}),
            ),
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
    ]
}

/// The variants `landing/floor-selftest.py` `build_commits` builds and the floor accepts.
fn landing_accepted() -> Vec<(&'static str, Change)> {
    vec![
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
    ]
}

#[test]
fn selftest_accepted_variants_load() {
    let variants = plan_approval_accepted()
        .into_iter()
        .chain(landing_accepted());
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
        json!({"docs": {"lines": 400}, "web": {"lines": 500, "files": 12}, "cli": {"lines": 400}}),
    )))
    .expect("component-limits loads");
    assert_eq!(
        config.plan_approval().limits_by_component["docs"].lines,
        Some(400)
    );
    assert_eq!(
        config.plan_approval().limits_by_component["web"].files,
        Some(12)
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
            "agents.stages.implment",
            Box::new(|c| set(c, "/agents/stages", "implment", json!({"model": "sonnet"}))),
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
    let null = summary(Some(Value::Null)).expect("null loads as absent");
    assert_eq!(null.gates_summary(), GatesSummary::Off);
    let text = SUMMARY_ON.replace("summary: on", "summary:");
    let empty = load_repo_config(&text).expect("summary with no value loads");
    assert_eq!(empty.gates_summary(), GatesSummary::Off);
    for value in [json!("on"), json!(true), json!("Agent"), json!(1)] {
        let error = summary(Some(value.clone())).expect_err("rejected");
        assert_eq!(error.path(), "gates.summary", "{value}: {error}");
    }
    let error = load_repo_config(SUMMARY_ON).expect_err("on is a string, not agent or off");
    assert_eq!(error.path(), "gates.summary", "{error}");
}

/// `tracker.state` as `msw-state.py` `settings` checks it, from the block `master` adds.
#[test]
fn tracker_state_values() {
    let base = || {
        json!({
            "source": "field",
            "field": "msw-state",
            "label": "msw-state:{value}",
            "status": "issue_status",
        })
    };
    let state = |change: &dyn Fn(&mut Value)| {
        let mut state = base();
        change(&mut state);
        load_repo_config(&variant(|c| set(c, "/tracker", "state", state)))
    };
    state(&|_| {}).expect("the field block, with no field_id, loads");
    state(&|s| {
        s["source"] = json!("labels");
        s["status"] = json!("off");
        s.as_object_mut().expect("a mapping").remove("field");
    })
    .expect("labels mode needs no field");
    let rejected: [(&str, &str, Option<Value>); 9] = [
        ("source", "source", Some(json!("project"))),
        ("source", "source", None),
        ("field", "field", Some(json!(""))),
        ("field", "field", Some(json!("  "))),
        ("field", "field", None),
        ("field_id", "field_id", Some(json!(50325307))),
        ("label", "label", Some(json!("msw-state"))),
        ("label", "label", Some(json!("{value}:{value}"))),
        ("status", "status", Some(json!("on"))),
    ];
    for (key, path, value) in rejected {
        let error = state(&|s| match &value {
            Some(value) => s[key] = value.clone(),
            None => drop(s.as_object_mut().expect("a mapping").remove(key)),
        })
        .expect_err("rejected");
        assert_eq!(
            error.path(),
            format!("tracker.state.{path}"),
            "{key} {value:?}: {error}"
        );
    }
    let error = state(&|s| s["colour"] = json!("ededed")).expect_err("unknown key");
    assert_eq!(error.path(), "tracker.state.colour", "{error}");
    let error = load_repo_config(&variant(|c| set(c, "/tracker", "state", json!("field"))))
        .expect_err("not a mapping");
    assert_eq!(error.path(), "tracker.state", "{error}");
}

/// `tracker.plan_verdict` and `tracker.decomp_verdict` as `msw-state.py` `settings` checks them:
/// `tracker.state`'s rules without `status`, and no `field_id`.
#[test]
fn tracker_verdict_values() {
    for (key, field) in [
        ("plan_verdict", "msw-plan"),
        ("decomp_verdict", "msw-decomp"),
    ] {
        let base = || {
            json!({
                "source": "field",
                "field": field,
                "label": format!("{field}:{{value}}"),
            })
        };
        let block = |change: &dyn Fn(&mut Value)| {
            let mut block = base();
            change(&mut block);
            load_repo_config(&variant(|c| set(c, "/tracker", key, block)))
        };
        block(&|_| {}).expect("the field block loads");
        block(&|b| b["source"] = json!("labels")).expect("a labels block with a field loads");
        block(&|b| {
            b["source"] = json!("labels");
            b.as_object_mut().expect("a mapping").remove("field");
        })
        .expect("labels mode needs no field");
        let rejected: [(&str, Option<Value>); 10] = [
            ("source", Some(json!("project"))),
            ("source", None),
            ("field", Some(json!(""))),
            ("field", Some(json!("  "))),
            ("field", None),
            ("label", Some(json!("msw-plan"))),
            ("label", Some(json!("{value}:{value}"))),
            ("label", None),
            ("status", Some(json!("off"))),
            ("field_id", Some(json!(50375240))),
        ];
        for (name, value) in rejected {
            let error = block(&|b| match &value {
                Some(value) => b[name] = value.clone(),
                None => drop(b.as_object_mut().expect("a mapping").remove(name)),
            })
            .expect_err("rejected");
            assert_eq!(
                error.path(),
                format!("tracker.{key}.{name}"),
                "{key} {name} {value:?}: {error}"
            );
        }
        let error = load_repo_config(&variant(|c| set(c, "/tracker", key, json!("field"))))
            .expect_err("not a mapping");
        assert_eq!(error.path(), format!("tracker.{key}"), "{error}");
    }
}

/// A config with `summary: on`, which yq reads as the string "on" (YAML 1.2), not a bool.
const SUMMARY_ON: &str = "components: {}\npipeline:\n  plan_approval:\n    mode: never\n    limits: {files: 1, lines: 1, new_files: 1}\n  landing:\n    mode: never\n    footprint: {lines_factor: 1}\ngates:\n  summary: on\n";

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
    let recorded = real()["pipeline"]["review"]["max_rounds"].as_u64();
    assert!(
        recorded.is_some(),
        "the snapshot sets pipeline.review.max_rounds"
    );
    assert_eq!(config.review_max_rounds(), recorded);
}

/// `pipeline.review.clear` as the readiness check's `review_config` accepts it: blocker, major,
/// minor or nit, and major when absent.
#[test]
fn review_clear_values() {
    let clear = |value: Option<Value>| {
        load_repo_config(&variant(|c| match value {
            Some(value) => set(c, "/pipeline/review", "clear", value),
            None => remove(c, "/pipeline/review", "clear"),
        }))
    };
    for (name, expected) in [
        ("blocker", ReviewClear::Blocker),
        ("major", ReviewClear::Major),
        ("minor", ReviewClear::Minor),
        ("nit", ReviewClear::Nit),
    ] {
        let config = clear(Some(json!(name))).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(config.review_clear(), expected, "{name}");
    }
    let absent = clear(None).expect("absent loads");
    assert_eq!(absent.review_clear(), ReviewClear::Major);
    let null = clear(Some(Value::Null)).expect("null loads as absent");
    assert_eq!(null.review_clear(), ReviewClear::Major);
    let no_review = load_repo_config(&variant(|c| remove(c, "/pipeline", "review")))
        .expect("no pipeline.review loads");
    assert_eq!(no_review.review_clear(), ReviewClear::Major);
    for value in [
        json!("trivial"),
        json!("Nit"),
        json!(""),
        json!(1),
        json!(true),
    ] {
        let error = clear(Some(value.clone())).expect_err("rejected");
        assert_eq!(error.path(), "pipeline.review.clear", "{value}: {error}");
    }
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

/// The variant names a dogfood self-test's `build_commits` builds, read from its source.
///
/// A name is a string literal passed to `variant`, a key of the dict constant that a
/// `for name, … in CONST.items():` loop passes, or an f-string over a `for name, value in (…):`
/// tuple list. Any other form of call fails, so a new form is noticed rather than skipped.
fn selftest_variants(relative: &str) -> std::collections::BTreeSet<String> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../.mintswarm/dogfood")
        .join(relative);
    let source = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
    let start = source
        .find("\ndef build_commits(")
        .unwrap_or_else(|| panic!("{relative}: no build_commits"));
    let end = source[start + 1..]
        .find("\ndef ")
        .map_or(source.len(), |end| start + 1 + end);
    let body = &source[start..end];

    let mut names = std::collections::BTreeSet::new();
    let mut from = 0;
    while let Some(found) = body[from..].find("variant(") {
        let at = from + found;
        from = at + "variant(".len();
        let before = body[..at].chars().next_back();
        if before.is_some_and(|c| c.is_alphanumeric() || c == '_') || body[..at].ends_with("def ") {
            continue;
        }
        let call = body[from..].trim_start();
        let loop_line = || {
            body[..at]
                .lines()
                .rev()
                .map(str::trim)
                .find(|line| line.starts_with("for name, "))
                .unwrap_or_else(|| panic!("{relative}: a variant call outside a name loop"))
        };
        if let Some(rest) = call.strip_prefix('"') {
            names.insert(rest[..rest.find('"').expect("a closed string")].to_owned());
        } else if call.starts_with("name,") {
            let line = loop_line();
            let constant = line
                .split(" in ")
                .nth(1)
                .and_then(|rest| rest.strip_suffix(".items():"))
                .unwrap_or_else(|| panic!("{relative}: an unknown loop: {line}"));
            names.extend(dict_keys(&source, constant, relative));
        } else if let Some(rest) = call.strip_prefix("f\"") {
            let pattern = &rest[..rest.find('"').expect("a closed string")];
            assert!(pattern.contains("{name}"), "{relative}: f\"{pattern}\"");
            let line = loop_line();
            assert!(
                line.ends_with(" in ("),
                "{relative}: an unknown loop: {line}"
            );
            let loop_at = body[..at].rfind(line).expect("the loop line");
            let items = body[loop_at + line.len()..at]
                .lines()
                .map(str::trim)
                .take_while(|line| !line.starts_with("):"))
                .filter_map(|line| line.strip_prefix("(\""))
                .map(|rest| rest[..rest.find('"').expect("a closed string")].to_owned());
            names.extend(items.map(|item| pattern.replace("{name}", &item)));
        } else {
            panic!(
                "{relative}: an unknown variant call: {}",
                call.lines().next().unwrap_or("")
            );
        }
    }
    assert!(!names.is_empty(), "{relative}: no variants found");
    names
}

/// The keys of the top-level dict constant `name` in a Python source.
fn dict_keys(source: &str, name: &str, relative: &str) -> Vec<String> {
    let start = source
        .find(&format!("\n{name} = {{"))
        .unwrap_or_else(|| panic!("{relative}: no {name}"));
    let text = &source[start + name.len() + 4..];
    let mut keys = Vec::new();
    let mut depth = 0;
    let mut chars = text.char_indices().peekable();
    while let Some((index, c)) = chars.next() {
        match c {
            '{' | '[' | '(' => depth += 1,
            '}' | ']' | ')' => {
                depth -= 1;
                if depth == 0 {
                    return keys;
                }
            }
            '"' => {
                let close = text[index + 1..].find('"').expect("a closed string") + index + 1;
                while chars.peek().is_some_and(|&(i, _)| i <= close) {
                    chars.next();
                }
                if depth == 1 && text[close + 1..].trim_start().starts_with(':') {
                    keys.push(text[index + 1..close].to_owned());
                }
            }
            _ => {}
        }
    }
    panic!("{relative}: {name} is not closed");
}

/// The Rust variant lists name exactly the variants the self-tests build, so a variant added to
/// either self-test, or to `MIN_CONFIDENCE_INVALID`, `SIZE_INVALID` or `REMOVED_LIMIT`, fails
/// here until it is added to a list above.
#[test]
fn variant_names_match_selftests() {
    let names = |lists: Vec<Vec<(&'static str, Change)>>| -> std::collections::BTreeSet<String> {
        lists
            .into_iter()
            .flatten()
            .map(|(name, _)| name.to_owned())
            .collect()
    };
    let plan = names(vec![plan_approval_rejected(), plan_approval_accepted()]);
    assert_eq!(plan, selftest_variants("plan-approval/selftest.py"));

    let landing = names(vec![landing_rejected(), landing_accepted()]);
    let mut built = selftest_variants("landing/floor-selftest.py");
    // `globs` varies the tree to exercise the floor's glob matcher, not the config's schema.
    assert!(built.remove("globs"), "landing: no globs variant");
    assert_eq!(landing, built);
}
