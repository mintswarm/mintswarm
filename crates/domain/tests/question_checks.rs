//! `StageOutcome::check`: one test per violation, the positive cases, and all violations of a
//! case returned together.

use mintswarm_domain::work::{OutcomeStatus, StageOutcome};
use mintswarm_domain::{
    Question, QuestionKind, QuestionOption, QuestionShape, QuestionViolation, Recommendation,
};

use QuestionKind::{Assumption, MustDecide, Recommended};
use QuestionShape::{One, Open, Several, YesNo};

fn option(id: &str) -> QuestionOption {
    QuestionOption {
        id: id.into(),
        label: format!("option {id}"),
        consequence: format!("choosing {id}"),
        trade_off: None,
    }
}

fn options(ids: &[&str]) -> Vec<QuestionOption> {
    ids.iter().map(|id| option(id)).collect()
}

fn recommend(choice: &[&str]) -> Option<Recommendation> {
    Some(Recommendation {
        choice: choice.iter().map(|c| (*c).into()).collect(),
        answer: None,
        reason: "It fits.".into(),
    })
}

/// A valid question of the kind and shape: two options for `one` and `several`, and a fitting
/// recommendation for `recommended` and `assumption`.
fn question(id: &str, kind: QuestionKind, shape: QuestionShape) -> Question {
    let options = match shape {
        One | Several => options(&["a", "b"]),
        YesNo | Open => vec![],
    };
    let recommendation = match (kind, shape) {
        (MustDecide, _) => None,
        (_, One | Several) => recommend(&["a"]),
        (_, YesNo) => recommend(&["yes"]),
        (_, Open) => Some(Recommendation {
            choice: vec![],
            answer: Some("Call it parse.".into()),
            reason: "It fits.".into(),
        }),
    };
    Question {
        id: id.into(),
        kind,
        question: "Which one?".into(),
        why: "It decides the plan.".into(),
        shape,
        options,
        recommendation,
        references: vec![],
    }
}

fn outcome(status: OutcomeStatus, questions: Vec<Question>) -> StageOutcome {
    StageOutcome {
        status,
        artefact: "analysis".into(),
        outputs: vec![".mintswarm/job/outputs/analysis.md".into()],
        questions,
        reason: String::new(),
        commit: None,
        pr_title: None,
    }
}

/// The violations of an outcome holding the one question, `complete` for an assumption and
/// `needs_input` otherwise.
fn violations_of(question: Question) -> Vec<QuestionViolation> {
    let status = if question.kind == Assumption {
        OutcomeStatus::Complete
    } else {
        OutcomeStatus::NeedsInput
    };
    outcome(status, vec![question])
        .check()
        .expect_err("the question breaks a rule")
}

fn id(id: &str) -> String {
    id.into()
}

#[test]
fn recommended_without_recommendation() {
    let mut q = question("q1", Recommended, One);
    q.recommendation = None;
    assert_eq!(
        violations_of(q),
        vec![QuestionViolation::MissingRecommendation { id: id("q1") }]
    );
}

#[test]
fn assumption_without_recommendation() {
    let mut q = question("q1", Assumption, One);
    q.recommendation = None;
    assert_eq!(
        violations_of(q),
        vec![QuestionViolation::MissingRecommendation { id: id("q1") }]
    );
}

#[test]
fn assumption_of_shape_open() {
    let q = question("q1", Assumption, Open);
    assert_eq!(
        violations_of(q),
        vec![QuestionViolation::AssumptionOpen { id: id("q1") }]
    );
}

#[test]
fn assumption_choosing_every_option() {
    let mut q = question("q1", Assumption, Several);
    q.recommendation = recommend(&["a", "b"]);
    assert_eq!(
        violations_of(q),
        vec![QuestionViolation::AssumptionNoAlternative { id: id("q1") }]
    );
}

