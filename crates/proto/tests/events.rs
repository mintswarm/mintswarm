//! The `mintswarm.runner.v1` events: `RunnerEvent` has the design's fields
//! and ten body cases, the outcome and disposition enums have exactly their
//! values, and `RecordPut` never carries the record's bytes.
//!
//! The struct literals are exhaustive and the matches have no wildcard, so a
//! field or case that is added, dropped or renamed fails compilation.

#![cfg(test)]

mod common;

use common::full_stage_result;
use mintswarm_proto::json;
use mintswarm_proto::runner::v1::{
    ArtefactPut, AttemptOutcome, AttemptOutcomeKind, AttemptStartFailed, AttemptStartFailedReason,
    AttemptStarted, DirectiveDisposition, DirectiveReceipt, EventBatch, GateResult, Progress,
    Question, QuestionKind, QuestionOption, QuestionRecommendation, QuestionReference,
    QuestionReferenceKind, QuestionShape, RecordPut, RunnerEvent, StageResult, StageStatus, Usage,
    Warning, progress, runner_event, usage,
};
use prost::{Message, Name};

fn at(seconds: i64) -> Option<mintswarm_proto::prost_types::Timestamp> {
    Some(mintswarm_proto::prost_types::Timestamp { seconds, nanos: 0 })
}

fn record_put() -> RecordPut {
    RecordPut {
        kind: "floor".to_owned(),
        version: 1,
        sha256: "ab".repeat(32),
        size: 512,
    }
}

fn bodies() -> Vec<runner_event::Body> {
    vec![
        runner_event::Body::Started(AttemptStarted {}),
        runner_event::Body::StartFailed(AttemptStartFailed {
            reason: AttemptStartFailedReason::Duplicate.into(),
            detail: "already running".to_owned(),
        }),
        runner_event::Body::Outcome(AttemptOutcome {
            kind: AttemptOutcomeKind::Failed.into(),
            reason: "budget_exceeded".to_owned(),
            head: Some("0123456789abcdef0123456789abcdef01234567".to_owned()),
        }),
        runner_event::Body::Usage(Usage {
            tokens: Some(usage::Tokens {
                input: 1_000,
                output: 200,
                cache_read: 3_000,
                cache_write: 40,
            }),
            cost: Some(usage::Cost {
                amount: 0.25,
                currency: "USD".to_owned(),
                basis: "reported".to_owned(),
            }),
            limits: vec![usage::Limit {
                name: "five_hour".to_owned(),
                used_percent: 42.5,
                resets_at: at(1_800_003_600),
            }],
            source: "claude-code".to_owned(),
            observed_at: at(1_800_000_010),
        }),
        runner_event::Body::Progress(Progress {
            event: Some(progress::Event::GateFinished(progress::GateFinished {
                name: "rust-test".to_owned(),
                result: GateResult::Passed.into(),
            })),
        }),
        runner_event::Body::Warning(Warning {
            message: "disk nearly full".to_owned(),
        }),
        runner_event::Body::Record(record_put()),
        runner_event::Body::Artefact(ArtefactPut {
            name: "plan".to_owned(),
            revision: 2,
            sha256: "ef".repeat(32),
            size: 4_096,
        }),
        runner_event::Body::Receipt(DirectiveReceipt {
            directive_id: "d-1".to_owned(),
            disposition: DirectiveDisposition::AppliedAtBoundary.into(),
            detail: String::new(),
        }),
        runner_event::Body::StageResult(StageResult {
            status: StageStatus::NeedsInput.into(),
            artefact: "plan".to_owned(),
            outputs: vec![".mintswarm/job/outputs/plan.md".to_owned()],
            questions: vec![Question {
                id: "q1".to_owned(),
                kind: QuestionKind::Recommended.into(),
                question: "Inline the fence?".to_owned(),
                why: "It fixes the wire shape.".to_owned(),
                shape: QuestionShape::YesNo.into(),
                options: vec![
                    QuestionOption {
                        id: "yes".to_owned(),
                        label: "Yes".to_owned(),
                        consequence: "Matches the design.".to_owned(),
                        trade_off: None,
                    },
                    QuestionOption {
                        id: "no".to_owned(),
                        label: "No".to_owned(),
                        consequence: "Uses a shared message.".to_owned(),
                        trade_off: Some("Departs from the design.".to_owned()),
                    },
                ],
                recommendation: Some(QuestionRecommendation {
                    choice: vec!["yes".to_owned()],
                    answer: None,
                    reason: "The design lists the fields inline.".to_owned(),
                }),
                references: vec![QuestionReference {
                    kind: QuestionReferenceKind::Design.into(),
                    target: "8.1".to_owned(),
                    lines: Some("40-72".to_owned()),
                }],
            }],
            reason: String::new(),
            commit: Some("feat(api): runner events".to_owned()),
            pr_title: None,
        }),
    ]
}

