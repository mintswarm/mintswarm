//! What a runner executes: stage jobs and step jobs, and what a stage returns.

use serde::{Deserialize, Serialize};

use crate::ids::{AttemptId, IssueNumber, JobId, RevId};
use crate::question::{Question, QuestionViolation, repeated};

/// A file written into a job's inputs, bound by its sha256.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InputFile {
    /// The path under `.mintswarm/job/inputs/`.
    pub path: String,
    /// The sha256 of the file's bytes, in lower-case hex.
    pub sha256: String,
}

/// One attempt at a stage, run by a harness.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StageJob {
    pub job: JobId,
    pub attempt: AttemptId,
    pub issue: IssueNumber,
    pub component: String,
    /// The stage's name, as in `.mintswarm/stages/<stage>.md`.
    pub stage: String,
    pub harness: String,
    pub model: String,
    pub effort: Option<String>,
    /// The commit the stage's tree is materialised from.
    pub base: RevId,
    /// The commit the stage definitions are read from.
    pub instructions: RevId,
    /// The artefact the stage must return, for example `plan`.
    pub artefact: String,
    pub inputs: Vec<InputFile>,
}

/// A deterministic step that needs the clone, a workspace or credentials.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepKind {
    Prepare,
    Commit,
    Gates,
    Push,
    /// Extract a bound input bundle.
    Bundle,
    KeepRound,
    DependencyInputs,
    LandingFacts,
    /// Send outbox rows.
    TrackerEffects,
    FileChildren,
}

impl StepKind {
    /// Every step, in the order design §5 lists them.
    pub const ALL: [StepKind; 10] = [
        StepKind::Prepare,
        StepKind::Commit,
        StepKind::Gates,
        StepKind::Push,
        StepKind::Bundle,
        StepKind::KeepRound,
        StepKind::DependencyInputs,
        StepKind::LandingFacts,
        StepKind::TrackerEffects,
        StepKind::FileChildren,
    ];
}

/// One attempt at a step.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StepJob {
    pub job: JobId,
    pub attempt: AttemptId,
    pub step: StepKind,
    pub base: RevId,
    pub head: Option<RevId>,
    pub inputs: Vec<InputFile>,
}

/// The status a stage reports in its result block.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutcomeStatus {
    Complete,
    NeedsInput,
    Failed,
}

/// What a stage returns: its result block, whichever producer read it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StageOutcome {
    pub status: OutcomeStatus,
    pub artefact: String,
    pub outputs: Vec<String>,
    /// The stage's open questions and assumptions; `needs_input` exactly when one is
    /// `must_decide` or `recommended`.
    pub questions: Vec<Question>,
    /// Non-empty for `failed`.
    pub reason: String,
    /// The commit message implement and address propose.
    pub commit: Option<String>,
    /// The PR title the first implement round proposes.
    pub pr_title: Option<String>,
}

impl StageOutcome {
    /// Checks the questions, and returns every violation at once, in this order:
    ///
    /// 1. each question's own violations, in question order (see [`Question::violations`]);
    /// 2. one `DuplicateId` per id that two or more questions share, in order of first use;
    /// 3. the status rule: `NeedsInputWithoutBlocking` when a `needs_input` outcome has no
    ///    `must_decide` or `recommended` question, or one `BlockingNotNeedsInput` per such
    ///    question in a `complete` or `failed` outcome.
    pub fn check(&self) -> Result<(), Vec<QuestionViolation>> {
        let mut out: Vec<QuestionViolation> = self
            .questions
            .iter()
            .flat_map(Question::violations)
            .collect();
        out.extend(
            repeated(self.questions.iter().map(|q| q.id.as_str()))
                .into_iter()
                .map(|id| QuestionViolation::DuplicateId { id: id.into() }),
        );
        match self.status {
            OutcomeStatus::NeedsInput => {
                if !self.questions.iter().any(Question::is_blocking) {
                    out.push(QuestionViolation::NeedsInputWithoutBlocking);
                }
            }
            OutcomeStatus::Complete | OutcomeStatus::Failed => {
                out.extend(self.questions.iter().filter(|q| q.is_blocking()).map(|q| {
                    QuestionViolation::BlockingNotNeedsInput {
                        id: q.id.clone(),
                        status: self.status,
                    }
                }));
            }
        }
        if out.is_empty() { Ok(()) } else { Err(out) }
    }
}

/// Token counts of an attempt.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tokens {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
}

/// What an attempt cost, and how the figure was obtained.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Cost {
    pub amount: f64,
    pub currency: String,
    /// How the amount was obtained, for example reported by the harness or estimated.
    pub basis: String,
}

/// A rate limit as the harness stream reports it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Limit {
    pub name: String,
    pub used_percent: f64,
    /// RFC 3339.
    pub resets_at: Option<String>,
}

/// Usage of an attempt, normalised into #3's shape.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Usage {
    pub tokens: Tokens,
    pub cost: Option<Cost>,
    pub limits: Vec<Limit>,
    /// Where the figures come from, for example the harness's stream.
    pub source: String,
    /// RFC 3339.
    pub observed_at: String,
}
