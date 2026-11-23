//! The transition table of §6 as rows of data, its named guards and actions, and the lookup.

use super::{
    ATTEMPT_CAP, AssignReason, Escalation, EscalationCause, Event, Facts, GATE_ROUND_CAP, Job,
    JobKind, JobState, Step, named_enum,
};
use std::fmt;
use std::sync::LazyLock;

named_enum! {
    /// A named guard of a row; [`Guard::holds`] evaluates it on the job and the caller's facts.
    pub enum Guard {
        /// Holds always.
        Always => "always",
        /// The facts allow dispatch and the job is not paused.
        Dispatchable => "dispatchable",
        /// Kind `implementation`, and the analysis proposes a split.
        AnalysisSplit => "analysis_split",
        /// Kind `implementation`, and the analysis proposes no split.
        AnalysisNoSplit => "analysis_no_split",
        /// Kind `design`.
        KindDesign => "kind_design",
        /// Kind `plan`.
        KindPlan => "kind_plan",
        /// The approval refers to the current artefact's sha256.
        ArtefactApproved => "artefact_approved",
        /// The approval refers to the current plan's sha256, and the base has not moved or the
        /// stale-plan check passes.
        PlanApproved => "plan_approved",
        /// The decision is `auto`, and `replan` did not force a human approval (N3).
        DecisionAuto => "decision_auto",
        /// The decision is not `auto`, or `replan` forced a human approval.
        DecisionHuman => "decision_human",
        /// A trusted author answered.
        TrustedAnswer => "trusted_answer",
        /// The failure that just ended is not the last of [`ATTEMPT_CAP`].
        AttemptsLeft => "attempts_left",
        /// The failure that just ended is the last of [`ATTEMPT_CAP`].
        AttemptsExhausted => "attempts_exhausted",
        /// The job itself was cancelled.
        JobCancelled => "job_cancelled",
        /// The job was not cancelled; only the attempt was.
        JobNotCancelled => "job_not_cancelled",
        /// The branch has not been pushed yet.
        NotYetPushed => "not_yet_pushed",
        /// A later push of an agent round.
        AgentRoundPush => "agent_round_push",
        /// A later push of an address round requested by a human from `ready`.
        HumanRoundPush => "human_round_push",
        /// Gate rounds are left for this step, and `gates.summary` is off.
        GateRoundsLeft => "gate_rounds_left",
        /// Gate rounds are left for this step, and `gates.summary: agent`.
        GateRoundsLeftSummarised => "gate_rounds_left_summarised",
        /// The failed round is the last of [`GATE_ROUND_CAP`] for this step.
        GateRoundsExhausted => "gate_rounds_exhausted",
        /// A `blocker` or `major` finding is open, and the round is below the review cap.
        ReviewRoundsLeft => "review_rounds_left",
        /// A `blocker` or `major` finding is open, and the round is at the review cap.
        ReviewCapReached => "review_cap_reached",
        /// No `blocker` or `major` finding is open.
        NoBlockingFindings => "no_blocking_findings",
        /// The PR is approved, clean and at the recorded head.
        MergeReady => "merge_ready",
        /// The merged PR's head equals the recorded head.
        MergedHeadMatches => "merged_head_matches",
        /// The PR's `external_steps` are empty.
        NoExternalSteps => "no_external_steps",
        /// The PR's `external_steps` are not empty.
        ExternalSteps => "external_steps",
        /// The open escalation came from the review cap.
        EscalatedByReviewCap => "escalated_by_review_cap",
        /// The open escalation came from anything but the review cap.
        EscalatedOtherwise => "escalated_otherwise",
    }
}

impl Guard {
    /// Whether the guard holds for `job` under `facts`.
    pub fn holds(self, job: &Job, facts: &Facts) -> bool {
        let c = &job.counters;
        let implementation = job.kind == JobKind::Implementation;
        let gates_left = c.gate_rounds + 1 < GATE_ROUND_CAP;
        let below_cap = c.review_round < c.review_cap(facts.review_max_rounds);
        let auto = facts.decision_auto && !c.plan_approval_forced;
        let by_review_cap = c.escalation.map(|e| e.cause == EscalationCause::ReviewCap);
        match self {
            Self::Always => true,
            Self::Dispatchable => facts.dispatchable && !job.paused,
            Self::AnalysisSplit => implementation && facts.split_proposed,
            Self::AnalysisNoSplit => implementation && !facts.split_proposed,
            Self::KindDesign => job.kind == JobKind::Design,
            Self::KindPlan => job.kind == JobKind::Plan,
            Self::ArtefactApproved => facts.approval_matches,
            Self::PlanApproved => facts.approval_matches && facts.base_current,
            Self::DecisionAuto => auto,
            Self::DecisionHuman => !auto,
            Self::TrustedAnswer => facts.trusted_answer,
            Self::AttemptsLeft => c.attempts + 1 < ATTEMPT_CAP,
            Self::AttemptsExhausted => c.attempts + 1 >= ATTEMPT_CAP,
            Self::JobCancelled => facts.job_cancelled,
            Self::JobNotCancelled => !facts.job_cancelled,
            Self::NotYetPushed => !facts.pushed_before,
            Self::AgentRoundPush => facts.pushed_before && !c.human_round,
            Self::HumanRoundPush => facts.pushed_before && c.human_round,
            Self::GateRoundsLeft => gates_left && !facts.gates_summary_agent,
            Self::GateRoundsLeftSummarised => gates_left && facts.gates_summary_agent,
            Self::GateRoundsExhausted => !gates_left,
            Self::ReviewRoundsLeft => facts.blocking_findings_open && below_cap,
            Self::ReviewCapReached => facts.blocking_findings_open && !below_cap,
            Self::NoBlockingFindings => !facts.blocking_findings_open,
            Self::MergeReady => facts.merge_ready,
            Self::MergedHeadMatches => facts.merged_head_matches,
            Self::NoExternalSteps => !facts.external_steps,
            Self::ExternalSteps => facts.external_steps,
            Self::EscalatedByReviewCap => by_review_cap == Some(true),
            Self::EscalatedOtherwise => by_review_cap == Some(false),
        }
    }
}

