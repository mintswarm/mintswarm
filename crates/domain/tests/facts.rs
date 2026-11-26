//! The fact ids against the shared fixture, whose expected ids were computed independently of
//! this crate, and the fixture's example facts through the `Fact` serde form.

#![cfg(test)]

use std::collections::BTreeSet;
use std::path::PathBuf;

use mintswarm_domain::facts::ids::{self, ItemKey, SpanKind};
use mintswarm_domain::facts::{Fact, FactKind};
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Fixture {
    items: Vec<FixtureItem>,
    facts: Vec<FixtureFact>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FixtureItem {
    key: String,
    rendered: String,
    trace_id: String,
    root_span_id: String,
    spans: Vec<FixtureSpan>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FixtureSpan {
    kind: SpanKind,
    local: Vec<String>,
    span_id: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FixtureFact {
    fact: serde_json::Value,
    span_id: String,
    fact_id: Option<String>,
}

fn fixture() -> Fixture {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/fact-ids.json");
    let text = std::fs::read_to_string(&path).unwrap();
    serde_json::from_str(&text).unwrap()
}

#[test]
fn fixture_item_ids() {
    let fixture = fixture();
    assert!(fixture.items.len() >= 3);
    assert!(fixture.items.iter().any(|i| i.key != i.rendered));
    for item in &fixture.items {
        let key: ItemKey = item.key.parse().unwrap();
        assert_eq!(key.to_string(), item.rendered);
        assert_eq!(
            ids::trace_id(&key).to_string(),
            item.trace_id,
            "{}",
            item.key
        );
        assert_eq!(
            ids::root_span_id(&key).to_string(),
            item.root_span_id,
            "{}",
            item.key
        );
        let kinds: Vec<SpanKind> = item.spans.iter().map(|s| s.kind).collect();
        assert_eq!(kinds, SpanKind::ALL, "{}", item.key);
        for span in &item.spans {
            assert_eq!(
                ids::span_id(&key, span.kind, &span.local).to_string(),
                span.span_id,
                "{} {:?}",
                item.key,
                span.kind
            );
        }
    }
}

#[test]
fn fixture_fact_ids() {
    let fixture = fixture();
    let mut seen = BTreeSet::new();
    for example in &fixture.facts {
        let fact: Fact = serde_json::from_value(example.fact.clone()).unwrap();
        let tag = fact.kind().tag();
        assert_eq!(example.fact["kind"], tag);
        assert_eq!(fact.span_id().to_string(), example.span_id, "{tag}");
        assert_eq!(
            fact.fact_id().map(|id| id.to_string()),
            example.fact_id,
            "{tag}"
        );
        let key: ItemKey = fact.item().key().clone();
        assert_eq!(fact.trace_id(), ids::trace_id(&key));
        seen.insert(tag);
    }
    let all: BTreeSet<&str> = FactKind::ALL.iter().map(|k| k.tag()).collect();
    assert_eq!(seen, all, "one example per kind");
}

#[test]
fn fixture_facts_round_trip() {
    for example in fixture().facts {
        let fact: Fact = serde_json::from_value(example.fact.clone()).unwrap();
        let again: Fact = serde_json::from_value(serde_json::to_value(&fact).unwrap()).unwrap();
        assert_eq!(again, fact);
        assert_eq!(again.kind(), fact.kind());
    }
}