/// The oneof name of a body case; a wildcard-free match over every variant.
fn body_name(body: &runner_event::Body) -> &'static str {
    match body {
        runner_event::Body::Started(_) => "started",
        runner_event::Body::StartFailed(_) => "start_failed",
        runner_event::Body::Outcome(_) => "outcome",
        runner_event::Body::Usage(_) => "usage",
        runner_event::Body::Progress(_) => "progress",
        runner_event::Body::Warning(_) => "warning",
        runner_event::Body::Record(_) => "record",
        runner_event::Body::Artefact(_) => "artefact",
        runner_event::Body::Receipt(_) => "receipt",
        runner_event::Body::StageResult(_) => "stage_result",
    }
}

#[test]
fn runner_event_fields_and_bodies() {
    let events: Vec<RunnerEvent> = bodies()
        .into_iter()
        .zip(1_u64..)
        .map(|(body, seq)| RunnerEvent {
            seq,
            at: at(1_800_000_000),
            job_id: "job-1".to_owned(),
            attempt_id: "attempt-1".to_owned(),
            lease_epoch: 3,
            body: Some(body),
        })
        .collect();
    let names: Vec<&str> = events
        .iter()
        .filter_map(|event| event.body.as_ref().map(body_name))
        .collect();
    assert_eq!(
        names,
        [
            "started",
            "start_failed",
            "outcome",
            "usage",
            "progress",
            "warning",
            "record",
            "artefact",
            "receipt",
            "stage_result",
        ]
    );

    let batch = EventBatch { events };
    let decoded = EventBatch::decode(batch.encode_to_vec().as_slice()).expect("decodes");
    assert_eq!(decoded, batch);
}

#[test]
fn attempt_outcome_kinds() {
    let all = [
        AttemptOutcomeKind::Unspecified,
        AttemptOutcomeKind::Succeeded,
        AttemptOutcomeKind::Failed,
        AttemptOutcomeKind::TimedOut,
        AttemptOutcomeKind::Stalled,
        AttemptOutcomeKind::Cancelled,
        AttemptOutcomeKind::Throttled,
        AttemptOutcomeKind::InvalidOutput,
        AttemptOutcomeKind::ModelUnavailable,
        AttemptOutcomeKind::Lost,
    ];
    for kind in all {
        // Wildcard-free: a value added to the enum fails compilation here.
        match kind {
            AttemptOutcomeKind::Unspecified
            | AttemptOutcomeKind::Succeeded
            | AttemptOutcomeKind::Failed
            | AttemptOutcomeKind::TimedOut
            | AttemptOutcomeKind::Stalled
            | AttemptOutcomeKind::Cancelled
            | AttemptOutcomeKind::Throttled
            | AttemptOutcomeKind::InvalidOutput
            | AttemptOutcomeKind::ModelUnavailable
            | AttemptOutcomeKind::Lost => {}
        }
    }
    let names: Vec<&str> = all.iter().map(|kind| kind.as_str_name()).collect();
    assert_eq!(
        names,
        [
            "ATTEMPT_OUTCOME_KIND_UNSPECIFIED",
            "ATTEMPT_OUTCOME_KIND_SUCCEEDED",
            "ATTEMPT_OUTCOME_KIND_FAILED",
            "ATTEMPT_OUTCOME_KIND_TIMED_OUT",
            "ATTEMPT_OUTCOME_KIND_STALLED",
            "ATTEMPT_OUTCOME_KIND_CANCELLED",
            "ATTEMPT_OUTCOME_KIND_THROTTLED",
            "ATTEMPT_OUTCOME_KIND_INVALID_OUTPUT",
            "ATTEMPT_OUTCOME_KIND_MODEL_UNAVAILABLE",
            "ATTEMPT_OUTCOME_KIND_LOST",
        ]
    );
    assert_eq!(
        AttemptOutcomeKind::from_str_name("ATTEMPT_OUTCOME_KIND_BUDGET_EXCEEDED"),
        None
    );
}