/// What a row does besides moving the job. The domain applies the counter actions to the next
/// [`Job`]; the plane that runs the table carries out the others.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Action {
    /// Counts one failed attempt of the current stage.
    CountAttempt,
    /// Starts the attempt count of a newly entered stage at 0.
    ResetAttempts,
    /// Counts one failed gate round of the current step.
    CountGateRound,
    /// Starts the gate-round count of a new implement or address step at 0.
    ResetGateRounds,
    /// Starts the next agent review round.
    StartReviewRound,
    /// Resets the review round count to 0 (`steer`, `replan`; decision 6).
    ResetReviewRounds,
    /// Raises the review cap by one for this job (`continue`).
    RaiseReviewCap,
    /// Records the step that produces the next change.
    SetStep(Step),
    /// Marks the address round as requested by a human from `ready`.
    SetHumanRound,
    /// Ends the human-requested address round.
    ClearHumanRound,
    /// Makes the next plan decision go to a human, whatever the mode (N3).
    ForcePlanApproval,
    /// Lifts [`Action::ForcePlanApproval`] once a human approved the plan.
    ClearForcedPlanApproval,
    /// Records the escalation's cause and the state to resume.
    Escalate(EscalationCause),
    /// Clears the escalation on a maintainer command.
    ClearEscalation,
    /// Records the stage that `awaiting_input` resumes.
    RecordResume,
    /// Clears the recorded stage once it resumes.
    ClearResume,
    /// Sets the job's `paused` flag until the throttle's `resets_at`.
    Pause,
    /// Changes the job's kind to `plan` (#146).
    SetKindPlan,
    /// Saves the stage's envelope and publishes it as an issue artifact.
    PublishArtefact,
    /// Assigns the issue or PR.
    Assign(AssignReason),
    /// Releases the job's assignments.
    ReleaseIssue,
    /// Records the approval with the sha256 of the artefact it refers to.
    RecordApproval,
    /// Adds the approval comment as an input of the producing stage.
    AddCommentInput,
    /// Adds the trusted answer as an input of the resumed stage.
    AddAnswerInput,
    /// Adds the steer text as an input (`inputs/steer.md`).
    AddSteerInput,
    /// Adds the open findings as an input of the next plan.
    AddFindingsInput,
    /// Writes `inputs/gates.md` from the failed round, with the summary if there is one.
    WriteGatesInput,
    /// Sets the issue type to `Plan` (`tracker.kinds.plan.issue_type`).
    SetIssueTypePlan,
    /// Queues the bot comment naming the analysis finding that re-classified the issue.
    CommentReclassification,
    /// Posts the plan's TL;DR comment.
    PostTldr,
    /// Posts the auto-approval comment.
    PostAutoComment,
    /// Posts the plan's approval package.
    PostApprovalPackage,
    /// Stops the running attempt or step.
    CancelAttempt,
    /// Keeps the uncommitted diff as a record.
    KeepDiffRecord,
    /// Opens the draft PR.
    OpenDraftPr,
    /// Prepares the merge of the base into the pushed branch, for conflict mode.
    PrepareMerge,
    /// Posts the escalation comment, which links the last rounds and lists the commands.
    PostEscalationComment,
    /// Marks the PR ready, requests review and posts the approval comment.
    MarkPrReady,
    /// Re-requests review from the reviewer who requested changes.
    ReRequestReview,
    /// Writes the merge-ready record (PR, head, approval) for the steward.
    WriteMergeReadyRecord,
    /// Posts the external-steps comment.
    PostExternalStepsComment,
    /// Closes the issue.
    CloseIssue,
    /// Records the open findings as documented limits in the PR body and the approval request.
    RecordDocumentedLimits,
    /// Moves the PR back to draft, if one is open.
    PrToDraft,
    /// The hand-over: closes the PR if one is open and keeps the branch.
    ClosePrKeepBranch,
}

