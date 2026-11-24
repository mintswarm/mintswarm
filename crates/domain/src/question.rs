//! The question object a stage or `ask_human` leaves for a person, and its checks.
//!
//! Optional fields may be absent or `null`, and the two mean the same. Serialisation writes every
//! key, with `null` and `[]` for empty fields, so an outcome has exactly one JSON form.

use std::collections::BTreeSet;
use std::fmt;

use serde::{Deserialize, Deserializer, Serialize};

use crate::work::OutcomeStatus;

/// Reads `null` as the type's default, so that `null` and an absent key mean the same.
fn null_as_default<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Default + Deserialize<'de>,
{
    Ok(Option::<T>::deserialize(deserializer)?.unwrap_or_default())
}

/// The ids that occur more than once, each once, in order of first use.
pub(crate) fn repeated<'a>(ids: impl IntoIterator<Item = &'a str>) -> Vec<&'a str> {
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

/// One open question or assumption.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Question {
    /// Unique within the outcome or set, for example `q1`.
    pub id: String,
    pub kind: QuestionKind,
    /// The question itself; never empty.
    pub question: String,
    /// Why it matters; never empty.
    pub why: String,
    pub shape: QuestionShape,
    /// At least two for `one` and `several`; none or exactly `yes` and `no` for `yes_no`; none
    /// for `open`.
    #[serde(default, deserialize_with = "null_as_default")]
    pub options: Vec<QuestionOption>,
    /// Required for `recommended` and `assumption`; a weak one is allowed on `must_decide`.
    #[serde(default)]
    pub recommendation: Option<Recommendation>,
    #[serde(default, deserialize_with = "null_as_default")]
    pub references: Vec<Reference>,
}

/// How a question blocks the item.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuestionKind {
    /// A person must answer; blocking.
    MustDecide,
    /// A person should confirm the recommendation; blocking.
    Recommended,
    /// The stage went ahead with the recommendation; non-blocking.
    Assumption,
}

impl QuestionKind {
    /// Whether a question of this kind makes its set blocking.
    pub fn is_blocking(self) -> bool {
        matches!(self, QuestionKind::MustDecide | QuestionKind::Recommended)
    }
}

/// The form of the answer a question expects.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuestionShape {
    /// Exactly one option.
    One,
    /// One or more options.
    Several,
    /// Yes or no.
    YesNo,
    /// Free text.
    Open,
}

impl fmt::Display for QuestionShape {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            QuestionShape::One => "one",
            QuestionShape::Several => "several",
            QuestionShape::YesNo => "yes_no",
            QuestionShape::Open => "open",
        })
    }
}

/// One answer a question offers.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuestionOption {
    /// Unique within the question, for example `a`.
    pub id: String,
    pub label: String,
    /// What choosing it leads to.
    pub consequence: String,
    #[serde(default)]
    pub trade_off: Option<String>,
}

/// The answer the asker recommends.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Recommendation {
    /// Option ids: one for `one` and `yes_no`, one or more for `several`, none for `open`.
    #[serde(default, deserialize_with = "null_as_default")]
    pub choice: Vec<String>,
    /// Free text, only for `open`.
    #[serde(default)]
    pub answer: Option<String>,
    pub reason: String,
}

/// Something a question points at.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reference {
    pub kind: ReferenceKind,
    /// A path, artefact, ticket, design section, `<set>/<id>` or URL, by `kind`.
    pub target: String,
    /// A line range, for example `40-72`.
    #[serde(default)]
    pub lines: Option<String>,
}

/// What a reference's target is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReferenceKind {
    File,
    Artefact,
    Ticket,
    Design,
    /// An earlier answer, named `<set>/<id>`.
    Answer,
    Url,
}

/// A rule a question or an outcome's questions break. Each names the question's id, except the
/// status rule for `needs_input`, which concerns the outcome as a whole.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum QuestionViolation {
    /// A `recommended` or `assumption` question has no recommendation.
    MissingRecommendation { id: String },
    /// An `assumption` has shape `open`.
    AssumptionOpen { id: String },
    /// An `assumption` has no valid option besides those it chooses.
    AssumptionNoAlternative { id: String },
    /// `question` is empty or only whitespace.
    EmptyQuestion { id: String },
    /// `why` is empty or only whitespace.
    EmptyWhy { id: String },
    /// A shape of `one` or `several` has fewer than two options.
    TooFewOptions { id: String },
    /// A `yes_no` question has options whose ids are not exactly `yes` and `no`.
    YesNoOptions { id: String },
    /// An `open` question has options.
    OpenWithOptions { id: String },
    /// Two or more of the question's options share this option id.
    DuplicateOptionId { id: String, option: String },
    /// The recommendation chooses the wrong number of options for the shape.
    ChoiceCount {
        id: String,
        shape: QuestionShape,
        count: usize,
    },
    /// The recommendation chooses an id that is not a valid option for the shape.
    UnknownChoice { id: String, choice: String },
    /// The recommendation chooses this id more than once.
    DuplicateChoice { id: String, choice: String },
    /// The recommendation has an `answer` on a shape other than `open`.
    AnswerNotOpen { id: String },
    /// Two or more questions share this id.
    DuplicateId { id: String },
    /// A `needs_input` outcome has no `must_decide` or `recommended` question.
    NeedsInputWithoutBlocking,
    /// A `complete` or `failed` outcome holds a `must_decide` or `recommended` question.
    BlockingNotNeedsInput { id: String, status: OutcomeStatus },
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
                let status = match status {
                    OutcomeStatus::Complete => "complete",
                    OutcomeStatus::NeedsInput => "needs_input",
                    OutcomeStatus::Failed => "failed",
                };
                write!(f, "{id}: a {status} outcome may hold assumptions only")
            }
        }
    }
}