#[test]
fn assumption_yes_no_choosing_both() {
    let mut q = question("q1", Assumption, YesNo);
    q.recommendation = recommend(&["yes", "no"]);
    assert_eq!(
        violations_of(q),
        vec![
            QuestionViolation::AssumptionNoAlternative { id: id("q1") },
            QuestionViolation::ChoiceCount {
                id: id("q1"),
                shape: YesNo,
                count: 2
            },
        ]
    );
}

#[test]
fn empty_question_and_why() {
    let mut q = question("q1", MustDecide, Open);
    q.question = String::new();
    q.why = "  ".into();
    assert_eq!(
        violations_of(q),
        vec![
            QuestionViolation::EmptyQuestion { id: id("q1") },
            QuestionViolation::EmptyWhy { id: id("q1") },
        ]
    );
}

#[test]
fn duplicate_ids() {
    let result = outcome(
        OutcomeStatus::NeedsInput,
        vec![
            question("q1", MustDecide, Open),
            question("q2", Recommended, One),
            question("q1", Assumption, YesNo),
            question("q1", Assumption, One),
            question("q2", MustDecide, Open),
        ],
    )
    .check();
    assert_eq!(
        result,
        Err(vec![
            QuestionViolation::DuplicateId { id: id("q1") },
            QuestionViolation::DuplicateId { id: id("q2") },
        ])
    );
}

#[test]
fn unknown_choice_for_one() {
    let mut q = question("q1", Recommended, One);
    q.recommendation = recommend(&["c"]);
    assert_eq!(
        violations_of(q),
        vec![QuestionViolation::UnknownChoice {
            id: id("q1"),
            choice: id("c")
        }]
    );
}

#[test]
fn unknown_choice_for_yes_no_without_options() {
    let mut q = question("q1", Recommended, YesNo);
    q.recommendation = recommend(&["maybe"]);
    assert_eq!(
        violations_of(q),
        vec![QuestionViolation::UnknownChoice {
            id: id("q1"),
            choice: id("maybe")
        }]
    );
}

#[test]
fn duplicate_option_ids() {
    let mut q = question("q1", MustDecide, One);
    q.options = options(&["a", "b", "a", "b", "a", "c"]);
    assert_eq!(
        violations_of(q),
        vec![
            QuestionViolation::DuplicateOptionId {
                id: id("q1"),
                option: id("a")
            },
            QuestionViolation::DuplicateOptionId {
                id: id("q1"),
                option: id("b")
            },
        ]
    );
}

#[test]
fn duplicate_option_ids_on_one_with_two_options() {
    let mut q = question("q1", MustDecide, One);
    q.options = options(&["a", "a"]);
    assert_eq!(
        violations_of(q),
        vec![QuestionViolation::DuplicateOptionId {
            id: id("q1"),
            option: id("a")
        }]
    );
}

#[test]
fn duplicate_choice_for_several() {
    let mut q = question("q1", Recommended, Several);
    q.recommendation = recommend(&["a", "a", "b"]);
    assert_eq!(
        violations_of(q),
        vec![QuestionViolation::DuplicateChoice {
            id: id("q1"),
            choice: id("a")
        }]
    );
}

#[test]
fn wrong_choice_count() {
    let mut one = question("q1", Recommended, One);
    one.recommendation = recommend(&["a", "b"]);
    let mut several = question("q2", Recommended, Several);
    several.recommendation = recommend(&[]);
    let mut open = question("q3", MustDecide, Open);
    open.recommendation = recommend(&["a"]);
    let result = outcome(OutcomeStatus::NeedsInput, vec![one, several, open]).check();
    assert_eq!(
        result,
        Err(vec![
            QuestionViolation::ChoiceCount {
                id: id("q1"),
                shape: One,
                count: 2
            },
            QuestionViolation::ChoiceCount {
                id: id("q2"),
                shape: Several,
                count: 0
            },
            QuestionViolation::ChoiceCount {
                id: id("q3"),
                shape: Open,
                count: 1
            },
        ])
    );
}

#[test]
fn too_few_options() {
    let mut q = question("q1", MustDecide, Several);
    q.options = options(&["a"]);
    assert_eq!(
        violations_of(q),
        vec![QuestionViolation::TooFewOptions { id: id("q1") }]
    );
}