impl Action {
    /// The action's stable snake_case name, without its value.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::CountAttempt => "count_attempt",
            Self::ResetAttempts => "reset_attempts",
            Self::CountGateRound => "count_gate_round",
            Self::ResetGateRounds => "reset_gate_rounds",
            Self::StartReviewRound => "start_review_round",
            Self::ResetReviewRounds => "reset_review_rounds",
            Self::RaiseReviewCap => "raise_review_cap",
            Self::SetStep(_) => "set_step",
            Self::SetHumanRound => "set_human_round",
            Self::ClearHumanRound => "clear_human_round",
            Self::ForcePlanApproval => "force_plan_approval",
            Self::ClearForcedPlanApproval => "clear_forced_plan_approval",
            Self::Escalate(_) => "escalate",
            Self::ClearEscalation => "clear_escalation",
            Self::RecordResume => "record_resume",
            Self::ClearResume => "clear_resume",
            Self::Pause => "pause",
            Self::SetKindPlan => "set_kind_plan",
            Self::PublishArtefact => "publish_artefact",
            Self::Assign(_) => "assign",
            Self::ReleaseIssue => "release_issue",
            Self::RecordApproval => "record_approval",
            Self::AddCommentInput => "add_comment_input",
            Self::AddAnswerInput => "add_answer_input",
            Self::AddSteerInput => "add_steer_input",
            Self::AddFindingsInput => "add_findings_input",
            Self::WriteGatesInput => "write_gates_input",
            Self::SetIssueTypePlan => "set_issue_type_plan",
            Self::CommentReclassification => "comment_reclassification",
            Self::PostTldr => "post_tldr",
            Self::PostAutoComment => "post_auto_comment",
            Self::PostApprovalPackage => "post_approval_package",
            Self::CancelAttempt => "cancel_attempt",
            Self::KeepDiffRecord => "keep_diff_record",
            Self::OpenDraftPr => "open_draft_pr",
            Self::PrepareMerge => "prepare_merge",
            Self::PostEscalationComment => "post_escalation_comment",
            Self::MarkPrReady => "mark_pr_ready",
            Self::ReRequestReview => "re_request_review",
            Self::WriteMergeReadyRecord => "write_merge_ready_record",
            Self::PostExternalStepsComment => "post_external_steps_comment",
            Self::CloseIssue => "close_issue",
            Self::RecordDocumentedLimits => "record_documented_limits",
            Self::PrToDraft => "pr_to_draft",
            Self::ClosePrKeepBranch => "close_pr_keep_branch",
        }
    }

    /// Applies the action's counter change, if it has one, to `next`, the job leaving `from`.
    fn apply(self, from: JobState, next: &mut Job) {
        let c = &mut next.counters;
        match self {
            Self::CountAttempt => c.attempts += 1,
            Self::ResetAttempts => c.attempts = 0,
            Self::CountGateRound => c.gate_rounds += 1,
            Self::ResetGateRounds => c.gate_rounds = 0,
            Self::StartReviewRound => c.review_round += 1,
            Self::ResetReviewRounds => c.review_round = 0,
            Self::RaiseReviewCap => c.review_cap_extensions += 1,
            Self::SetStep(step) => c.step = step,
            Self::SetHumanRound => c.human_round = true,
            Self::ClearHumanRound => c.human_round = false,
            Self::ForcePlanApproval => c.plan_approval_forced = true,
            Self::ClearForcedPlanApproval => c.plan_approval_forced = false,
            Self::Escalate(cause) => {
                let resume = cause.resume_state(from, c.step);
                c.escalation = Some(Escalation { cause, resume });
            }
            Self::ClearEscalation => c.escalation = None,
            Self::RecordResume => c.resume = Some(from),
            Self::ClearResume => c.resume = None,
            Self::Pause => next.paused = true,
            Self::SetKindPlan => next.kind = JobKind::Plan,
            _ => {}
        }
    }
}

impl fmt::Display for Action {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SetStep(step) => write!(f, "set_step({})", step.state()),
            Self::Escalate(cause) => write!(f, "escalate({cause})"),
            Self::Assign(reason) => write!(f, "assign({reason})"),
            other => f.write_str(other.as_str()),
        }
    }
}

/// Where a row leads.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Target {
    /// A fixed state.
    Fixed(JobState),
    /// The recorded state: the escalation's resume state in `escalated`, otherwise the stage
    /// recorded by `stage_needs_input`.
    Resume,
    /// The step that produced the change: `implementing` or `addressing`.
    CurrentStep,
}

/// One row of the table: (state, event, guard) → (target, actions).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    /// The state the row leaves.
    pub from: JobState,
    /// The event the row takes.
    pub event: Event,
    /// The guard that must hold.
    pub guard: Guard,
    /// Where the row leads.
    pub target: Target,
    /// The row's actions, in order.
    pub actions: Vec<Action>,
}

/// The result of a transition.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Transition {
    /// The index of the matched row in [`table`].
    pub row: usize,
    /// The job after the transition, with the counter actions applied.
    pub job: Job,
    /// The matched row's actions, for the plane to carry out.
    pub actions: &'static [Action],
}

