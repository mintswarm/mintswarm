//! `proto::otlp`: the golden journal lines, one per fact kind, the OTLP/JSON rules, the request
//! shape, the per-kind encoding, exact decoding and the refusals; and the journal line section
//! of `docs/architecture/metrics.md`.
//!
//! The inputs are the example facts of `crates/domain/tests/fixtures/fact-ids.json`. Line *i*
//! of `tests/fixtures/otlp/journal.jsonl` is the line of the first example of
//! `FactKind::ALL[i]`. With `MSW_OTLP_BLESS=1` the golden test writes the file instead of
//! comparing it.

use std::fs;
use std::path::{Path, PathBuf};

use mintswarm_domain::facts::catalogue::{OutcomeStatus, RunOutcome};
use mintswarm_domain::facts::{
    CATALOGUE_VERSION, Fact, FactKind, SCHEMA_URL, SCOPE_NAME, Signal, envelope, ids,
};
use mintswarm_proto::otlp::{self, Error, Origin, json};
use serde::Deserialize;
use serde_json::{Value, json};

fn manifest_dir() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn golden_path() -> PathBuf {
    manifest_dir().join("tests/fixtures/otlp/journal.jsonl")
}

fn origin() -> Origin {
    Origin {
        service_name: "mintswarm-session".into(),
        service_version: "0.1.0".into(),
        instance_id: "00000000-0000-4000-8000-000000000001".into(),
        host_name: "host.example".into(),
    }
}

/// Every example fact of the id fixture, in its order.
fn fixture_facts() -> Vec<Fact> {
    let path = manifest_dir().join("../domain/tests/fixtures/fact-ids.json");
    let fixture: Value =
        serde_json::from_str(&fs::read_to_string(path).expect("the id fixture reads"))
            .expect("the id fixture is JSON");
    fixture["facts"]
        .as_array()
        .expect("the fixture lists facts")
        .iter()
        .map(|entry| serde_json::from_value(entry["fact"].clone()).expect("a fixture fact"))
        .collect()
}

/// The first example of each kind, in `FactKind::ALL` order.
fn golden_facts() -> Vec<Fact> {
    let facts = fixture_facts();
    FactKind::ALL
        .iter()
        .map(|kind| {
            facts
                .iter()
                .find(|fact| fact.kind() == *kind)
                .cloned()
                .unwrap_or_else(|| panic!("no example of {kind:?}"))
        })
        .collect()
}

fn golden_lines() -> Vec<String> {
    fs::read_to_string(golden_path())
        .expect("the golden lines read")
        .lines()
        .map(str::to_owned)
        .collect()
}

fn line_of(fact: &Fact) -> String {
    otlp::encode_line(fact, &origin()).expect("a fixture fact encodes")
}

fn tree(line: &str) -> Value {
    serde_json::from_str(line).expect("a line is JSON")
}

/// The one record of a line's tree, and whether it is a span.
fn record(tree: &Value) -> (&Value, bool) {
    if let Some(spans) = tree.pointer("/resourceSpans/0/scopeSpans/0/spans/0") {
        (spans, true)
    } else {
        (
            tree.pointer("/resourceLogs/0/scopeLogs/0/logRecords/0")
                .expect("a line holds a span or a log record"),
            false,
        )
    }
}

fn attribute_keys(record: &Value) -> Vec<String> {
    record["attributes"]
        .as_array()
        .map(|list| {
            list.iter()
                .map(|kv| kv["key"].as_str().unwrap_or_default().to_owned())
                .collect()
        })
        .unwrap_or_default()
}

fn attribute<'a>(record: &'a Value, key: &str) -> Option<&'a Value> {
    record["attributes"]
        .as_array()?
        .iter()
        .find(|kv| kv["key"] == key)
        .map(|kv| &kv["value"])
}

#[test]
fn golden_lines_are_the_codec_output() {
    let lines: Vec<String> = golden_facts().iter().map(line_of).collect();
    if std::env::var_os("MSW_OTLP_BLESS").is_some_and(|v| v == "1") {
        fs::write(golden_path(), lines.join("\n") + "\n").unwrap();
    }
    let golden = golden_lines();
    assert_eq!(golden.len(), FactKind::ALL.len());
    for (i, (actual, expected)) in lines.iter().zip(&golden).enumerate() {
        assert_eq!(actual, expected, "line {} ({:?})", i + 1, FactKind::ALL[i]);
    }
}

#[test]
fn golden_lines_carry_every_envelope_key() {
    let text = golden_lines().join("\n");
    for key in envelope::KEYS {
        assert!(text.contains(&format!("\"key\":\"{key}\"")), "{key}");
    }
}

