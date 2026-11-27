//! The stage contract over the generated `StageResult`: each question check,
//! field rule and the status rule of `.mintswarm/stages.md` gives its typed
//! error, with the wording of `mintswarm_domain::question`, and a valid result
//! is accepted.

#![cfg(test)]

use mintswarm_proto::check::{QuestionViolation, Receive, ReceiveError, Rule};
use mintswarm_proto::json;
use mintswarm_proto::runner::v1::{
    Question, QuestionKind, QuestionOption, QuestionRecommendation, QuestionReference,
    QuestionReferenceKind, QuestionShape, StageResult, StageStatus,
};

const MESSAGE: &str = "mintswarm.runner.v1.StageResult";

fn option(id: &str) -> QuestionOption {
    QuestionOption {
        id: id.to_owned(),
        label: id.to_owned(),
        consequence: "it follows".to_owned(),
        trade_off: None,
    }
}

fn recommend(choice: &[&str]) -> Option<QuestionRecommendation> {
    Some(QuestionRecommendation {
        choice: choice.iter().map(|&c| c.to_owned()).collect(),
        answer: None,
        reason: "it fits".to_owned(),
    })
}

/// A valid `assumption` of shape `one`, choosing `a` of `a` and `b`.
fn assumption(id: &str) -> Question {
    Question {
        id: id.to_owned(),
        kind: QuestionKind::Assumption.into(),
        question: "Which?".to_owned(),
        why: "It matters.".to_owned(),
        shape: QuestionShape::One.into(),
        options: vec![option("a"), option("b")],
        recommendation: recommend(&["a"]),
        references: Vec::new(),
    }
}

/// A valid `must_decide` of shape `one`, with no recommendation.
fn must_decide(id: &str) -> Question {
    Question {
        kind: QuestionKind::MustDecide.into(),
        recommendation: None,
        ..assumption(id)
    }
}

fn result(status: StageStatus, questions: Vec<Question>) -> StageResult {
    StageResult {
        status: status.into(),
        artefact: "plan".to_owned(),
        outputs: vec![".mintswarm/job/outputs/plan.md".to_owned()],
        questions,
        reason: String::new(),
        commit: None,
        pr_title: None,
    }
}

fn complete(question: Question) -> StageResult {
    result(StageStatus::Complete, vec![question])
}

fn error(field: &str, rule: Rule) -> ReceiveError {
    ReceiveError {
        message: MESSAGE.to_owned(),
        field: field.to_owned(),
        rule,
    }
}

/// Asserts that `result` gives exactly `violation` on `field`, with `text`.
fn breaks(result: StageResult, field: &str, violation: QuestionViolation, text: &str) {
    assert_eq!(violation.to_string(), text);
    assert_eq!(
        result.receive().expect_err("the result is refused"),
        vec![error(field, Rule::Question(violation))]
    );
}

fn id() -> String {
    "q1".to_owned()
}

#[test]
fn a_valid_result_is_accepted_unchanged() {
    let mut open = must_decide("q2");
    open.shape = QuestionShape::Open.into();
    open.options = Vec::new();
    open.recommendation = Some(QuestionRecommendation {
        choice: Vec::new(),
        answer: Some("result_block".to_owned()),
        reason: "it reads well".to_owned(),
    });
    open.references = vec![QuestionReference {
        kind: QuestionReferenceKind::File.into(),
        target: "crates/proto/src/check".to_owned(),
        lines: None,
    }];
    let valid = result(StageStatus::NeedsInput, vec![assumption("q1"), open]);
    assert_eq!(valid.clone().receive(), Ok(valid));
    let empty = result(StageStatus::Complete, Vec::new());
    assert_eq!(empty.clone().receive(), Ok(empty));
}

#[test]
fn the_result_block_fixtures_are_accepted() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/result-blocks");
    let mut read = 0;
    for entry in std::fs::read_dir(&dir).expect("fixtures directory") {
        let path = entry.expect("directory entry").path();
        let text = std::fs::read_to_string(&path).expect("fixture reads");
        let value: serde_json::Value = serde_json::from_str(&text).expect("fixture is JSON");
        let result: StageResult = json::deserialize(value).expect("fixture parses");
        if let Err(errors) = result.receive() {
            panic!("{} is accepted: {errors:?}", path.display());
        }
        read += 1;
    }
    assert!(read >= 3, "read {read} fixtures");
}

#[test]
fn the_default_result_is_refused() {
    assert_eq!(
        StageResult::default().receive(),
        Err(vec![error("status", Rule::Unspecified)])
    );
}

// The field rules.

#[test]
fn an_unknown_status_is_an_unknown_value_and_skips_the_status_rule() {
    let mut r = result(StageStatus::Complete, vec![must_decide("q1")]);
    r.status = 9;
    assert_eq!(
        r.receive(),
        Err(vec![error("status", Rule::UnknownValue { raw: 9 })])
    );
}