/// Why a transition was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransitionError {
    /// No row of the table has this (state, event) pair.
    NotInTable {
        /// The job's state.
        state: JobState,
        /// The refused event.
        event: Event,
    },
    /// Rows exist for the pair, but none of their guards holds.
    GuardRefused {
        /// The job's state.
        state: JobState,
        /// The refused event.
        event: Event,
    },
    /// The row resumes a recorded state, and the job has none.
    NoResumeState {
        /// The job's state.
        state: JobState,
        /// The refused event.
        event: Event,
    },
}

impl fmt::Display for TransitionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotInTable { state, event } => {
                write!(f, "no row takes event {event} in state {state}")
            }
            Self::GuardRefused { state, event } => {
                write!(f, "no guard holds for event {event} in state {state}")
            }
            Self::NoResumeState { state, event } => {
                write!(f, "event {event} in state {state} has no state to resume")
            }
        }
    }
}

impl std::error::Error for TransitionError {}

/// The whole table, built once.
pub fn table() -> &'static [Row] {
    &TABLE
}

/// The rows, with their indices in [`table`], that take `event` in `state`; an error when there
/// are none.
pub fn rows_for(
    state: JobState,
    event: Event,
) -> Result<Vec<(usize, &'static Row)>, TransitionError> {
    let rows: Vec<_> = table()
        .iter()
        .enumerate()
        .filter(|(_, row)| row.from == state && row.event == event)
        .collect();
    if rows.is_empty() {
        Err(TransitionError::NotInTable { state, event })
    } else {
        Ok(rows)
    }
}

/// Moves `job` by `event`: takes the first row of the pair whose guard holds, resolves its target
/// and applies its counter actions to a copy of the job. `job` itself is never changed.
pub fn transition(job: &Job, event: Event, facts: &Facts) -> Result<Transition, TransitionError> {
    let state = job.state;
    let (row, matched) = rows_for(state, event)?
        .into_iter()
        .find(|(_, row)| row.guard.holds(job, facts))
        .ok_or(TransitionError::GuardRefused { state, event })?;
    let to = match matched.target {
        Target::Fixed(to) => to,
        Target::CurrentStep => job.counters.step.state(),
        Target::Resume if state == JobState::Escalated => job
            .counters
            .escalation
            .map(|e| e.resume)
            .ok_or(TransitionError::NoResumeState { state, event })?,
        Target::Resume => job
            .counters
            .resume
            .ok_or(TransitionError::NoResumeState { state, event })?,
    };
    let mut next = *job;
    next.state = to;
    for action in &matched.actions {
        action.apply(state, &mut next);
    }
    Ok(Transition {
        row,
        job: next,
        actions: &matched.actions,
    })
}

static TABLE: LazyLock<Vec<Row>> = LazyLock::new(build);

/// The actions of every escalation: record it, assign the approvers and post the commands.
fn escalate(cause: EscalationCause) -> [Action; 3] {
    [
        Action::Escalate(cause),
        Action::Assign(AssignReason::Escalation),
        Action::PostEscalationComment,
    ]
}