#[test]
fn every_golden_line_round_trips_byte_for_byte() {
    for line in golden_lines() {
        let export = json::from_json(&line).unwrap();
        assert_eq!(json::to_json(&export), line);
    }
}

/// Checks the OTLP/JSON rules at every level of `value`.
fn check_rules(value: &Value, key: &str) {
    let hex = |text: &str, width: usize| {
        text.len() == width && text.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
    };
    match value {
        Value::Object(object) => {
            for (child, inner) in object {
                assert!(!child.contains('_'), "key `{child}`");
                check_rules(inner, child);
            }
        }
        Value::Array(items) => items.iter().for_each(|item| check_rules(item, key)),
        Value::String(text) => match key {
            "traceId" => assert!(hex(text, 32), "{key}: {text}"),
            "spanId" | "parentSpanId" => assert!(hex(text, 16), "{key}: {text}"),
            _ => {}
        },
        Value::Number(_) => {
            assert!(
                matches!(key, "kind" | "code" | "doubleValue"),
                "`{key}` is a number"
            );
        }
        _ => {}
    }
    if key.ends_with("UnixNano") || key == "intValue" {
        assert!(value.is_string(), "`{key}` is not a string");
    }
    if key == "kind" || key == "code" {
        assert!(value.is_u64(), "`{key}` is not an integer");
    }
}

#[test]
fn lines_follow_the_otlp_json_rules() {
    for line in golden_lines() {
        let tree = tree(&line);
        check_rules(&tree, "");
        let (record, is_span) = record(&tree);
        assert!(record["traceId"].is_string());
        assert!(record["spanId"].is_string());
        if is_span {
            assert_eq!(record["kind"], json!(1));
            assert!(record["startTimeUnixNano"].is_string());
        } else {
            assert!(record["timeUnixNano"].is_string());
        }
    }
}

/// A fact's serde form, keeping the order of its attributes.
#[derive(Deserialize)]
struct Form {
    attributes: Ordered,
}

/// A JSON object's keys, in order.
struct Ordered(Vec<String>);

impl<'de> Deserialize<'de> for Ordered {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Keys;
        impl<'de> serde::de::Visitor<'de> for Keys {
            type Value = Ordered;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("an object")
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut map: A,
            ) -> Result<Ordered, A::Error> {
                let mut keys = Vec::new();
                while let Some((key, _)) = map.next_entry::<String, serde::de::IgnoredAny>()? {
                    keys.push(key);
                }
                Ok(Ordered(keys))
            }
        }
        deserializer.deserialize_map(Keys)
    }
}

/// The fact's attribute keys in the catalogue's field order.
fn kind_keys(fact: &Fact) -> Vec<String> {
    let text = serde_json::to_string(fact).expect("the test data is well formed");
    let form: Form = serde_json::from_str(&text).expect("the test data is well formed");
    form.attributes.0
}

const ITEM_KEYS: [&str; 5] = [
    "mintswarm.item.tracker",
    "mintswarm.item.repository",
    "mintswarm.item.number",
    "mintswarm.item.kind",
    "mintswarm.item.components",
];

#[test]
fn requests_have_one_resource_scope_and_record_in_order() {
    for fact in golden_facts() {
        let tree = tree(&line_of(&fact));
        let (signal, scopes, records) = match fact.kind().otlp().signal {
            Signal::Span => ("resourceSpans", "scopeSpans", "spans"),
            Signal::Log => ("resourceLogs", "scopeLogs", "logRecords"),
        };
        let resources = tree[signal].as_array().unwrap();
        assert_eq!(resources.len(), 1);
        let resource = &resources[0];
        assert_eq!(
            attribute_keys(&resource["resource"]),
            [
                "service.name",
                "service.version",
                "service.instance.id",
                "host.name"
            ]
        );
        let scope_list = resource[scopes].as_array().unwrap();
        assert_eq!(scope_list.len(), 1);
        let scope = &scope_list[0];
        assert_eq!(scope["scope"]["name"], json!(SCOPE_NAME));
        assert_eq!(
            scope["scope"]["version"],
            json!(CATALOGUE_VERSION.to_string())
        );
        assert_eq!(scope["schemaUrl"], json!(SCHEMA_URL));
        assert_eq!(scope[records].as_array().unwrap().len(), 1);

        let (record, is_span) = record(&tree);
        let keys = attribute_keys(record);
        assert_eq!(keys[..5], ITEM_KEYS);
        let mut at = 5;
        let envelope_keys: Vec<&str> = keys[at..]
            .iter()
            .map(String::as_str)
            .take_while(|k| envelope::KEYS.contains(k))
            .collect();
        let mut sorted = envelope_keys.clone();
        sorted.sort_by_key(|k| envelope::KEYS.iter().position(|e| e == k));
        assert_eq!(envelope_keys, sorted, "{:?}", fact.kind());
        at += envelope_keys.len();
        if !is_span {
            assert_eq!(keys[at], envelope::FACT_ID);
            at += 1;
        }
        assert_eq!(keys[at..], kind_keys(&fact)[..], "{:?}", fact.kind());
    }
}

