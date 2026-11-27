//! The stage contract over [`StageResult`]: the question checks and the
//! status rule of `.mintswarm/stages.md`, with the wording and order of
//! `mintswarm_domain::question`.

use std::collections::BTreeSet;
use std::fmt;

use super::{At, Receive, ReceiveError, Rule, enum_value, full_name};
use crate::runner::v1::{
    Question, QuestionKind, QuestionReferenceKind, QuestionShape, StageResult, StageStatus,
};

/// A rule a question or a stage result's questions break. Each names the
/// question's id, except the status rule for `needs_input`, which concerns
/// the result as a whole.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum QuestionViolation {
    /// A `recommended` or `assumption` question has no recommendation.
    MissingRecommendation {
        /// The question's id.
        id: String,
    },
    /// An `assumption` has shape `open`.
    AssumptionOpen {
        /// The question's id.
        id: String,
    },
    /// An `assumption` has no valid option besides those it chooses.
    AssumptionNoAlternative {
        /// The question's id.
        id: String,
    },
    /// `question` is empty or only whitespace.
    EmptyQuestion {
        /// The question's id.
        id: String,
    },
    /// `why` is empty or only whitespace.
    EmptyWhy {
        /// The question's id.
        id: String,
    },
    /// A shape of `one` or `several` has fewer than two options.
    TooFewOptions {
        /// The question's id.
        id: String,
    },
    /// A `yes_no` question has options whose ids are not exactly `yes` and
    /// `no`.
    YesNoOptions {
        /// The question's id.
        id: String,
    },
    /// An `open` question has options.
    OpenWithOptions {
        /// The question's id.
        id: String,
    },
    /// Two or more of the question's options share this option id.
    DuplicateOptionId {
        /// The question's id.
        id: String,
        /// The shared option id.
        option: String,
    },
    /// The recommendation chooses the wrong number of options for the shape.
    ChoiceCount {
        /// The question's id.
        id: String,
        /// The question's shape.
        shape: QuestionShape,
        /// How many ids the recommendation chooses.
        count: usize,
    },
    /// The recommendation chooses an id that is not a valid option for the
    /// shape.
    UnknownChoice {
        /// The question's id.
        id: String,
        /// The chosen id.
        choice: String,
    },
    /// The recommendation chooses this id more than once.
    DuplicateChoice {
        /// The question's id.
        id: String,
        /// The chosen id.
        choice: String,
    },
    /// The recommendation has an `answer` on a shape other than `open`.
    AnswerNotOpen {
        /// The question's id.
        id: String,
    },
    /// Two or more questions share this id.
    DuplicateId {
        /// The shared id.
        id: String,
    },
    /// A `needs_input` result has no `must_decide` or `recommended` question.
    NeedsInputWithoutBlocking,
    /// A `complete` or `failed` result holds a `must_decide` or `recommended`
    /// question.
    BlockingNotNeedsInput {
        /// The question's id.
        id: String,
        /// The result's status.
        status: StageStatus,
    },
}

/// The name a shape has in `.mintswarm/stages.md`.
fn shape_name(shape: QuestionShape) -> &'static str {
    match shape {
        QuestionShape::Unspecified => "unspecified",
        QuestionShape::One => "one",
        QuestionShape::Several => "several",
        QuestionShape::YesNo => "yes_no",
        QuestionShape::Open => "open",
    }
}

/// The name a status has in `.mintswarm/stages.md`.
fn status_name(status: StageStatus) -> &'static str {
    match status {
        StageStatus::Unspecified => "unspecified",
        StageStatus::Complete => "complete",
        StageStatus::NeedsInput => "needs_input",
        StageStatus::Failed => "failed",
    }
}