#[test]
fn attempt_start_failed_reasons() {
    let all = [
        AttemptStartFailedReason::Unspecified,
        AttemptStartFailedReason::Duplicate,
        AttemptStartFailedReason::Unsupported,
    ];
    for reason in all {
        // Wildcard-free: a value added to the enum fails compilation here.
        match reason {
            AttemptStartFailedReason::Unspecified
            | AttemptStartFailedReason::Duplicate
            | AttemptStartFailedReason::Unsupported => {}
        }
    }
    let pairs: Vec<(&str, i32)> = all
        .iter()
        .map(|reason| (reason.as_str_name(), *reason as i32))
        .collect();
    assert_eq!(
        pairs,
        [
            ("ATTEMPT_START_FAILED_REASON_UNSPECIFIED", 0),
            ("ATTEMPT_START_FAILED_REASON_DUPLICATE", 1),
            ("ATTEMPT_START_FAILED_REASON_UNSUPPORTED", 2),
        ]
    );
}

#[test]
fn directive_dispositions() {
    let all = [
        DirectiveDisposition::Unspecified,
        DirectiveDisposition::AppliedInSession,
        DirectiveDisposition::AppliedAtBoundary,
        DirectiveDisposition::Cancelled,
        DirectiveDisposition::NotApplicable,
        DirectiveDisposition::Unsupported,
    ];
    for disposition in all {
        // Wildcard-free: a value added to the enum fails compilation here.
        match disposition {
            DirectiveDisposition::Unspecified
            | DirectiveDisposition::AppliedInSession
            | DirectiveDisposition::AppliedAtBoundary
            | DirectiveDisposition::Cancelled
            | DirectiveDisposition::NotApplicable
            | DirectiveDisposition::Unsupported => {}
        }
    }
    let names: Vec<&str> = all.iter().map(|value| value.as_str_name()).collect();
    assert_eq!(
        names,
        [
            "DIRECTIVE_DISPOSITION_UNSPECIFIED",
            "DIRECTIVE_DISPOSITION_APPLIED_IN_SESSION",
            "DIRECTIVE_DISPOSITION_APPLIED_AT_BOUNDARY",
            "DIRECTIVE_DISPOSITION_CANCELLED",
            "DIRECTIVE_DISPOSITION_NOT_APPLICABLE",
            "DIRECTIVE_DISPOSITION_UNSUPPORTED",
        ]
    );
}

#[test]
fn record_put_fields() {
    // Each field gets a value of its expected type, so no field can be
    // `Vec<u8>` without breaking compilation.
    let kind: String = "decision".to_owned();
    let version: u32 = 2;
    let sha256: String = "01".repeat(32);
    let size: u64 = 1_024;
    let record = RecordPut {
        kind,
        version,
        sha256,
        size,
    };

    let RecordPut {
        kind,
        version,
        sha256,
        size,
    } = record;
    assert_eq!(kind, "decision");
    assert_eq!(version, 2);
    assert_eq!(sha256.len(), 64);
    assert_eq!(size, 1_024);
}

/// Writes `message` as proto3 JSON through `mintswarm_proto::json`.
fn to_json<M: Message + Name>(message: &M) -> serde_json::Value {
    json::serialize(message, serde_json::value::Serializer).expect("serialises")
}

/// Reads `M` from proto3 JSON through `mintswarm_proto::json`.
fn from_json<M: Message + Name + Default>(value: serde_json::Value) -> serde_json::Result<M> {
    json::deserialize(value)
}