#[test]
fn an_empty_question_id_is_an_empty_id() {
    let mut q = assumption("");
    q.id = String::new();
    assert_eq!(
        complete(q).receive(),
        Err(vec![error("questions[0].id", Rule::EmptyId)])
    );
}

#[test]
fn an_unspecified_kind_or_shape_skips_the_question_rules() {
    let mut q = assumption("q1");
    q.kind = QuestionKind::Unspecified.into();
    q.question = String::new();
    assert_eq!(
        complete(q).receive(),
        Err(vec![error("questions[0].kind", Rule::Unspecified)])
    );
    let mut q = assumption("q1");
    q.shape = 42;
    q.why = String::new();
    assert_eq!(
        complete(q).receive(),
        Err(vec![error(
            "questions[0].shape",
            Rule::UnknownValue { raw: 42 }
        )])
    );
}

#[test]
fn an_unknown_kind_is_non_blocking() {
    let mut q = assumption("q1");
    q.kind = 7;
    assert_eq!(
        result(StageStatus::NeedsInput, vec![q]).receive(),
        Err(vec![
            error("questions[0].kind", Rule::UnknownValue { raw: 7 }),
            error(
                "status",
                Rule::Question(QuestionViolation::NeedsInputWithoutBlocking)
            ),
        ])
    );
}

#[test]
fn an_empty_option_id_is_an_empty_id() {
    let mut q = assumption("q1");
    q.options[1].id = String::new();
    assert_eq!(
        complete(q).receive(),
        Err(vec![error("questions[0].options[1].id", Rule::EmptyId)])
    );
}

#[test]
fn an_unspecified_reference_kind_is_unspecified() {
    let mut q = assumption("q1");
    q.references = vec![QuestionReference {
        kind: QuestionReferenceKind::Unspecified.into(),
        target: "x".to_owned(),
        lines: None,
    }];
    assert_eq!(
        complete(q).receive(),
        Err(vec![error(
            "questions[0].references[0].kind",
            Rule::Unspecified
        )])
    );
}

// The question checks, in `Question::violations` order.

#[test]
fn missing_recommendation() {
    let mut q = must_decide("q1");
    q.kind = QuestionKind::Recommended.into();
    breaks(
        result(StageStatus::NeedsInput, vec![q]),
        "questions[0].recommendation",
        QuestionViolation::MissingRecommendation { id: id() },
        "q1: a recommended or assumption question needs a recommendation",
    );
}

#[test]
fn assumption_open() {
    let mut q = assumption("q1");
    q.shape = QuestionShape::Open.into();
    q.options = Vec::new();
    q.recommendation = recommend(&[]);
    breaks(
        complete(q),
        "questions[0].shape",
        QuestionViolation::AssumptionOpen { id: id() },
        "q1: an assumption cannot have shape open",
    );
}

#[test]
fn assumption_no_alternative() {
    let mut q = assumption("q1");
    q.shape = QuestionShape::Several.into();
    q.recommendation = recommend(&["a", "b"]);
    breaks(
        complete(q),
        "questions[0].options",
        QuestionViolation::AssumptionNoAlternative { id: id() },
        "q1: an assumption needs an option besides those it chooses",
    );
}

#[test]
fn empty_question() {
    let mut q = assumption("q1");
    q.question = "  ".to_owned();
    breaks(
        complete(q),
        "questions[0].question",
        QuestionViolation::EmptyQuestion { id: id() },
        "q1: question is empty",
    );
}

#[test]
fn empty_why() {
    let mut q = assumption("q1");
    q.why = String::new();
    breaks(
        complete(q),
        "questions[0].why",
        QuestionViolation::EmptyWhy { id: id() },
        "q1: why is empty",
    );
}

#[test]
fn too_few_options() {
    let mut q = must_decide("q1");
    q.options = vec![option("a")];
    breaks(
        result(StageStatus::NeedsInput, vec![q]),
        "questions[0].options",
        QuestionViolation::TooFewOptions { id: id() },
        "q1: shapes one and several need at least two options",
    );
}

#[test]
fn yes_no_options() {
    let mut q = must_decide("q1");
    q.shape = QuestionShape::YesNo.into();
    breaks(
        result(StageStatus::NeedsInput, vec![q]),
        "questions[0].options",
        QuestionViolation::YesNoOptions { id: id() },
        "q1: yes_no options must be exactly yes and no",
    );
}

#[test]
fn open_with_options() {
    let mut q = must_decide("q1");
    q.shape = QuestionShape::Open.into();
    breaks(
        result(StageStatus::NeedsInput, vec![q]),
        "questions[0].options",
        QuestionViolation::OpenWithOptions { id: id() },
        "q1: an open question has no options",
    );
}