impl fmt::Display for QuestionViolation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            QuestionViolation::MissingRecommendation { id } => {
                write!(
                    f,
                    "{id}: a recommended or assumption question needs a recommendation"
                )
            }
            QuestionViolation::AssumptionOpen { id } => {
                write!(f, "{id}: an assumption cannot have shape open")
            }
            QuestionViolation::AssumptionNoAlternative { id } => {
                write!(
                    f,
                    "{id}: an assumption needs an option besides those it chooses"
                )
            }
            QuestionViolation::EmptyQuestion { id } => write!(f, "{id}: question is empty"),
            QuestionViolation::EmptyWhy { id } => write!(f, "{id}: why is empty"),
            QuestionViolation::TooFewOptions { id } => {
                write!(f, "{id}: shapes one and several need at least two options")
            }
            QuestionViolation::YesNoOptions { id } => {
                write!(f, "{id}: yes_no options must be exactly yes and no")
            }
            QuestionViolation::OpenWithOptions { id } => {
                write!(f, "{id}: an open question has no options")
            }
            QuestionViolation::DuplicateOptionId { id, option } => {
                write!(f, "{id}: option id {option:?} is not unique")
            }
            QuestionViolation::ChoiceCount { id, shape, count } => {
                let shape = shape_name(*shape);
                write!(f, "{id}: {count} choices do not fit shape {shape}")
            }
            QuestionViolation::UnknownChoice { id, choice } => {
                write!(f, "{id}: choice {choice:?} is not an option id")
            }
            QuestionViolation::DuplicateChoice { id, choice } => {
                write!(f, "{id}: choice {choice:?} is chosen more than once")
            }
            QuestionViolation::AnswerNotOpen { id } => {
                write!(
                    f,
                    "{id}: only an open question's recommendation has an answer"
                )
            }
            QuestionViolation::DuplicateId { id } => write!(f, "{id}: the id is not unique"),
            QuestionViolation::NeedsInputWithoutBlocking => {
                f.write_str("needs_input needs a must_decide or recommended question")
            }
            QuestionViolation::BlockingNotNeedsInput { id, status } => {
                let status = status_name(*status);
                write!(f, "{id}: a {status} outcome may hold assumptions only")
            }
        }
    }
}

impl std::error::Error for QuestionViolation {}

/// Returns every violation at once; see [`violations`] for the order.
impl Receive for StageResult {
    type Error = Vec<ReceiveError>;

    fn receive(self) -> Result<Self, Vec<ReceiveError>> {
        let name = full_name::<Self>();
        let out = violations(&At::root(&name), &self);
        if out.is_empty() { Ok(self) } else { Err(out) }
    }
}

/// The ids that occur more than once, each once, in order of first use.
fn repeated<'a>(ids: impl IntoIterator<Item = &'a str>) -> Vec<&'a str> {
    let mut seen: Vec<&str> = Vec::new();
    let mut repeated: Vec<&str> = Vec::new();
    for id in ids {
        if !seen.contains(&id) {
            seen.push(id);
        } else if !repeated.contains(&id) {
            repeated.push(id);
        }
    }
    repeated
}

/// Whether a question of the raw `kind` makes its result blocking; a kind
/// this build does not know is non-blocking.
fn is_blocking(kind: i32) -> bool {
    matches!(
        QuestionKind::try_from(kind),
        Ok(QuestionKind::MustDecide | QuestionKind::Recommended)
    )
}