/// The parent span id hex of a span fact's line, as the envelope derives it.
fn expected_parent(fact: &Fact) -> Option<String> {
    let value = serde_json::to_value(fact).expect("the test data is well formed");
    if fact.kind() == FactKind::ItemRoot {
        return None;
    }
    let key = fact.item().key();
    Some(match value.pointer("/concerns/parent") {
        Some(parent) => serde_json::from_value::<mintswarm_domain::facts::SpanRef>(parent.clone())
            .expect("the test data is well formed")
            .span_id(key)
            .to_string(),
        None => ids::root_span_id(key).to_string(),
    })
}

/// The `(trace id, span id)` hex a fact's links point to.
fn expected_links(fact: &Fact) -> Vec<(String, String)> {
    let value = serde_json::to_value(fact).expect("the test data is well formed");
    let Some(links) = value["links"].as_array() else {
        return Vec::new();
    };
    links
        .iter()
        .map(|link| {
            let target: mintswarm_domain::facts::LinkTarget =
                serde_json::from_value(link["target"].clone())
                    .expect("the test data is well formed");
            let span = target.concerns.as_ref().map_or_else(
                || ids::root_span_id(&target.item),
                |span| span.span_id(&target.item),
            );
            (ids::trace_id(&target.item).to_string(), span.to_string())
        })
        .collect()
}

fn expected_status(fact: &Fact) -> Value {
    match fact {
        Fact::StageRun(run) => match run.attributes.outcome {
            RunOutcome::Known(OutcomeStatus::Complete | OutcomeStatus::NeedsInput) => {
                json!({ "code": 1 })
            }
            RunOutcome::Known(OutcomeStatus::Failed) => json!({ "code": 2 }),
            RunOutcome::Unknown => Value::Null,
        },
        _ => Value::Null,
    }
}

/// Checks the record of `fact`'s line against design §4.3.
fn check_kind(fact: &Fact) {
    let line = line_of(fact);
    let tree = tree(&line);
    let (record, is_span) = record(&tree);
    let otlp = fact.kind().otlp();
    assert_eq!(record["traceId"], json!(fact.trace_id().to_string()));
    assert_eq!(record["spanId"], json!(fact.span_id().to_string()));
    match otlp.signal {
        Signal::Log => {
            assert!(!is_span);
            assert_eq!(record["eventName"], json!(otlp.name));
            let fact_id = fact
                .fact_id()
                .expect("the test data is well formed")
                .to_string();
            assert_eq!(
                attribute(record, envelope::FACT_ID),
                Some(&json!({ "stringValue": fact_id }))
            );
        }
        Signal::Span => {
            assert!(is_span);
            assert!(attribute(record, envelope::FACT_ID).is_none());
            match expected_parent(fact) {
                Some(parent) => assert_eq!(record["parentSpanId"], json!(parent)),
                None => assert!(record.get("parentSpanId").is_none()),
            }
            let links: Vec<(String, String)> = record["links"]
                .as_array()
                .map(|links| {
                    links
                        .iter()
                        .map(|l| {
                            (
                                l["traceId"]
                                    .as_str()
                                    .expect("the test data is well formed")
                                    .to_owned(),
                                l["spanId"]
                                    .as_str()
                                    .expect("the test data is well formed")
                                    .to_owned(),
                            )
                        })
                        .collect()
                })
                .unwrap_or_default();
            assert_eq!(links, expected_links(fact));
            assert_eq!(
                record.get("status").cloned().unwrap_or(Value::Null),
                expected_status(fact)
            );
        }
    }
    assert_eq!(
        otlp::decode_line(&line).expect("the test data is well formed"),
        *fact
    );
}

macro_rules! per_kind {
    ($($test:ident: $kind:ident;)*) => {
        $(
            #[test]
            fn $test() {
                let fact = golden_facts()
                    .into_iter()
                    .find(|fact| fact.kind() == FactKind::$kind)
                    .expect("the test data is well formed");
                check_kind(&fact);
            }
        )*

        #[test]
        fn per_kind_tests_cover_every_kind() {
            assert_eq!([$(FactKind::$kind),*].as_slice(), FactKind::ALL);
        }
    };
}