#[test]
fn bad_yes_no_options() {
    let mut q = question("q1", MustDecide, YesNo);
    q.options = options(&["yes", "nope"]);
    assert_eq!(
        violations_of(q),
        vec![QuestionViolation::YesNoOptions { id: id("q1") }]
    );
}

#[test]
fn open_with_options() {
    let mut q = question("q1", MustDecide, Open);
    q.options = options(&["a", "b"]);
    assert_eq!(
        violations_of(q),
        vec![QuestionViolation::OpenWithOptions { id: id("q1") }]
    );
}

#[test]
fn answer_on_a_shape_other_than_open() {
    let mut q = question("q1", Recommended, One);
    if let Some(r) = q.recommendation.as_mut() {
        r.answer = Some("a".into());
    }
    assert_eq!(
        violations_of(q),
        vec![QuestionViolation::AnswerNotOpen { id: id("q1") }]
    );
}

#[test]
fn needs_input_with_assumptions_only() {
    let result = outcome(
        OutcomeStatus::NeedsInput,
        vec![question("q1", Assumption, One)],
    )
    .check();
    assert_eq!(
        result,
        Err(vec![QuestionViolation::NeedsInputWithoutBlocking])
    );
}

#[test]
fn complete_holding_a_recommended_question() {
    let result = outcome(
        OutcomeStatus::Complete,
        vec![
            question("q1", Assumption, One),
            question("q2", Recommended, One),
        ],
    )
    .check();
    assert_eq!(
        result,
        Err(vec![QuestionViolation::BlockingNotNeedsInput {
            id: id("q2"),
            status: OutcomeStatus::Complete
        }])
    );
}

#[test]
fn failed_holding_a_must_decide_question() {
    let result = outcome(
        OutcomeStatus::Failed,
        vec![question("q1", MustDecide, Open)],
    )
    .check();
    assert_eq!(
        result,
        Err(vec![QuestionViolation::BlockingNotNeedsInput {
            id: id("q1"),
            status: OutcomeStatus::Failed
        }])
    );
}

#[test]
fn complete_with_assumptions_only_passes() {
    let result = outcome(
        OutcomeStatus::Complete,
        vec![
            question("q1", Assumption, One),
            question("q2", Assumption, Several),
            question("q3", Assumption, YesNo),
        ],
    )
    .check();
    assert_eq!(result, Ok(()));
}

#[test]
fn empty_complete_passes() {
    assert_eq!(outcome(OutcomeStatus::Complete, vec![]).check(), Ok(()));
}

#[test]
fn every_kind_in_needs_input_passes() {
    let result = outcome(
        OutcomeStatus::NeedsInput,
        vec![
            question("q1", MustDecide, Open),
            question("q2", Recommended, Several),
            question("q3", Assumption, One),
            question("q4", Recommended, Open),
        ],
    )
    .check();
    assert_eq!(result, Ok(()));
}

#[test]
fn all_violations_together() {
    let mut q = question("q1", Recommended, One);
    q.question = String::new();
    q.why = String::new();
    q.recommendation = None;
    let result = outcome(
        OutcomeStatus::Complete,
        vec![q, question("q1", Assumption, YesNo)],
    )
    .check();
    assert_eq!(
        result,
        Err(vec![
            QuestionViolation::MissingRecommendation { id: id("q1") },
            QuestionViolation::EmptyQuestion { id: id("q1") },
            QuestionViolation::EmptyWhy { id: id("q1") },
            QuestionViolation::DuplicateId { id: id("q1") },
            QuestionViolation::BlockingNotNeedsInput {
                id: id("q1"),
                status: OutcomeStatus::Complete
            },
        ])
    );
}

#[test]
fn violations_display_their_id() {
    let violation = QuestionViolation::ChoiceCount {
        id: id("q4"),
        shape: YesNo,
        count: 2,
    };
    assert_eq!(
        violation.to_string(),
        "q4: 2 choices do not fit shape yes_no"
    );
}