#[test]
fn stage_result_json_uses_camel_case_and_enum_names() {
    let result = full_stage_result();
    let json = to_json(&result);

    assert_eq!(json["prTitle"], "feat(api): proto3 JSON for runner/v1");
    assert!(json.get("pr_title").is_none(), "{json}");
    assert_eq!(json["status"], "STAGE_STATUS_NEEDS_INPUT");
    let question = &json["questions"][0];
    assert_eq!(question["kind"], "QUESTION_KIND_ASSUMPTION");
    assert_eq!(question["options"][0]["tradeOff"], "A larger crate.");
    assert!(question["options"][0].get("trade_off").is_none(), "{json}");

    let parsed: StageResult = from_json(json).expect("parses");
    assert_eq!(parsed, result);
}

#[test]
fn stage_result_parses_proto_field_names() {
    let json = serde_json::json!({
        "status": "STAGE_STATUS_NEEDS_INPUT",
        "artefact": "handoff",
        "outputs": [".mintswarm/job/outputs/handoff.md"],
        "questions": [{
            "id": "q1",
            "kind": "QUESTION_KIND_ASSUMPTION",
            "question": "Which crate owns the parser?",
            "why": "It decides the reviewers.",
            "shape": "QUESTION_SHAPE_ONE",
            "options": [{
                "id": "a",
                "label": "proto",
                "consequence": "The wire types and the parser live together.",
                "trade_off": "A larger crate.",
            }],
            "recommendation": {
                "choice": ["a"],
                "answer": "proto",
                "reason": "It holds the protos.",
            },
            "references": [{
                "kind": "QUESTION_REFERENCE_KIND_FILE",
                "target": "crates/proto/src/lib.rs",
                "lines": "1-20",
            }],
        }],
        "reason": "Waiting on a decision.",
        "commit": "feat(api): proto3 JSON",
        "pr_title": "feat(api): proto3 JSON for runner/v1",
    });
    let parsed: StageResult = from_json(json).expect("parses");
    assert_eq!(parsed, full_stage_result());
}

#[test]
fn optional_fields_keep_presence() {
    type Field = fn(&mut StageResult) -> &mut Option<String>;
    let fields: [(&str, Field); 2] = [
        ("commit", |result| &mut result.commit),
        ("prTitle", |result| &mut result.pr_title),
    ];
    for (key, field) in fields {
        for value in [None, Some(String::new()), Some("x".to_owned())] {
            let mut result = full_stage_result();
            *field(&mut result) = value.clone();
            let json = to_json(&result);
            match &value {
                None => assert!(json.get(key).is_none(), "{key} is written: {json}"),
                Some(text) => assert_eq!(json[key], text.as_str(), "{key}: {json}"),
            }
            let mut parsed: StageResult = from_json(json).expect("parses");
            assert_eq!(*field(&mut parsed), value, "{key}");
            assert_eq!(parsed, result);
        }
    }
}

#[test]
fn unknown_keys_are_refused() {
    let json = to_json(&full_stage_result());

    let mut top = json.clone();
    top["extra"] = 1.into();
    assert!(from_json::<StageResult>(top).is_err());

    let mut nested = json;
    nested["questions"][0]["extra"] = 1.into();
    assert!(from_json::<StageResult>(nested).is_err());
}

#[test]
fn timestamp_is_rfc3339() {
    let event = RunnerEvent {
        seq: 1,
        at: at(0),
        job_id: "job-1".to_owned(),
        attempt_id: "attempt-1".to_owned(),
        lease_epoch: 1,
        body: Some(runner_event::Body::Started(AttemptStarted {})),
    };
    assert_eq!(to_json(&event)["at"], "1970-01-01T00:00:00Z");
}

#[test]
fn result_block_fixtures_round_trip() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/result-blocks");
    let mut read = 0;
    for entry in std::fs::read_dir(&dir).expect("fixtures directory") {
        let path = entry.expect("directory entry").path();
        let text = std::fs::read_to_string(&path).expect("fixture reads");
        let json: serde_json::Value = serde_json::from_str(&text).expect("fixture is JSON");
        let result: StageResult = from_json(json.clone())
            .unwrap_or_else(|error| panic!("{} parses: {error}", path.display()));
        assert_eq!(to_json(&result), json, "{}", path.display());
        read += 1;
    }
    assert!(read >= 3, "read {read} fixtures");
}
