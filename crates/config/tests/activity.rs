//! The activity rules' schema: one checker for a rules file and for `metrics.activity`.

#![cfg(test)]

use mintswarm_config::{ActivityMatch, ActivityRule, Error, load_activity_rules, load_repo_config};
use mintswarm_domain::facts::catalogue::Harness;

/// The sections every repository config needs.
const REST: &str = "
components:
  domain: { paths: [\"crates/**\"] }
pipeline:
  plan_approval:
    mode: always
    limits: { files: 10, lines: 500, new_files: 3 }
  landing:
    mode: always
    footprint: { lines_factor: 1.5 }
";

/// One rule of each kind, as a rules file's `rules` list, indented for either place.
fn rules(indent: &str) -> String {
    [
        "rules:",
        "  - { tool: Read, activity: reading }",
        "  - { tool: \"Web*\", harness: [claude], activity: fetching }",
        "  - { command: git, args: [commit], activity: committing }",
        "  - { command: grep, activity: searching }",
        "  - { mcp: { server: github, tool: \"*\" }, activity: fetching }",
    ]
    .iter()
    .map(|line| format!("{indent}{line}\n"))
    .collect()
}

/// A repository config with `metrics` set to `metrics`, indented under the key.
fn repo_config(metrics: &str) -> String {
    format!("metrics:\n{metrics}{REST}")
}

fn expected() -> Vec<ActivityRule> {
    vec![
        ActivityRule {
            matcher: ActivityMatch::Tool("Read".into()),
            harness: None,
            activity: "reading".into(),
        },
        ActivityRule {
            matcher: ActivityMatch::Tool("Web*".into()),
            harness: Some(vec![Harness::Claude]),
            activity: "fetching".into(),
        },
        ActivityRule {
            matcher: ActivityMatch::Command {
                program: "git".into(),
                args: vec!["commit".into()],
            },
            harness: None,
            activity: "committing".into(),
        },
        ActivityRule {
            matcher: ActivityMatch::Command {
                program: "grep".into(),
                args: vec![],
            },
            harness: None,
            activity: "searching".into(),
        },
        ActivityRule {
            matcher: ActivityMatch::Mcp {
                server: "github".into(),
                tool: "*".into(),
            },
            harness: None,
            activity: "fetching".into(),
        },
    ]
}

#[test]
fn one_rule_of_each_kind_parses() {
    assert_eq!(load_activity_rules(&rules("")).unwrap(), expected());
}

#[test]
fn both_sources_share_one_schema() {
    let file = load_activity_rules(&rules("")).unwrap();
    let config =
        load_repo_config(&repo_config(&format!("  activity:\n{}", rules("    ")))).unwrap();
    assert_eq!(config.activity_rules(), &file[..]);
}

#[test]
fn absent_metrics_has_no_rules() {
    let config = load_repo_config(REST).unwrap();
    assert!(config.activity_rules().is_empty());
    let config = load_repo_config(&repo_config("  activity: {}\n")).unwrap();
    assert!(config.activity_rules().is_empty());
}

/// The error a rules file holding the one rule `rule` gives.
fn rule_error(rule: &str) -> Error {
    load_activity_rules(&format!("rules:\n  - {rule}\n")).expect_err(rule)
}

fn assert_schema(error: &Error, path: &str) {
    assert!(matches!(error, Error::Schema { .. }), "{error:?}");
    assert_eq!(error.path(), path, "{error}");
}

#[test]
fn a_rule_without_a_matcher_is_an_error() {
    assert_schema(&rule_error("{ activity: reading }"), "rules[0]");
}

#[test]
fn a_rule_with_two_matchers_is_an_error() {
    assert_schema(
        &rule_error("{ tool: Read, command: cat, activity: reading }"),
        "rules[0]",
    );
    assert_schema(
        &rule_error("{ tool: Read, mcp: { server: a, tool: b }, activity: reading }"),
        "rules[0]",
    );
}

#[test]
fn args_without_command_is_an_error() {
    assert_schema(
        &rule_error("{ tool: Read, args: [x], activity: reading }"),
        "rules[0].args",
    );
    assert_schema(
        &rule_error("{ mcp: { server: a, tool: b }, args: [x], activity: reading }"),
        "rules[0].args",
    );
}

#[test]
fn an_activity_outside_the_pattern_is_an_error() {
    for activity in ["Reading", "1x", "\"\""] {
        assert_schema(
            &rule_error(&format!("{{ tool: Read, activity: {activity} }}")),
            "rules[0].activity",
        );
    }
    assert_schema(&rule_error("{ tool: Read }"), "rules[0].activity");
}

#[test]
fn unknown_keys_are_errors_at_every_level() {
    assert_schema(
        &rule_error("{ tool: Read, activity: reading, label: x }"),
        "rules[0].label",
    );
    assert_schema(
        &rule_error("{ mcp: { server: a, tool: b, kind: c }, activity: fetching }"),
        "rules[0].mcp.kind",
    );
    assert_schema(
        &load_activity_rules("rules: []\nextra: 1\n").unwrap_err(),
        "extra",
    );
    let error = load_repo_config(&repo_config("  spans: {}\n")).unwrap_err();
    assert_schema(&error, "metrics.spans");
    let error =
        load_repo_config(&repo_config("  activity:\n    rules: []\n    more: 1\n")).unwrap_err();
    assert_schema(&error, "metrics.activity.more");
}

#[test]
fn mcp_needs_server_and_tool() {
    assert_schema(
        &rule_error("{ mcp: { server: a }, activity: fetching }"),
        "rules[0].mcp.tool",
    );
}

#[test]
fn the_harness_filter_names_known_harnesses() {
    assert_schema(
        &rule_error("{ tool: Read, harness: [claude, cursor], activity: reading }"),
        "rules[0].harness[1]",
    );
    assert_schema(
        &rule_error("{ tool: Read, harness: [], activity: reading }"),
        "rules[0].harness",
    );
    let rules = load_activity_rules(
        "rules:\n  - { tool: shell, harness: [codex, claude], activity: running }\n",
    )
    .unwrap();
    assert_eq!(
        rules[0].harness,
        Some(vec![Harness::Codex, Harness::Claude])
    );
}

#[test]
fn errors_in_the_repository_config_are_located_under_metrics() {
    let error = load_repo_config(&repo_config(
        "  activity:\n    rules:\n      - { command: git, activity: Committing }\n",
    ))
    .unwrap_err();
    assert_schema(&error, "metrics.activity.rules[0].activity");
}