fn build() -> Vec<Row> {
    use Action as A;
    use Event as E;
    use Guard as G;
    use JobState as S;
    use Target::{CurrentStep, Fixed, Resume};

    let mut rows = Vec::new();
    let mut row = |from, event, guard, target, actions: &[Action]| {
        rows.push(Row {
            from,
            event,
            guard,
            target,
            actions: actions.to_vec(),
        });
    };
    let new_implement = [
        A::SetStep(Step::Implement),
        A::ResetAttempts,
        A::ResetGateRounds,
    ];
    let new_address = [
        A::SetStep(Step::Address),
        A::ResetAttempts,
        A::ResetGateRounds,
    ];
    let hand_over = [A::ClosePrKeepBranch, A::ReleaseIssue];

    // §6 "Failure, retry, cap and escalation": the attempt outcomes of every stage, and "any
    // stage | stage_needs_input".
    for &stage in S::STAGES {
        row(
            stage,
            E::AttemptFailed,
            G::AttemptsLeft,
            Fixed(stage),
            &[A::CountAttempt],
        );
        let exhausted = escalate(EscalationCause::AttemptsExhausted);
        let exhausted = [&[A::CountAttempt][..], &exhausted].concat();
        row(
            stage,
            E::AttemptFailed,
            G::AttemptsExhausted,
            Fixed(S::Escalated),
            &exhausted,
        );
        // "`throttled` is not an attempt: the job pauses until the harness's `resets_at`."
        row(
            stage,
            E::AttemptThrottled,
            G::Always,
            Fixed(stage),
            &[A::Pause],
        );
        // "`model_unavailable` escalates at once, without a retry."
        let unavailable = escalate(EscalationCause::ModelUnavailable);
        row(
            stage,
            E::ModelUnavailable,
            G::Always,
            Fixed(S::Escalated),
            &unavailable,
        );
        // `cancelled` is kept distinct: it ends the job only when the job was cancelled.
        row(
            stage,
            E::AttemptCancelled,
            G::JobCancelled,
            Fixed(S::Cancelled),
            &hand_over,
        );
        row(
            stage,
            E::AttemptCancelled,
            G::JobNotCancelled,
            Fixed(stage),
            &[],
        );
        let needs_input = [A::RecordResume, A::Assign(AssignReason::NeedsInput)];
        row(
            stage,
            E::StageNeedsInput,
            G::Always,
            Fixed(S::AwaitingInput),
            &needs_input,
        );
    }
    // "A step whose input is unusable (exit 2 semantics) is not retried. [...] the job escalates."
    for &step in S::STEPS {
        let unusable = escalate(EscalationCause::InputUnusable);
        row(
            step,
            E::InputUnusable,
            G::Always,
            Fixed(S::Escalated),
            &unusable,
        );
    }

    // Queue and preparation: "`queued` → `preparing` → first stage".
    row(
        S::Queued,
        E::Dispatch,
        G::Dispatchable,
        Fixed(S::Preparing),
        &[],
    );
    row(
        S::Preparing,
        E::Prepared,
        G::Always,
        Fixed(S::Analysing),
        &[A::ResetAttempts],
    );

    // Analysis.
    let publish_next = [A::PublishArtefact, A::ResetAttempts];
    row(
        S::Analysing,
        E::StageComplete,
        G::AnalysisSplit,
        Fixed(S::Reclassifying),
        &[A::PublishArtefact],
    );
    row(
        S::Analysing,
        E::StageComplete,
        G::AnalysisNoSplit,
        Fixed(S::Planning),
        &publish_next,
    );
    row(
        S::Analysing,
        E::StageComplete,
        G::KindDesign,
        Fixed(S::Designing),
        &publish_next,
    );
    row(
        S::Analysing,
        E::StageComplete,
        G::KindPlan,
        Fixed(S::Decomposing),
        &publish_next,
    );
    row(
        S::AwaitingInput,
        E::Answered,
        G::TrustedAnswer,
        Resume,
        &[A::AddAnswerInput, A::ReleaseIssue, A::ClearResume],
    );
    row(
        S::AwaitingInput,
        E::Cancel,
        G::Always,
        Fixed(S::Cancelled),
        &hand_over,
    );

    // Re-classification (#146): set the type, change the kind, comment, dispatch decompose.
    row(
        S::Reclassifying,
        E::Reclassified,
        G::Always,
        Fixed(S::Decomposing),
        &[
            A::SetIssueTypePlan,
            A::SetKindPlan,
            A::CommentReclassification,
            A::ResetAttempts,
        ],
    );

    // Design, and the commands "at an approval": `approve`, `changes`, `reject`.
    row(
        S::Designing,
        E::StageComplete,
        G::Always,
        Fixed(S::AwaitingDesignApproval),
        &[A::PublishArtefact, A::Assign(AssignReason::Design)],
    );
    let approved_next = [A::RecordApproval, A::ReleaseIssue, A::ResetAttempts];
    let changes = [A::AddCommentInput, A::ReleaseIssue, A::ResetAttempts];
    let design = S::AwaitingDesignApproval;
    row(
        design,
        E::Approved,
        G::ArtefactApproved,
        Fixed(S::Decomposing),
        &approved_next,
    );
    row(
        design,
        E::ChangesRequested,
        G::Always,
        Fixed(S::Designing),
        &changes,
    );
    row(
        design,
        E::Reject,
        G::Always,
        Fixed(S::Rejected),
        &[A::ReleaseIssue],
    );
    row(
        design,
        E::Cancel,
        G::Always,
        Fixed(S::Cancelled),
        &hand_over,
    );

    // Decompose: "A decomposition always goes to a human."
    let decomposition = S::AwaitingDecompositionApproval;
    row(
        S::Decomposing,
        E::StageComplete,
        G::Always,
        Fixed(decomposition),
        &[A::PublishArtefact, A::Assign(AssignReason::Decomposition)],
    );
    row(
        decomposition,
        E::Approved,
        G::ArtefactApproved,
        Fixed(S::FilingChildren),
        &[A::RecordApproval, A::ReleaseIssue],
    );
    row(
        decomposition,
        E::ChangesRequested,
        G::Always,
        Fixed(S::Decomposing),
        &changes,
    );
    row(
        decomposition,
        E::Reject,
        G::Always,
        Fixed(S::Rejected),
        &[A::ReleaseIssue],
    );
    row(
        decomposition,
        E::Cancel,
        G::Always,
        Fixed(S::Cancelled),
        &hand_over,
    );
    // "`filing_children` refuses to file a child under a parent whose type is neither `Plan` nor
    // `Design`, and escalates instead"; "`awaiting_children`: done when every child is done".
    row(
        S::FilingChildren,
        E::ChildrenFiled,
        G::Always,
        Fixed(S::AwaitingChildren),
        &[],
    );
    let refused = escalate(EscalationCause::ParentTypeRefused);
    row(
        S::FilingChildren,
        E::ParentTypeRefused,
        G::Always,
        Fixed(S::Escalated),
        &refused,
    );
    row(
        S::AwaitingChildren,
        E::ChildrenDone,
        G::Always,
        Fixed(S::Done),
        &[A::ReleaseIssue, A::CloseIssue],
    );

    // Plan: "`planning` → `plan_floor` → `assessing` → `deciding` → `awaiting_plan_approval` or
    // `implementing`".
    row(
        S::Planning,
        E::StageComplete,
        G::Always,
        Fixed(S::PlanFloor),
        &[A::PublishArtefact],
    );
    row(
        S::PlanFloor,
        E::FloorRecorded,
        G::Always,
        Fixed(S::Assessing),
        &[A::PostTldr, A::ResetAttempts],
    );
    row(
        S::Assessing,
        E::StageComplete,
        G::Always,
        Fixed(S::Deciding),
        &[A::PublishArtefact],
    );
    let auto = [&[A::PostAutoComment][..], &new_implement].concat();
    row(
        S::Deciding,
        E::Decided,
        G::DecisionAuto,
        Fixed(S::Implementing),
        &auto,
    );
    row(
        S::Deciding,
        E::Decided,
        G::DecisionHuman,
        Fixed(S::AwaitingPlanApproval),
        &[
            A::PostApprovalPackage,
            A::Assign(AssignReason::PlanApproval),
        ],
    );
    let plan = S::AwaitingPlanApproval;
    let approve_plan = [
        &[
            A::RecordApproval,
            A::ClearForcedPlanApproval,
            A::ReleaseIssue,
        ][..],
        &new_implement,
    ]
    .concat();
    row(
        plan,
        E::Approved,
        G::PlanApproved,
        Fixed(S::Implementing),
        &approve_plan,
    );
    row(
        plan,
        E::ChangesRequested,
        G::Always,
        Fixed(S::Planning),
        &changes,
    );
    row(
        plan,
        E::Reject,
        G::Always,
        Fixed(S::Rejected),
        &[A::ReleaseIssue],
    );
    row(plan, E::Cancel, G::Always, Fixed(S::Cancelled), &hand_over);

    // "`implementing` … `ready` | `veto` | before implement finishes"; read as before the first
    // push, since after it the PR exists and a veto is a rejection at PR approval.
    for from in [
        S::Implementing,
        S::Committing,
        S::Gating,
        S::SummarisingGates,
        S::Pushing,
    ] {
        row(
            from,
            E::Veto,
            G::NotYetPushed,
            Fixed(plan),
            &[A::CancelAttempt, A::Assign(AssignReason::PlanApproval)],
        );
    }

    // Implement, commit, gate and push: "`implementing` or `addressing` → `committing`".
    row(
        S::Implementing,
        E::StageComplete,
        G::Always,
        Fixed(S::Committing),
        &[],
    );
    row(
        S::Addressing,
        E::StageComplete,
        G::Always,
        Fixed(S::Committing),
        &[],
    );
    row(
        S::Committing,
        E::Committed,
        G::Always,
        Fixed(S::Gating),
        &[],
    );
    let footprint = [
        &[A::KeepDiffRecord][..],
        &escalate(EscalationCause::OutOfFootprint),
    ]
    .concat();
    row(
        S::Committing,
        E::OutOfFootprint,
        G::Always,
        Fixed(S::Escalated),
        &footprint,
    );
    row(
        S::Gating,
        E::GatesFailed,
        G::GateRoundsLeft,
        CurrentStep,
        &[A::CountGateRound, A::WriteGatesInput, A::ResetAttempts],
    );
    row(
        S::Gating,
        E::GatesFailed,
        G::GateRoundsLeftSummarised,
        Fixed(S::SummarisingGates),
        &[A::CountGateRound, A::ResetAttempts],
    );
    let gate_cap = [
        &[A::CountGateRound][..],
        &escalate(EscalationCause::GateCap),
    ]
    .concat();
    row(
        S::Gating,
        E::GatesFailed,
        G::GateRoundsExhausted,
        Fixed(S::Escalated),
        &gate_cap,
    );
    row(S::Gating, E::GatesPassed, G::Always, Fixed(S::Pushing), &[]);
    // "`summarising_gates` → `implementing`", back to the step that produced the change.
    row(
        S::SummarisingGates,
        E::StageComplete,
        G::Always,
        CurrentStep,
        &[A::WriteGatesInput, A::ResetAttempts],
    );
    row(
        S::Pushing,
        E::Pushed,
        G::NotYetPushed,
        Fixed(S::Reviewing),
        &[A::OpenDraftPr, A::StartReviewRound, A::ResetAttempts],
    );
    // A later push starts review round n + 1, or returns a human-requested round to `ready`.
    row(
        S::Pushing,
        E::Pushed,
        G::AgentRoundPush,
        Fixed(S::Reviewing),
        &[A::StartReviewRound, A::ResetAttempts],
    );
    row(
        S::Pushing,
        E::Pushed,
        G::HumanRoundPush,
        Fixed(S::Ready),
        &[A::ReRequestReview, A::ClearHumanRound],
    );
    // "`pushing` (behind, with conflict) → `implementing` (conflict mode) → `committing`".
    let conflict = [&[A::PrepareMerge][..], &new_implement].concat();
    row(
        S::Pushing,
        E::PushConflict,
        G::Always,
        Fixed(S::Implementing),
        &conflict,
    );

    // Review: "Only `blocker` and `major` findings continue the loop."
    let review = S::Reviewing;
    let changes_required = E::VerdictChangesRequired;
    row(
        review,
        E::VerdictPass,
        G::Always,
        Fixed(S::LandingFloor),
        &[A::PublishArtefact],
    );
    let address = [&[A::PublishArtefact][..], &new_address].concat();
    row(
        review,
        changes_required,
        G::ReviewRoundsLeft,
        Fixed(S::Addressing),
        &address,
    );
    let cap = [
        &[A::PublishArtefact][..],
        &escalate(EscalationCause::ReviewCap),
    ]
    .concat();
    row(
        review,
        changes_required,
        G::ReviewCapReached,
        Fixed(S::Escalated),
        &cap,
    );
    row(
        review,
        changes_required,
        G::NoBlockingFindings,
        Fixed(S::LandingFloor),
        &[A::PublishArtefact],
    );

    // Landing: "`landing_floor` → `landing_judging` → `landing_deciding` → `ready`".
    row(
        S::LandingFloor,
        E::FloorRecorded,
        G::Always,
        Fixed(S::LandingJudging),
        &[A::ResetAttempts],
    );
    row(
        S::LandingJudging,
        E::StageComplete,
        G::Always,
        Fixed(S::LandingDeciding),
        &[A::PublishArtefact],
    );
    row(
        S::LandingDeciding,
        E::Decided,
        G::Always,
        Fixed(S::Ready),
        &[A::MarkPrReady, A::Assign(AssignReason::Review)],
    );

    // Ready, merge and release.
    let human_round = [&[A::SetHumanRound][..], &new_address].concat();
    row(
        S::Ready,
        E::ReviewChangesRequested,
        G::Always,
        Fixed(S::Addressing),
        &human_round,
    );
    row(
        S::Ready,
        E::PrApproved,
        G::MergeReady,
        Fixed(S::Merging),
        &[A::WriteMergeReadyRecord],
    );
    row(
        S::Ready,
        E::Cancel,
        G::Always,
        Fixed(S::Cancelled),
        &hand_over,
    );
    row(
        S::Merging,
        E::Merged,
        G::MergedHeadMatches,
        Fixed(S::Releasing),
        &[],
    );
    row(S::Merging, E::HeadMoved, G::Always, Fixed(S::Ready), &[]);
    let merge_refused = escalate(EscalationCause::MergeRefused);
    row(
        S::Merging,
        E::MergeRefused,
        G::Always,
        Fixed(S::Escalated),
        &merge_refused,
    );
    row(
        S::Releasing,
        E::Released,
        G::NoExternalSteps,
        Fixed(S::Done),
        &[A::ReleaseIssue, A::CloseIssue],
    );
    row(
        S::Releasing,
        E::Released,
        G::ExternalSteps,
        Fixed(S::AwaitingExternalSteps),
        &[
            A::ReleaseIssue,
            A::PostExternalStepsComment,
            A::Assign(AssignReason::ExternalSteps),
        ],
    );
    let external = S::AwaitingExternalSteps;
    row(
        external,
        E::ExternalStepsDone,
        G::Always,
        Fixed(S::Done),
        &[A::ReleaseIssue, A::CloseIssue],
    );
    row(
        external,
        E::Cancel,
        G::Always,
        Fixed(S::Cancelled),
        &hand_over,
    );

    // The commands "in `escalated`" (§6), decision 6, N2 and N3.
    let esc = S::Escalated;
    let leave = [A::ClearEscalation, A::ReleaseIssue];
    let fresh = [A::ResetAttempts, A::ResetGateRounds];
    let steer = [&[A::ResetReviewRounds, A::AddSteerInput][..], &leave].concat();
    row(
        esc,
        E::Steer,
        G::EscalatedByReviewCap,
        Fixed(S::Addressing),
        &[&steer[..], &new_address].concat(),
    );
    row(
        esc,
        E::Steer,
        G::EscalatedOtherwise,
        Resume,
        &[&steer[..], &fresh].concat(),
    );
    let raise = [&[A::RaiseReviewCap][..], &leave, &new_address].concat();
    row(
        esc,
        E::Continue,
        G::EscalatedByReviewCap,
        Fixed(S::Addressing),
        &raise,
    );
    row(
        esc,
        E::Continue,
        G::EscalatedOtherwise,
        Resume,
        &[&leave[..], &fresh].concat(),
    );
    row(
        esc,
        E::Accept,
        G::EscalatedByReviewCap,
        Fixed(S::LandingFloor),
        &[&[A::RecordDocumentedLimits][..], &leave].concat(),
    );
    let replan = [
        &[
            A::ResetReviewRounds,
            A::ForcePlanApproval,
            A::PrToDraft,
            A::AddFindingsInput,
        ][..],
        &leave,
        &[A::ResetAttempts],
    ]
    .concat();
    row(esc, E::Replan, G::Always, Fixed(S::Planning), &replan);
    row(
        esc,
        E::Cancel,
        G::Always,
        Fixed(S::Cancelled),
        &[&[A::ClearEscalation][..], &hand_over].concat(),
    );

    rows
}

