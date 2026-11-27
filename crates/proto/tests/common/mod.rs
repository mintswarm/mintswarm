//! Helpers shared by the integration tests: a fully populated `StageResult`.

use mintswarm_proto::runner::v1::{
    Question, QuestionKind, QuestionOption, QuestionRecommendation, QuestionReference,
    QuestionReferenceKind, QuestionShape, StageResult, StageStatus,
};

/// A `StageResult` with every field set, including each proto3 `optional`.
pub fn full_stage_result() -> StageResult {
    StageResult {
        status: StageStatus::NeedsInput.into(),
        artefact: "handoff".to_owned(),
        outputs: vec![".mintswarm/job/outputs/handoff.md".to_owned()],
        questions: vec![Question {
            id: "q1".to_owned(),
            kind: QuestionKind::Assumption.into(),
            question: "Which crate owns the parser?".to_owned(),
            why: "It decides the reviewers.".to_owned(),
            shape: QuestionShape::One.into(),
            options: vec![QuestionOption {
                id: "a".to_owned(),
                label: "proto".to_owned(),
                consequence: "The wire types and the parser live together.".to_owned(),
                trade_off: Some("A larger crate.".to_owned()),
            }],
            recommendation: Some(QuestionRecommendation {
                choice: vec!["a".to_owned()],
                answer: Some("proto".to_owned()),
                reason: "It holds the protos.".to_owned(),
            }),
            references: vec![QuestionReference {
                kind: QuestionReferenceKind::File.into(),
                target: "crates/proto/src/lib.rs".to_owned(),
                lines: Some("1-20".to_owned()),
            }],
        }],
        reason: "Waiting on a decision.".to_owned(),
        commit: Some("feat(api): proto3 JSON".to_owned()),
        pr_title: Some("feat(api): proto3 JSON for runner/v1".to_owned()),
    }
}