per_kind! {
    kind_item_root: ItemRoot;
    kind_stage_run_started: StageRunStarted;
    kind_stage_run: StageRun;
    kind_turn: Turn;
    kind_tool_call: ToolCall;
    kind_phase: Phase;
    kind_wait_started: WaitStarted;
    kind_wait: Wait;
    kind_confidence_scored: ConfidenceScored;
    kind_decision_requested: DecisionRequested;
    kind_decision_taken: DecisionTaken;
    kind_intervention: Intervention;
    kind_picked_up: PickedUp;
    kind_state_changed: StateChanged;
    kind_link_changed: LinkChanged;
    kind_pr_opened: PrOpened;
    kind_pr_merged: PrMerged;
    kind_pr_reverted: PrReverted;
    kind_regression_linked: RegressionLinked;
}

fn with_attribute(kind: FactKind, key: &str, value: Value) -> Fact {
    let fact = golden_facts()
        .into_iter()
        .find(|fact| fact.kind() == kind)
        .expect("the test data is well formed");
    let mut form = serde_json::to_value(&fact).expect("the test data is well formed");
    form["attributes"][key] = value;
    serde_json::from_value(form).expect("the test data is well formed")
}

#[test]
fn stage_run_status_follows_every_outcome() {
    let codes = [
        ("complete", Some(1)),
        ("needs_input", Some(1)),
        ("failed", Some(2)),
        ("unknown", None),
    ];
    assert_eq!(codes.len(), RunOutcome::ALL.len());
    for (outcome, code) in codes {
        let fact = with_attribute(
            FactKind::StageRun,
            "mintswarm.outcome.status",
            json!(outcome),
        );
        check_kind(&fact);
        let tree = tree(&line_of(&fact));
        let (record, _) = record(&tree);
        assert_eq!(record["status"]["code"].as_u64(), code, "{outcome}");
    }
}

#[test]
fn a_17_digit_double_round_trips() {
    let fact = with_attribute(
        FactKind::ConfidenceScored,
        "mintswarm.confidence.value",
        json!(0.300_000_000_000_000_04_f64),
    );
    let line = line_of(&fact);
    assert!(line.contains("\"doubleValue\":0.30000000000000004"));
    let back = otlp::decode_line(&line).unwrap();
    assert_eq!(back, fact);
    assert_eq!(line_of(&back), line);
}

#[test]
fn golden_lines_decode_to_their_facts() {
    for (line, fact) in golden_lines().iter().zip(golden_facts()) {
        assert_eq!(otlp::decode_line(line).unwrap(), fact);
    }
    let second = fixture_facts()
        .into_iter()
        .filter(|fact| fact.kind() == FactKind::Intervention)
        .nth(1)
        .unwrap();
    assert!(
        serde_json::to_value(&second)
            .unwrap()
            .get("concerns")
            .is_some()
    );
    assert_eq!(otlp::decode_line(&line_of(&second)).unwrap(), second);
}

fn line_for(kind: FactKind) -> String {
    let i = FactKind::ALL
        .iter()
        .position(|k| *k == kind)
        .expect("the test data is well formed");
    golden_lines()[i].clone()
}

fn edited(kind: FactKind, edit: impl FnOnce(&mut Value)) -> String {
    let mut value = tree(&line_for(kind));
    edit(&mut value);
    serde_json::to_string(&value).expect("the test data is well formed")
}

#[test]
fn changed_ids_are_refused() {
    let span = edited(FactKind::Turn, |v| {
        v["resourceSpans"][0]["scopeSpans"][0]["spans"][0]["spanId"] = json!("0000000000000001");
    });
    assert!(matches!(otlp::decode_line(&span), Err(Error::Ids(_))));
    let log = edited(FactKind::PrMerged, |v| {
        v["resourceLogs"][0]["scopeLogs"][0]["logRecords"][0]["spanId"] = json!("0000000000000001");
    });
    assert!(matches!(otlp::decode_line(&log), Err(Error::Ids(_))));
    let fact_id = edited(FactKind::PrMerged, |v| {
        let attributes = v["resourceLogs"][0]["scopeLogs"][0]["logRecords"][0]["attributes"]
            .as_array_mut()
            .unwrap();
        for kv in attributes {
            if kv["key"] == envelope::FACT_ID {
                kv["value"] = json!({ "stringValue": "00000000000000000000000000000001" });
            }
        }
    });
    assert!(matches!(otlp::decode_line(&fact_id), Err(Error::Ids(_))));
}