impl std::error::Error for QuestionViolation {}

impl Question {
    /// Whether this question makes its set blocking.
    pub fn is_blocking(&self) -> bool {
        self.kind.is_blocking()
    }

    /// The ids a recommendation may choose: the option ids, `yes` and `no` for `yes_no` without
    /// options, and none for `open`.
    fn valid_choices(&self) -> Vec<&str> {
        match self.shape {
            QuestionShape::Open => Vec::new(),
            QuestionShape::YesNo if self.options.is_empty() => vec!["yes", "no"],
            _ => self.options.iter().map(|o| o.id.as_str()).collect(),
        }
    }

    /// The violations of this question alone, in this order:
    ///
    /// 1. `MissingRecommendation`: `recommended` and `assumption` need a recommendation.
    /// 2. `AssumptionOpen`: an `assumption` has shape `one`, `several` or `yes_no`.
    /// 3. `AssumptionNoAlternative`: an `assumption` of another shape has at least one valid
    ///    choice id (see 10) besides those its recommendation chooses.
    /// 4. `EmptyQuestion` and `EmptyWhy`: `question` and `why` are not empty or whitespace.
    /// 5. `TooFewOptions`: `one` and `several` have at least two options.
    /// 6. `YesNoOptions`: `yes_no` has no options, or exactly two with the ids `yes` and `no`.
    /// 7. `OpenWithOptions`: `open` has no options.
    /// 8. `DuplicateOptionId`, once per option id that two or more options share, in order of
    ///    first use.
    ///
    /// With a recommendation:
    ///
    /// 9. `ChoiceCount`: `choice` holds exactly one id for `one` and `yes_no`, at least one for
    ///    `several`, and none for `open`.
    /// 10. `UnknownChoice`, once per bad id in `choice` order, except for `open`: each id is an
    ///     option id, or `yes` or `no` for `yes_no` without options.
    /// 11. `DuplicateChoice`, once per id that `choice` holds more than once, in order of first
    ///     use.
    /// 12. `AnswerNotOpen`: `answer` is set only for `open`.
    pub fn violations(&self) -> Vec<QuestionViolation> {
        let id = || self.id.clone();
        let mut out = Vec::new();
        let choice: &[String] = self
            .recommendation
            .as_ref()
            .map_or(&[], |r| r.choice.as_slice());
        let valid = self.valid_choices();

        if self.kind != QuestionKind::MustDecide && self.recommendation.is_none() {
            out.push(QuestionViolation::MissingRecommendation { id: id() });
        }
        if self.kind == QuestionKind::Assumption {
            if self.shape == QuestionShape::Open {
                out.push(QuestionViolation::AssumptionOpen { id: id() });
            } else if valid.iter().all(|v| choice.iter().any(|c| c == v)) {
                out.push(QuestionViolation::AssumptionNoAlternative { id: id() });
            }
        }
        if self.question.trim().is_empty() {
            out.push(QuestionViolation::EmptyQuestion { id: id() });
        }
        if self.why.trim().is_empty() {
            out.push(QuestionViolation::EmptyWhy { id: id() });
        }
        match self.shape {
            QuestionShape::One | QuestionShape::Several if self.options.len() < 2 => {
                out.push(QuestionViolation::TooFewOptions { id: id() });
            }
            QuestionShape::YesNo if !self.options.is_empty() => {
                let ids: BTreeSet<&str> = self.options.iter().map(|o| o.id.as_str()).collect();
                if self.options.len() != 2 || ids != BTreeSet::from(["no", "yes"]) {
                    out.push(QuestionViolation::YesNoOptions { id: id() });
                }
            }
            QuestionShape::Open if !self.options.is_empty() => {
                out.push(QuestionViolation::OpenWithOptions { id: id() });
            }
            _ => {}
        }
        out.extend(
            repeated(self.options.iter().map(|o| o.id.as_str()))
                .into_iter()
                .map(|option| QuestionViolation::DuplicateOptionId {
                    id: id(),
                    option: option.into(),
                }),
        );
        if let Some(recommendation) = &self.recommendation {
            let count = choice.len();
            let fits = match self.shape {
                QuestionShape::One | QuestionShape::YesNo => count == 1,
                QuestionShape::Several => count >= 1,
                QuestionShape::Open => count == 0,
            };
            if !fits {
                out.push(QuestionViolation::ChoiceCount {
                    id: id(),
                    shape: self.shape,
                    count,
                });
            }
            if self.shape != QuestionShape::Open {
                for c in choice.iter().filter(|c| !valid.contains(&c.as_str())) {
                    out.push(QuestionViolation::UnknownChoice {
                        id: id(),
                        choice: c.clone(),
                    });
                }
            }
            out.extend(
                repeated(choice.iter().map(String::as_str))
                    .into_iter()
                    .map(|c| QuestionViolation::DuplicateChoice {
                        id: id(),
                        choice: c.into(),
                    }),
            );
            if recommendation.answer.is_some() && self.shape != QuestionShape::Open {
                out.push(QuestionViolation::AnswerNotOpen { id: id() });
            }
        }
        out
    }
}
