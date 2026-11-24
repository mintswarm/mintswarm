//! Probe results: their serde shape, namespaces and the three-valued version check.

#![cfg(test)]

use std::collections::BTreeMap;

use mintswarm_domain::probe::{
    Check, Fact, Namespace, ProbeKind, ProbeResult, ProbeValue, Version, VersionRequirement,
};
use serde_json::json;

fn known(version: &str) -> ProbeValue {
    ProbeValue::Known {
        version: Some(version.to_owned()),
        facts: BTreeMap::new(),
    }
}

fn round_trip(result: &ProbeResult, expected: serde_json::Value) {
    let written = serde_json::to_value(result).unwrap();
    assert_eq!(written, expected);
    let read: ProbeResult = serde_json::from_value(written).unwrap();
    assert_eq!(&read, result);
}

#[test]
fn tool_result_round_trips() {
    let result = ProbeResult {
        namespace: Namespace::new(ProbeKind::Tool, "git"),
        value: known("2.47.1"),
        observed_at: "2026-11-25T10:00:00Z".to_owned(),
    };
    round_trip(
        &result,
        json!({
            "namespace": "tool.git",
            "value": {"known": {"version": "2.47.1"}},
            "observed_at": "2026-11-25T10:00:00Z",
        }),
    );
}

#[test]
fn harness_result_round_trips_with_facts() {
    let facts = BTreeMap::from([
        ("logged_in".to_owned(), Fact::Bool(true)),
        (
            "credential_mode".to_owned(),
            Fact::Text("subscription".to_owned()),
        ),
        ("accounts".to_owned(), Fact::Integer(2)),
    ]);
    let result = ProbeResult {
        namespace: Namespace::new(ProbeKind::Harness, "claude"),
        value: ProbeValue::Known {
            version: Some("2.1.3".to_owned()),
            facts,
        },
        observed_at: "2026-11-25T10:00:00Z".to_owned(),
    };
    round_trip(
        &result,
        json!({
            "namespace": "harness.claude",
            "value": {"known": {
                "version": "2.1.3",
                "facts": {"logged_in": true, "credential_mode": "subscription", "accounts": 2},
            }},
            "observed_at": "2026-11-25T10:00:00Z",
        }),
    );
}

#[test]
fn unknown_is_a_value_not_an_error() {
    let result = ProbeResult {
        namespace: Namespace::new(ProbeKind::Toolchain, "rust"),
        value: ProbeValue::Unknown,
        observed_at: "2026-11-25T10:00:00Z".to_owned(),
    };
    round_trip(
        &result,
        json!({
            "namespace": "toolchain.rust",
            "value": "unknown",
            "observed_at": "2026-11-25T10:00:00Z",
        }),
    );
    let read: ProbeResult = serde_json::from_str(
        r#"{"namespace": "toolchain.rust", "value": "unknown", "observed_at": "t"}"#,
    )
    .expect("unknown reads as a value");
    assert_eq!(read.value, ProbeValue::Unknown);
    assert_ne!(read.value, known("1.87.0"));
}

#[test]
fn known_without_version_or_facts_reads() {
    let value: ProbeValue = serde_json::from_value(json!({"known": {}})).unwrap();
    assert_eq!(
        value,
        ProbeValue::Known {
            version: None,
            facts: BTreeMap::new(),
        }
    );
    assert_eq!(serde_json::to_value(&value).unwrap(), json!({"known": {}}));
}

#[test]
fn namespace_parsing() {
    let namespace: Namespace = "tool.git".parse().unwrap();
    assert_eq!(namespace, Namespace::new(ProbeKind::Tool, "git"));
    let namespace: Namespace = "toolchain.c.cpp".parse().unwrap();
    assert_eq!(namespace.id, "c.cpp");
    for kind in ProbeKind::ALL {
        let namespace: Namespace = format!("{kind}.x").parse().unwrap();
        assert_eq!(namespace.kind, kind);
    }
    for text in ["git", "bogus.git", "tool.", ".git", ""] {
        assert!(text.parse::<Namespace>().is_err(), "{text}");
    }
    let error = serde_json::from_value::<ProbeResult>(json!({
        "namespace": "bogus.git", "value": "unknown", "observed_at": "t",
    }))
    .expect_err("an unknown kind");
    assert!(error.to_string().contains("bogus.git"), "{error}");
}

#[test]
fn version_check_at_least_2_45() {
    let requirement = VersionRequirement::parse(">= 2.45").unwrap();
    assert_eq!(requirement.check(&known("2.45.0")), Check::Satisfied);
    assert_eq!(requirement.check(&known("2.47.1")), Check::Satisfied);
    assert_eq!(requirement.check(&known("2.44.2")), Check::Unsatisfied);
    assert_eq!(requirement.check(&ProbeValue::Unknown), Check::Unknown);
}

#[test]
fn version_check_without_a_version_is_unknown() {
    let requirement = VersionRequirement::parse(">=2.45").unwrap();
    let no_version = ProbeValue::Known {
        version: None,
        facts: BTreeMap::new(),
    };
    assert_eq!(requirement.check(&no_version), Check::Unknown);
    assert_eq!(requirement.check(&known("unreleased")), Check::Unknown);
}

#[test]
fn version_check_reads_the_numeric_prefix() {
    let requirement = VersionRequirement::parse(">= 2.45").unwrap();
    assert_eq!(
        requirement.check(&known("2.39.5 (Apple Git-154)")),
        Check::Unsatisfied
    );
    assert_eq!(requirement.check(&known("2.45")), Check::Satisfied);
    assert_eq!(requirement.check(&known("2.45.0-rc1")), Check::Satisfied);
    let rust = VersionRequirement::parse(">= 1.87").unwrap();
    assert_eq!(rust.check(&known("1.87.0-nightly")), Check::Satisfied);
}

#[test]
fn operators_and_ranges() {
    let version = |text: &str| Version::extract(text).unwrap();
    let cases = [
        ("> 2.45", "2.45.0", false),
        ("> 2.45", "2.45.1", true),
        ("<= 2.45", "2.45.0", true),
        ("<= 2.45", "2.45.1", false),
        ("< 3", "2.99", true),
        ("< 3", "3.0.0", false),
        ("= 2.45", "2.45.0", true),
        ("= 2.45", "2.45.1", false),
        (">= 2.45, < 3", "2.47.1", true),
        (">= 2.45, < 3", "3.1", false),
        (">=2.45,<3", "2.44", false),
    ];
    for (requirement, candidate, expected) in cases {
        let parsed = VersionRequirement::parse(requirement).unwrap();
        assert_eq!(
            parsed.matches(&version(candidate)),
            expected,
            "{requirement} over {candidate}"
        );
    }
    let parsed = VersionRequirement::parse(">=2.45,<3").unwrap();
    assert_eq!(parsed.to_string(), ">= 2.45, < 3");
}

#[test]
fn malformed_requirements_are_errors() {
    for text in [
        "", "  ", "2.45", "~> 2.45", ">=", ">= 2.45,", ">= 2.x", ">= 2.45.", ">= v2",
    ] {
        assert!(VersionRequirement::parse(text).is_err(), "{text:?}");
    }
    let error = VersionRequirement::parse(">= 2.x").unwrap_err();
    assert!(error.to_string().contains("2.x"), "{error}");
}

#[test]
fn versions_compare_with_missing_components_as_zero() {
    let version = |text: &str| Version::extract(text).unwrap();
    assert_eq!(version("2.45"), version("2.45.0"));
    assert!(version("2.45") < version("2.45.0.1"));
    assert!(version("2.9") < version("2.10"));
    assert_eq!(version("2.45").components(), [2, 45]);
}