/// Every violation of `result`, at `at`, in this order:
///
/// 1. `status`: `Unspecified` or `UnknownValue`;
/// 2. each question in turn: first its field rules (an empty `id`, an
///    unspecified or unknown `kind` or `shape`, an empty option id, an
///    unspecified or unknown reference kind), then, unless its `kind` or
///    `shape` broke a field rule, its own violations (see
///    [`question_violations`]);
/// 3. one `DuplicateId` per id that two or more questions share, in order of
///    first use, on `questions`;
/// 4. unless `status` broke its field rule, the status rule on `status`:
///    `NeedsInputWithoutBlocking` when a `needs_input` result has no
///    `must_decide` or `recommended` question, or one `BlockingNotNeedsInput`
///    per such question in a `complete` or `failed` result.
pub(super) fn violations(at: &At<'_>, result: &StageResult) -> Vec<ReceiveError> {
    let mut out = Vec::new();
    let status = match enum_value::<StageStatus>(result.status) {
        Ok(status) => Some(status),
        Err(rule) => {
            out.push(at.fail("status", rule));
            None
        }
    };
    for (index, question) in result.questions.iter().enumerate() {
        let at = at.item("questions", index);
        if question.id.is_empty() {
            out.push(at.fail("id", Rule::EmptyId));
        }
        let kind = enum_value::<QuestionKind>(question.kind)
            .map_err(|rule| out.push(at.fail("kind", rule)))
            .ok();
        let shape = enum_value::<QuestionShape>(question.shape)
            .map_err(|rule| out.push(at.fail("shape", rule)))
            .ok();
        for (option_index, option) in question.options.iter().enumerate() {
            if option.id.is_empty() {
                out.push(at.item("options", option_index).fail("id", Rule::EmptyId));
            }
        }
        for (reference_index, reference) in question.references.iter().enumerate() {
            if let Err(rule) = enum_value::<QuestionReferenceKind>(reference.kind) {
                out.push(at.item("references", reference_index).fail("kind", rule));
            }
        }
        if let (Some(kind), Some(shape)) = (kind, shape) {
            out.extend(
                question_violations(question, kind, shape)
                    .into_iter()
                    .map(|(field, violation)| at.fail(field, Rule::Question(violation))),
            );
        }
    }
    out.extend(
        repeated(result.questions.iter().map(|q| q.id.as_str()))
            .into_iter()
            .map(|id| {
                at.fail(
                    "questions",
                    Rule::Question(QuestionViolation::DuplicateId { id: id.into() }),
                )
            }),
    );
    match status {
        Some(StageStatus::NeedsInput) => {
            if !result.questions.iter().any(|q| is_blocking(q.kind)) {
                out.push(at.fail(
                    "status",
                    Rule::Question(QuestionViolation::NeedsInputWithoutBlocking),
                ));
            }
        }
        Some(status @ (StageStatus::Complete | StageStatus::Failed)) => {
            out.extend(
                result
                    .questions
                    .iter()
                    .filter(|q| is_blocking(q.kind))
                    .map(|q| {
                        at.fail(
                            "status",
                            Rule::Question(QuestionViolation::BlockingNotNeedsInput {
                                id: q.id.clone(),
                                status,
                            }),
                        )
                    }),
            );
        }
        Some(StageStatus::Unspecified) | None => {}
    }
    out
}

/// The ids a recommendation may choose: the option ids, `yes` and `no` for
/// `yes_no` without options, and none for `open`.
fn valid_choices(question: &Question, shape: QuestionShape) -> Vec<&str> {
    match shape {
        QuestionShape::Open => Vec::new(),
        QuestionShape::YesNo if question.options.is_empty() => vec!["yes", "no"],
        _ => question.options.iter().map(|o| o.id.as_str()).collect(),
    }
}