#[cfg(test)]
mod tests {
    use super::{Action, Guard, Target, table};
    use crate::job::{Counters, Escalation, EscalationCause, Facts, Job, JobKind, JobState, Step};

    /// A small deterministic generator, so the overlap check samples the same inputs every run.
    struct XorShift(u64);

    impl XorShift {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }

        fn flag(&mut self) -> bool {
            self.next() & 1 == 1
        }

        fn below(&mut self, n: u64) -> u32 {
            u32::try_from(self.next() % n).unwrap()
        }
    }

    fn sample(rng: &mut XorShift) -> (Job, Facts) {
        let causes = EscalationCause::ALL;
        let escalation = rng.below(causes.len() as u64 + 1) as usize;
        let job = Job {
            kind: JobKind::ALL[rng.below(3) as usize],
            state: JobState::Queued,
            paused: rng.flag(),
            counters: Counters {
                attempts: rng.below(4),
                gate_rounds: rng.below(4),
                review_round: rng.below(6),
                review_cap_extensions: rng.below(2),
                step: if rng.flag() {
                    Step::Implement
                } else {
                    Step::Address
                },
                human_round: rng.flag(),
                plan_approval_forced: rng.flag(),
                escalation: causes.get(escalation).map(|&cause| Escalation {
                    cause,
                    resume: JobState::Analysing,
                }),
                resume: None,
            },
        };
        let facts = Facts {
            review_max_rounds: rng.below(4) + 1,
            dispatchable: rng.flag(),
            split_proposed: rng.flag(),
            approval_matches: rng.flag(),
            base_current: rng.flag(),
            decision_auto: rng.flag(),
            gates_summary_agent: rng.flag(),
            pushed_before: rng.flag(),
            blocking_findings_open: rng.flag(),
            trusted_answer: rng.flag(),
            merge_ready: rng.flag(),
            merged_head_matches: rng.flag(),
            external_steps: rng.flag(),
            job_cancelled: rng.flag(),
        };
        (job, facts)
    }

    #[test]
    fn guards_of_one_pair_never_overlap() {
        let rows = table();
        let mut pairs: Vec<Vec<Guard>> = Vec::new();
        for (i, row) in rows.iter().enumerate() {
            let same = |other: &&super::Row| other.from == row.from && other.event == row.event;
            if rows[..i].iter().any(|r| same(&r)) {
                continue;
            }
            let guards: Vec<_> = rows.iter().filter(same).map(|r| r.guard).collect();
            if guards.len() > 1 {
                pairs.push(guards);
            }
        }
        assert!(!pairs.is_empty());
        let mut rng = XorShift(0x9e37_79b9_7f4a_7c15);
        for _ in 0..20_000 {
            let (job, facts) = sample(&mut rng);
            for guards in &pairs {
                let holding = guards.iter().filter(|g| g.holds(&job, &facts)).count();
                assert!(holding <= 1, "{guards:?} overlap for {job:?} {facts:?}");
            }
        }
    }

    #[test]
    fn rows_are_unique() {
        let rows = table();
        for (i, row) in rows.iter().enumerate() {
            let key = (row.from, row.event, row.guard);
            assert!(
                rows[i + 1..]
                    .iter()
                    .all(|r| (r.from, r.event, r.guard) != key),
                "duplicate row {key:?}"
            );
        }
    }

    /// The states a row may lead to, with its dynamic targets expanded.
    fn targets(row: &super::Row) -> Vec<JobState> {
        match row.target {
            Target::Fixed(to) => vec![to],
            Target::CurrentStep => vec![JobState::Implementing, JobState::Addressing],
            Target::Resume => JobState::STAGES.to_vec(),
        }
    }

    #[test]
    fn entering_a_stage_resets_attempts() {
        for row in table() {
            if row.from == JobState::AwaitingInput {
                // Resumes the same stage: the answer is not an attempt, and the count stands.
                continue;
            }
            for to in targets(row) {
                if to.is_stage() && to != row.from {
                    assert!(row.actions.contains(&Action::ResetAttempts), "{row:?}");
                }
            }
        }
    }

    #[test]
    fn entering_a_step_from_outside_the_gate_loop_resets_gate_rounds() {
        let gate_loop = [
            JobState::Gating,
            JobState::SummarisingGates,
            JobState::AwaitingInput,
        ];
        for row in table() {
            if gate_loop.contains(&row.from) {
                continue;
            }
            for to in targets(row) {
                if matches!(to, JobState::Implementing | JobState::Addressing) && to != row.from {
                    assert!(row.actions.contains(&Action::ResetGateRounds), "{row:?}");
                }
            }
        }
    }

    #[test]
    fn action_names_are_snake_case() {
        let mut names: Vec<_> = table()
            .iter()
            .flat_map(|row| row.actions.iter().map(|a| a.to_string()))
            .collect();
        names.sort();
        names.dedup();
        for name in &names {
            assert!(
                name.chars()
                    .all(|c| c.is_ascii_lowercase() || "_()".contains(c)),
                "{name}"
            );
        }
    }
}