#[test]
fn other_counts_are_refused() {
    let paths = [
        "/resourceLogs",
        "/resourceLogs/0/scopeLogs",
        "/resourceLogs/0/scopeLogs/0/logRecords",
    ];
    for path in paths {
        for count in [0, 2] {
            let line = edited(FactKind::PrMerged, |v| {
                let list = v.pointer_mut(path).unwrap().as_array_mut().unwrap();
                let first = list[0].clone();
                list.clear();
                list.extend(std::iter::repeat_n(first, count));
            });
            assert!(
                matches!(otlp::decode_line(&line), Err(Error::Shape(_))),
                "{path} × {count}"
            );
        }
    }
    let spans = edited(FactKind::Turn, |v| {
        let list = v["resourceSpans"][0]["scopeSpans"][0]["spans"]
            .as_array_mut()
            .unwrap();
        let first = list[0].clone();
        list.push(first);
    });
    assert!(matches!(otlp::decode_line(&spans), Err(Error::Shape(_))));
}

#[test]
fn other_scopes_and_an_empty_status_are_refused() {
    let name = edited(FactKind::PrMerged, |v| {
        v["resourceLogs"][0]["scopeLogs"][0]["scope"]["name"] = json!("other");
    });
    assert!(matches!(otlp::decode_line(&name), Err(Error::Shape(_))));
    let version = edited(FactKind::PrMerged, |v| {
        v["resourceLogs"][0]["scopeLogs"][0]["scope"]["version"] = json!("2");
    });
    assert!(matches!(otlp::decode_line(&version), Err(Error::Shape(_))));
    let status = edited(FactKind::StageRun, |v| {
        v["resourceSpans"][0]["scopeSpans"][0]["spans"][0]["status"] = json!({});
    });
    assert!(matches!(otlp::decode_line(&status), Err(Error::Shape(_))));
}

#[test]
fn a_changed_record_is_refused() {
    let kind = edited(FactKind::Turn, |v| {
        v["resourceSpans"][0]["scopeSpans"][0]["spans"][0]["kind"] = json!(3);
    });
    assert!(matches!(otlp::decode_line(&kind), Err(Error::Differs)));
    assert!(matches!(otlp::decode_line("{}"), Err(Error::Json(_))));
    assert!(matches!(otlp::decode_line("not json"), Err(Error::Json(_))));
}

#[test]
fn an_integer_above_i64_max_is_refused_at_encode() {
    let fact = with_attribute(FactKind::PrMerged, "mintswarm.pr.number", json!(u64::MAX));
    assert!(matches!(
        otlp::encode_line(&fact, &origin()),
        Err(Error::Value(_))
    ));
}

/// The "Journal lines" section of `metrics.md`, from its heading to the next `## `.
fn journal_section() -> String {
    let path = manifest_dir().join("../../docs/architecture/metrics.md");
    let text = fs::read_to_string(path).expect("the test data is well formed");
    let start = text
        .find("\n## Journal lines\n")
        .expect("metrics.md has a Journal lines section");
    let rest = &text[start + 1..];
    let end = rest[3..].find("\n## ").map_or(rest.len(), |i| i + 4);
    rest[..end].to_owned()
}

/// The section's fenced blocks, and its text outside them.
fn fences(section: &str) -> (Vec<String>, String) {
    let mut blocks = Vec::new();
    let mut outside = String::new();
    let mut current: Option<Vec<&str>> = None;
    for line in section.lines() {
        if line.starts_with("```") {
            match current.take() {
                Some(block) => blocks.push(block.join("\n")),
                None => current = Some(Vec::new()),
            }
        } else if let Some(block) = current.as_mut() {
            block.push(line);
        } else {
            outside.push_str(line);
            outside.push('\n');
        }
    }
    (blocks, outside)
}

#[test]
fn the_journal_section_shows_one_golden_line() {
    let (blocks, _) = fences(&journal_section());
    assert_eq!(blocks.len(), 1);
    assert!(golden_lines().contains(&blocks[0]));
}

#[test]
fn the_journal_section_links_the_codec_and_restates_no_rule() {
    let (_, outside) = fences(&journal_section());
    assert!(outside.contains("](../../crates/proto/src/otlp.rs)"));
    let lower = outside.to_lowercase();
    for term in [
        "base64",
        "lowercamelcase",
        "stringif",
        "intvalue",
        "sorted",
        "hex",
    ] {
        assert!(!lower.contains(term), "{term}");
    }
}