/// The violations of `question` alone, each with the field it concerns, in
/// this order:
///
/// 1. `MissingRecommendation` on `recommendation`: `recommended` and
///    `assumption` need a recommendation.
/// 2. `AssumptionOpen` on `shape`: an `assumption` has shape `one`,
///    `several` or `yes_no`.
/// 3. `AssumptionNoAlternative` on `options`: an `assumption` of another
///    shape has at least one valid choice id (see 10) besides those its
///    recommendation chooses.
/// 4. `EmptyQuestion` on `question` and `EmptyWhy` on `why`: neither is empty
///    or whitespace.
/// 5. `TooFewOptions` on `options`: `one` and `several` have at least two
///    options.
/// 6. `YesNoOptions` on `options`: `yes_no` has no options, or exactly two
///    with the ids `yes` and `no`.
/// 7. `OpenWithOptions` on `options`: `open` has no options.
/// 8. `DuplicateOptionId` on `options`, once per option id that two or more
///    options share, in order of first use.
///
/// With a recommendation:
///
/// 9. `ChoiceCount` on `recommendation.choice`: it holds exactly one id for
///    `one` and `yes_no`, at least one for `several`, and none for `open`.
/// 10. `UnknownChoice` on `recommendation.choice`, once per bad id in
///     `choice` order, except for `open`: each id is an option id, or `yes` or
///     `no` for `yes_no` without options.
/// 11. `DuplicateChoice` on `recommendation.choice`, once per id that
///     `choice` holds more than once, in order of first use.
/// 12. `AnswerNotOpen` on `recommendation.answer`: `answer` is set only for
///     `open`.
fn question_violations(
    question: &Question,
    kind: QuestionKind,
    shape: QuestionShape,
) -> Vec<(&'static str, QuestionViolation)> {
    let id = || question.id.clone();
    let mut out = Vec::new();
    let choice: &[String] = question
        .recommendation
        .as_ref()
        .map_or(&[], |r| r.choice.as_slice());
    let valid = valid_choices(question, shape);

    if kind != QuestionKind::MustDecide && question.recommendation.is_none() {
        out.push((
            "recommendation",
            QuestionViolation::MissingRecommendation { id: id() },
        ));
    }
    if kind == QuestionKind::Assumption {
        if shape == QuestionShape::Open {
            out.push(("shape", QuestionViolation::AssumptionOpen { id: id() }));
        } else if valid.iter().all(|v| choice.iter().any(|c| c == v)) {
            out.push((
                "options",
                QuestionViolation::AssumptionNoAlternative { id: id() },
            ));
        }
    }
    if question.question.trim().is_empty() {
        out.push(("question", QuestionViolation::EmptyQuestion { id: id() }));
    }
    if question.why.trim().is_empty() {
        out.push(("why", QuestionViolation::EmptyWhy { id: id() }));
    }
    match shape {
        QuestionShape::One | QuestionShape::Several if question.options.len() < 2 => {
            out.push(("options", QuestionViolation::TooFewOptions { id: id() }));
        }
        QuestionShape::YesNo if !question.options.is_empty() => {
            let ids: BTreeSet<&str> = question.options.iter().map(|o| o.id.as_str()).collect();
            if question.options.len() != 2 || ids != BTreeSet::from(["no", "yes"]) {
                out.push(("options", QuestionViolation::YesNoOptions { id: id() }));
            }
        }
        QuestionShape::Open if !question.options.is_empty() => {
            out.push(("options", QuestionViolation::OpenWithOptions { id: id() }));
        }
        _ => {}
    }
    out.extend(
        repeated(question.options.iter().map(|o| o.id.as_str()))
            .into_iter()
            .map(|option| {
                (
                    "options",
                    QuestionViolation::DuplicateOptionId {
                        id: id(),
                        option: option.into(),
                    },
                )
            }),
    );
    if let Some(recommendation) = &question.recommendation {
        let count = choice.len();
        let fits = match shape {
            QuestionShape::One | QuestionShape::YesNo => count == 1,
            QuestionShape::Several => count >= 1,
            QuestionShape::Open => count == 0,
            QuestionShape::Unspecified => true,
        };
        if !fits {
            out.push((
                "recommendation.choice",
                QuestionViolation::ChoiceCount {
                    id: id(),
                    shape,
                    count,
                },
            ));
        }
        if shape != QuestionShape::Open {
            for c in choice.iter().filter(|c| !valid.contains(&c.as_str())) {
                out.push((
                    "recommendation.choice",
                    QuestionViolation::UnknownChoice {
                        id: id(),
                        choice: c.clone(),
                    },
                ));
            }
        }
        out.extend(
            repeated(choice.iter().map(String::as_str))
                .into_iter()
                .map(|c| {
                    (
                        "recommendation.choice",
                        QuestionViolation::DuplicateChoice {
                            id: id(),
                            choice: c.into(),
                        },
                    )
                }),
        );
        if recommendation.answer.is_some() && shape != QuestionShape::Open {
            out.push((
                "recommendation.answer",
                QuestionViolation::AnswerNotOpen { id: id() },
            ));
        }
    }
    out
}