#[test]
fn duplicate_option_id() {
    let mut q = must_decide("q1");
    q.options = vec![option("a"), option("b"), option("a")];
    breaks(
        result(StageStatus::NeedsInput, vec![q]),
        "questions[0].options",
        QuestionViolation::DuplicateOptionId {
            id: id(),
            option: "a".to_owned(),
        },
        "q1: option id \"a\" is not unique",
    );
}

#[test]
fn choice_count() {
    let mut q = must_decide("q1");
    q.recommendation = recommend(&[]);
    breaks(
        result(StageStatus::NeedsInput, vec![q]),
        "questions[0].recommendation.choice",
        QuestionViolation::ChoiceCount {
            id: id(),
            shape: QuestionShape::One,
            count: 0,
        },
        "q1: 0 choices do not fit shape one",
    );
}

#[test]
fn unknown_choice() {
    let mut q = assumption("q1");
    q.options.push(option("c"));
    q.recommendation = recommend(&["z"]);
    breaks(
        complete(q),
        "questions[0].recommendation.choice",
        QuestionViolation::UnknownChoice {
            id: id(),
            choice: "z".to_owned(),
        },
        "q1: choice \"z\" is not an option id",
    );
}

#[test]
fn duplicate_choice() {
    let mut q = assumption("q1");
    q.shape = QuestionShape::Several.into();
    q.options.push(option("c"));
    q.recommendation = recommend(&["a", "a"]);
    breaks(
        complete(q),
        "questions[0].recommendation.choice",
        QuestionViolation::DuplicateChoice {
            id: id(),
            choice: "a".to_owned(),
        },
        "q1: choice \"a\" is chosen more than once",
    );
}

#[test]
fn answer_not_open() {
    let mut q = assumption("q1");
    if let Some(r) = q.recommendation.as_mut() {
        r.answer = Some("a".to_owned());
    }
    breaks(
        complete(q),
        "questions[0].recommendation.answer",
        QuestionViolation::AnswerNotOpen { id: id() },
        "q1: only an open question's recommendation has an answer",
    );
}

#[test]
fn duplicate_id() {
    let r = result(
        StageStatus::Complete,
        vec![assumption("q1"), assumption("q1")],
    );
    breaks(
        r,
        "questions",
        QuestionViolation::DuplicateId { id: id() },
        "q1: the id is not unique",
    );
}

// The status rule.

#[test]
fn needs_input_without_blocking() {
    breaks(
        result(StageStatus::NeedsInput, vec![assumption("q1")]),
        "status",
        QuestionViolation::NeedsInputWithoutBlocking,
        "needs_input needs a must_decide or recommended question",
    );
}

#[test]
fn blocking_not_needs_input() {
    breaks(
        result(StageStatus::Complete, vec![must_decide("q1")]),
        "status",
        QuestionViolation::BlockingNotNeedsInput {
            id: id(),
            status: StageStatus::Complete,
        },
        "q1: a complete outcome may hold assumptions only",
    );
    breaks(
        result(StageStatus::Failed, vec![must_decide("q1")]),
        "status",
        QuestionViolation::BlockingNotNeedsInput {
            id: id(),
            status: StageStatus::Failed,
        },
        "q1: a failed outcome may hold assumptions only",
    );
}

#[test]
fn every_violation_is_returned_in_the_domain_order() {
    let mut first = must_decide("q1");
    first.question = String::new();
    first.options = vec![option("a"), option("a")];
    first.recommendation = recommend(&["a", "a"]);
    let mut second = assumption("q2");
    second.shape = QuestionShape::Open.into();
    second.options = Vec::new();
    second.recommendation = None;
    let third = must_decide("q1");
    let errors = result(StageStatus::Complete, vec![first, second, third])
        .receive()
        .expect_err("the result is refused");
    let got: Vec<(String, String)> = errors
        .iter()
        .map(|e| (e.field.clone(), e.rule.to_string()))
        .collect();
    let want = [
        ("questions[0].question", "q1: question is empty"),
        ("questions[0].options", "q1: option id \"a\" is not unique"),
        (
            "questions[0].recommendation.choice",
            "q1: 2 choices do not fit shape one",
        ),
        (
            "questions[0].recommendation.choice",
            "q1: choice \"a\" is chosen more than once",
        ),
        (
            "questions[1].recommendation",
            "q2: a recommended or assumption question needs a recommendation",
        ),
        (
            "questions[1].shape",
            "q2: an assumption cannot have shape open",
        ),
        ("questions", "q1: the id is not unique"),
        ("status", "q1: a complete outcome may hold assumptions only"),
        ("status", "q1: a complete outcome may hold assumptions only"),
    ];
    assert_eq!(
        got,
        want.iter()
            .map(|&(f, r)| (f.to_owned(), r.to_owned()))
            .collect::<Vec<_>>()
    );
    assert!(errors.iter().all(|e| e.message == MESSAGE));
}
