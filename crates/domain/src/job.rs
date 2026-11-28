//! The job loop of the runner design, §6: a job's states, the events that move it, the counters
//! and caps that guard it, and the transition table that ties them together as data.
//!
//! A **job** is one issue's life in the factory. The table is a list of rows, each
//! (state, event, guard) → (next state, actions); [`transition`] looks a job's (state, event) pair
//! up in it, picks the row whose guard holds and returns the next job and the actions the caller
//! carries out. The domain applies the counter actions itself; every other action is a label for
//! the plane that runs the table. Facts that need I/O, such as whether an approval refers to the
//! current artefact, come in through [`Facts`].

mod table;

pub use table::{
    Action, Guard, Row, Target, Transition, TransitionError, rows_for, table, transition,
};

/// The number of failed attempts after which a stage escalates instead of running again (#3).
pub const ATTEMPT_CAP: u32 = 3;

/// The number of failed gate rounds per implement or address step after which the job escalates
/// instead of running another round (decision 5).
pub const GATE_ROUND_CAP: u32 = 3;

/// Declares a closed enum whose values have stable snake_case names, with an `ALL` list, `as_str`
/// and a `Display` that writes the name.
macro_rules! named_enum {
    (
        $(#[$meta:meta])*
        $vis:vis enum $name:ident {
            $( $(#[$vmeta:meta])* $variant:ident => $text:literal, )+
        }
    ) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
        $vis enum $name {
            $( $(#[$vmeta])* $variant, )+
        }

        impl $name {
            /// Every value, in declaration order.
            pub const ALL: &'static [Self] = &[$(Self::$variant,)+];

            /// The value's stable snake_case name, as the design spells it.
            pub const fn as_str(self) -> &'static str {
                match self {
                    $( Self::$variant => $text, )+
                }
            }
        }

        impl ::std::fmt::Display for $name {
            fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
                f.write_str(self.as_str())
            }
        }
    };
}

pub(crate) use named_enum;

named_enum! {
    /// The kind of a job, from the issue type per `tracker.kinds` (§6, "States").
    pub enum JobKind {
        /// A design issue: analyse, design, approval, then decompose.
        Design => "design",
        /// A plan issue: analyse, then decompose into children.
        Plan => "plan",
        /// An implementation issue: analyse, plan, implement, review and land.
        Implementation => "implementation",
    }
}

named_enum! {
    /// A state of the job loop (§6, "States"). `paused` is a flag on [`Job`], never a state.
    pub enum JobState {
        /// Waiting for a trusted signal, its blockers, the account's throttle and a free worker.
        Queued => "queued",
        /// Step `prepare`: pins the base commit and writes the inputs.
        Preparing => "preparing",
        /// Stage `analyse`.
        Analysing => "analysing",
        /// Human wait: a stage returned `needs_input`, and the issue is assigned.
        AwaitingInput => "awaiting_input",
        /// Re-classification of an implementation issue whose analysis proposes a split (#146).
        Reclassifying => "reclassifying",
        /// Stage `design`, for design issues only.
        Designing => "designing",
        /// Human wait: the design artefact is published and the issue assigned.
        AwaitingDesignApproval => "awaiting_design_approval",
        /// Stage `decompose`.
        Decomposing => "decomposing",
        /// Human wait: the decomposition waits for approval.
        AwaitingDecompositionApproval => "awaiting_decomposition_approval",
        /// Step `file_children`: files the children and their dependency links.
        FilingChildren => "filing_children",
        /// Deterministic wait until every child is done.
        AwaitingChildren => "awaiting_children",
        /// Stage `plan`.
        Planning => "planning",
        /// Step `bundle` of the plan floor, once per plan revision.
        PlanFloor => "plan_floor",
        /// Stage `assess`.
        Assessing => "assessing",
        /// The plan-approval decision under `pipeline.plan_approval`.
        Deciding => "deciding",
        /// Human wait: the approval package is posted and the issue assigned.
        AwaitingPlanApproval => "awaiting_plan_approval",
        /// Stage `implement`, also in conflict mode.
        Implementing => "implementing",
        /// Step `commit`: the footprint check and the runner's commit.
        Committing => "committing",
        /// Step `gates`: one gate round; the verdict always comes from here.
        Gating => "gating",
        /// Stage `gates`, opt-in under `gates.summary: agent`, after a failed round.
        SummarisingGates => "summarising_gates",
        /// Step `push`; opens the draft PR on the first push.
        Pushing => "pushing",
        /// Stage `review`, round n.
        Reviewing => "reviewing",
        /// Stage `implement` in address mode: answers the findings.
        Addressing => "addressing",
        /// Steps `landing_facts` and `bundle` of the landing floor.
        LandingFloor => "landing_floor",
        /// Stage `landing`.
        LandingJudging => "landing_judging",
        /// The landing decision.
        LandingDeciding => "landing_deciding",
        /// Human wait: the PR is ready, review is requested and the PR is assigned.
        Ready => "ready",
        /// The merge-ready record is written; waits for the watch to observe `merged`.
        Merging => "merging",
        /// Releases assignments, posts external steps if any, and closes.
        Releasing => "releasing",
        /// Human wait: the PR's `external_steps` are not empty.
        AwaitingExternalSteps => "awaiting_external_steps",
        /// Human wait, left only by a maintainer command.
        Escalated => "escalated",
        /// Terminal: the job's work landed.
        Done => "done",
        /// Terminal: rejected at an approval.
        Rejected => "rejected",
        /// Terminal: cancelled; after a hand-over the branch is kept.
        Cancelled => "cancelled",
    }
}

impl JobState {
    /// The states that run a stage agent; each takes the attempt-outcome events.
    pub const STAGES: &'static [Self] = &[
        Self::Analysing,
        Self::Designing,
        Self::Decomposing,
        Self::Planning,
        Self::Assessing,
        Self::Implementing,
        Self::SummarisingGates,
        Self::Reviewing,
        Self::Addressing,
        Self::LandingJudging,
    ];

    /// The deterministic step states; each takes `input_unusable`, which is never retried (§6).
    pub const STEPS: &'static [Self] = &[
        Self::Preparing,
        Self::Reclassifying,
        Self::FilingChildren,
        Self::PlanFloor,
        Self::Deciding,
        Self::Committing,
        Self::Gating,
        Self::Pushing,
        Self::LandingFloor,
        Self::LandingDeciding,
        Self::Releasing,
    ];

    /// The states in which the job waits for a person.
    pub const HUMAN_WAITS: &'static [Self] = &[
        Self::AwaitingInput,
        Self::AwaitingDesignApproval,
        Self::AwaitingDecompositionApproval,
        Self::AwaitingPlanApproval,
        Self::Ready,
        Self::AwaitingExternalSteps,
        Self::Escalated,
    ];

    /// Whether the job's life has ended: `done`, `rejected` or `cancelled`.
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Done | Self::Rejected | Self::Cancelled)
    }

    /// Whether the job waits for a person in this state.
    pub fn is_human_wait(self) -> bool {
        Self::HUMAN_WAITS.contains(&self)
    }

    /// Whether this state runs a stage agent.
    pub fn is_stage(self) -> bool {
        Self::STAGES.contains(&self)
    }

    /// Whether this state runs a deterministic step.
    pub fn is_step(self) -> bool {
        Self::STEPS.contains(&self)
    }
}

named_enum! {
    /// An event that moves a job (§6, "Main guards and events", and the states and commands it
    /// implies). The approval commands `approve` and `changes` arrive as `approved` and
    /// `changes_requested`.
    pub enum Event {
        /// The queue starts a queued job.
        Dispatch => "dispatch",
        /// `prepare` wrote the inputs.
        Prepared => "prepared",
        /// A stage returned a valid envelope with status `complete`.
        StageComplete => "stage_complete",
        /// A stage returned `needs_input`.
        StageNeedsInput => "stage_needs_input",
        /// A trusted author answered the question of `awaiting_input`.
        Answered => "answered",
        /// The `approve` command at an approval.
        Approved => "approved",
        /// The `changes` command at an approval.
        ChangesRequested => "changes_requested",
        /// The `reject` command at an approval.
        Reject => "reject",
        /// The `veto` command after a plan approval, before the first push.
        Veto => "veto",
        /// `reclassifying` set the issue type to `Plan` (#146).
        Reclassified => "reclassified",
        /// `file_children` filed every child and link.
        ChildrenFiled => "children_filed",
        /// `file_children` refused a parent whose type is neither `Plan` nor `Design`.
        ParentTypeRefused => "parent_type_refused",
        /// Every child of the job is done.
        ChildrenDone => "children_done",
        /// A floor (plan or landing) was recorded.
        FloorRecorded => "floor_recorded",
        /// A decision (plan or landing) was taken.
        Decided => "decided",
        /// `commit` committed the staged work.
        Committed => "committed",
        /// `commit` found a changed path outside the footprint.
        OutOfFootprint => "out_of_footprint",
        /// A gate round failed.
        GatesFailed => "gates_failed",
        /// A gate round passed under the `--verdict` conditions.
        GatesPassed => "gates_passed",
        /// `push` pushed the branch.
        Pushed => "pushed",
        /// `push` found the pushed branch behind its base, with a conflict.
        PushConflict => "push_conflict",
        /// The review verdict `pass`.
        VerdictPass => "verdict_pass",
        /// The review verdict `changes_required`.
        VerdictChangesRequired => "verdict_changes_required",
        /// A human reviewer requested changes on the ready PR.
        ReviewChangesRequested => "review_changes_requested",
        /// The watch saw the PR approved.
        PrApproved => "pr_approved",
        /// The watch saw the PR merged.
        Merged => "merged",
        /// The PR's head moved away from the recorded head before the merge.
        HeadMoved => "head_moved",
        /// The keeper refused the merge.
        MergeRefused => "merge_refused",
        /// `releasing` released the assignments.
        Released => "released",
        /// A person finished the PR's external steps.
        ExternalStepsDone => "external_steps_done",
        /// A stage attempt ended `failed`, `timed_out`, `stalled`, `invalid_output` or `lost`.
        AttemptFailed => "attempt_failed",
        /// A stage attempt ended `throttled`.
        AttemptThrottled => "attempt_throttled",
        /// A stage attempt ended `model_unavailable`.
        ModelUnavailable => "model_unavailable",
        /// A stage attempt ended `cancelled`.
        AttemptCancelled => "attempt_cancelled",
        /// A deterministic step's input is unusable (exit 2 semantics).
        InputUnusable => "input_unusable",
        /// The `steer` command in `escalated`.
        Steer => "steer",
        /// The `continue` command in `escalated`.
        Continue => "continue",
        /// The `accept` command in `escalated`.
        Accept => "accept",
        /// The `replan` command in `escalated`.
        Replan => "replan",
        /// The `cancel` command; every state but the terminal ones takes it.
        Cancel => "cancel",
    }
}

named_enum! {
    /// How one stage attempt ended (§6, "Failure, retry, cap and escalation").
    pub enum AttemptOutcome {
        /// The stage returned an envelope.
        Succeeded => "succeeded",
        /// The harness failed.
        Failed => "failed",
        /// The attempt ran past its timeout.
        TimedOut => "timed_out",
        /// The attempt stopped making progress.
        Stalled => "stalled",
        /// The attempt was cancelled.
        Cancelled => "cancelled",
        /// The harness's account is throttled until its `resets_at`.
        Throttled => "throttled",
        /// The envelope stayed invalid after the one correction.
        InvalidOutput => "invalid_output",
        /// The configured model is not available.
        ModelUnavailable => "model_unavailable",
        /// The attempt's runner was lost.
        Lost => "lost",
    }
}

impl AttemptOutcome {
    /// The event this outcome raises in the stage's state. `succeeded` raises none: the envelope
    /// raises `stage_complete` or `stage_needs_input`.
    pub const fn event(self) -> Option<Event> {
        match self {
            Self::Succeeded => None,
            Self::Failed | Self::TimedOut | Self::Stalled | Self::InvalidOutput | Self::Lost => {
                Some(Event::AttemptFailed)
            }
            Self::Throttled => Some(Event::AttemptThrottled),
            Self::ModelUnavailable => Some(Event::ModelUnavailable),
            Self::Cancelled => Some(Event::AttemptCancelled),
        }
    }

    /// Whether the outcome is counted against [`ATTEMPT_CAP`]: only the failures that retry.
    pub const fn counts_as_attempt(self) -> bool {
        matches!(
            self,
            Self::Failed | Self::TimedOut | Self::Stalled | Self::InvalidOutput | Self::Lost
        )
    }

    /// Whether the outcome is retryable in principle: every outcome but `succeeded` and
    /// `model_unavailable`. Whether the stage actually runs again is the table's call, through
    /// the guards `attempts_left` (counted failures) and `job_not_cancelled` (`cancelled`).
    pub const fn retries(self) -> bool {
        !matches!(self, Self::Succeeded | Self::ModelUnavailable)
    }
}

named_enum! {
    /// Why a job is in `escalated` (§6, "States", `escalated`).
    pub enum EscalationCause {
        /// The review cap was reached with a `blocker` or `major` finding open.
        ReviewCap => "review_cap",
        /// A stage failed [`ATTEMPT_CAP`] attempts.
        AttemptsExhausted => "attempts_exhausted",
        /// A stage's model is not available.
        ModelUnavailable => "model_unavailable",
        /// A step failed [`GATE_ROUND_CAP`] gate rounds.
        GateCap => "gate_cap",
        /// The commit changed a path outside the footprint.
        OutOfFootprint => "out_of_footprint",
        /// A deterministic step's input is unusable.
        InputUnusable => "input_unusable",
        /// The keeper refused the merge.
        MergeRefused => "merge_refused",
        /// `file_children` refused the parent's issue type.
        ParentTypeRefused => "parent_type_refused",
        /// The PR was merged at a head other than the recorded one.
        MergedHeadMismatch => "merged_head_mismatch",
    }
}

impl EscalationCause {
    /// The states that `steer` or `continue` may resume for an escalation with this cause.
    pub const fn resume_candidates(self) -> &'static [JobState] {
        match self {
            Self::ReviewCap => &[JobState::Addressing],
            Self::AttemptsExhausted | Self::ModelUnavailable => JobState::STAGES,
            Self::GateCap | Self::OutOfFootprint => &[JobState::Implementing, JobState::Addressing],
            Self::InputUnusable => JobState::STEPS,
            Self::MergeRefused => &[JobState::Ready],
            Self::ParentTypeRefused => &[JobState::FilingChildren],
            Self::MergedHeadMismatch => &[JobState::Releasing],
        }
    }

    /// The state to resume, for an escalation raised in `from` during `step`: the failed stage or
    /// step, the step that produced the change, `ready` after a refused merge, or `releasing`
    /// after a merge at another head.
    pub const fn resume_state(self, from: JobState, step: Step) -> JobState {
        match self {
            Self::ReviewCap => JobState::Addressing,
            Self::AttemptsExhausted
            | Self::ModelUnavailable
            | Self::InputUnusable
            | Self::ParentTypeRefused => from,
            Self::GateCap | Self::OutOfFootprint => step.state(),
            Self::MergeRefused => JobState::Ready,
            Self::MergedHeadMismatch => JobState::Releasing,
        }
    }
}

named_enum! {
    /// Who a row assigns the issue or PR to, and why.
    pub enum AssignReason {
        /// A stage returned `needs_input`.
        NeedsInput => "needs_input",
        /// The design waits for approval.
        Design => "design",
        /// The decomposition waits for approval.
        Decomposition => "decomposition",
        /// The plan waits for approval.
        PlanApproval => "plan_approval",
        /// The ready PR waits for review.
        Review => "review",
        /// The PR's external steps wait for a person.
        ExternalSteps => "external_steps",
        /// The job escalated; assigned to the approvers.
        Escalation => "escalation",
    }
}

/// The kind of step that produced the change under gates and review.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Step {
    /// An implement step, also in conflict mode.
    #[default]
    Implement,
    /// An address step.
    Address,
}

impl Step {
    /// The stage state that runs this step.
    pub const fn state(self) -> JobState {
        match self {
            Self::Implement => JobState::Implementing,
            Self::Address => JobState::Addressing,
        }
    }
}

/// An escalation's cause and the state that `steer` or `continue` resumes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Escalation {
    /// Why the job escalated.
    pub cause: EscalationCause,
    /// The state to resume.
    pub resume: JobState,
}

/// The counters a job's guards read, kept with the job (`jobs.counters`, §12).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Counters {
    /// Counted failed attempts of the current stage.
    pub attempts: u32,
    /// Failed gate rounds of the current implement or address step.
    pub gate_rounds: u32,
    /// The number of the current or last agent review round; 0 before the first.
    pub review_round: u32,
    /// How many times `continue` raised the review cap for this job.
    pub review_cap_extensions: u32,
    /// The step that produced the change under gates and review.
    pub step: Step,
    /// Whether the current address round was requested by a human from `ready`; it does not
    /// count against the review cap.
    pub human_round: bool,
    /// Whether the next plan decision must go to a human, whatever the mode (`replan`, N3).
    pub plan_approval_forced: bool,
    /// The open escalation, while the job is in `escalated`.
    pub escalation: Option<Escalation>,
    /// The stage that `awaiting_input` resumes.
    pub resume: Option<JobState>,
}

impl Counters {
    /// The review cap: `pipeline.review.max_rounds` plus the `continue` extensions.
    pub const fn review_cap(&self, max_rounds: u32) -> u32 {
        max_rounds + self.review_cap_extensions
    }
}

/// A job as the table sees it: its kind, state, `paused` flag and counters.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Job {
    /// The job's kind; re-classification changes it to `plan`.
    pub kind: JobKind,
    /// The job's state.
    pub state: JobState,
    /// Set by the operator or a throttle signal; a paused job starts nothing new. The caller
    /// clears it on unpause or at the throttle's `resets_at`.
    pub paused: bool,
    /// The counters the guards read.
    pub counters: Counters,
}

impl Job {
    /// A new, queued and unpaused job of `kind`.
    pub fn new(kind: JobKind) -> Self {
        Self {
            kind,
            state: JobState::Queued,
            paused: false,
            counters: Counters::default(),
        }
    }
}

/// The facts a guard needs that the domain cannot know by itself; the caller evaluates them, and
/// they are what `transitions.guard_inputs` records (§12).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Facts {
    /// `pipeline.review.max_rounds`, read from the base ref.
    pub review_max_rounds: u32,
    /// The job is trusted, no blocker is unmerged, the account is not throttled, a worker is
    /// free, and the runner has the required harness, model and probes.
    pub dispatchable: bool,
    /// The analysis proposes a split (machine-readable, #146).
    pub split_proposed: bool,
    /// The approval refers to the current artefact's sha256.
    pub approval_matches: bool,
    /// The base has not moved since the plan, or the stale-plan check passes.
    pub base_current: bool,
    /// The plan decision's `effective` is `auto`.
    pub decision_auto: bool,
    /// `gates.summary: agent`.
    pub gates_summary_agent: bool,
    /// The branch has been pushed before, so the PR exists.
    pub pushed_before: bool,
    /// A `blocker` or `major` finding is open.
    pub blocking_findings_open: bool,
    /// The answer comes from a trusted author, after the question.
    pub trusted_answer: bool,
    /// Approved by a required reviewer, no re-requested reviewer pending, mergeable state clean,
    /// and the head equals the recorded head.
    pub merge_ready: bool,
    /// The merged PR's head equals the recorded head.
    pub merged_head_matches: bool,
    /// The PR's `external_steps` are not empty.
    pub external_steps: bool,
    /// The job itself was cancelled, so a cancelled attempt is not retried.
    pub job_cancelled: bool,
}

impl Facts {
    /// Facts with every flag false and the given review cap.
    pub fn new(review_max_rounds: u32) -> Self {
        Self {
            review_max_rounds,
            ..Self::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{AssignReason, AttemptOutcome, EscalationCause, Event, JobKind, JobState, Step};
    use std::collections::HashSet;

    fn assert_names(names: &[&str]) {
        let unique: HashSet<_> = names.iter().collect();
        assert_eq!(unique.len(), names.len(), "duplicate names in {names:?}");
        for name in names {
            assert!(
                !name.is_empty()
                    && !name.starts_with('_')
                    && !name.ends_with('_')
                    && name.chars().all(|c| c.is_ascii_lowercase() || c == '_'),
                "{name} is not snake_case"
            );
        }
    }

    #[test]
    fn names_are_unique_and_snake_case() {
        assert_names(&JobKind::ALL.iter().map(|v| v.as_str()).collect::<Vec<_>>());
        assert_names(&JobState::ALL.iter().map(|v| v.as_str()).collect::<Vec<_>>());
        assert_names(&Event::ALL.iter().map(|v| v.as_str()).collect::<Vec<_>>());
        assert_names(
            &AttemptOutcome::ALL
                .iter()
                .map(|v| v.as_str())
                .collect::<Vec<_>>(),
        );
        assert_names(
            &EscalationCause::ALL
                .iter()
                .map(|v| v.as_str())
                .collect::<Vec<_>>(),
        );
        assert_names(
            &AssignReason::ALL
                .iter()
                .map(|v| v.as_str())
                .collect::<Vec<_>>(),
        );
    }

    #[test]
    fn states_are_those_of_the_design() {
        let names: Vec<_> = JobState::ALL.iter().map(|s| s.to_string()).collect();
        assert_eq!(
            names,
            [
                "queued",
                "preparing",
                "analysing",
                "awaiting_input",
                "reclassifying",
                "designing",
                "awaiting_design_approval",
                "decomposing",
                "awaiting_decomposition_approval",
                "filing_children",
                "awaiting_children",
                "planning",
                "plan_floor",
                "assessing",
                "deciding",
                "awaiting_plan_approval",
                "implementing",
                "committing",
                "gating",
                "summarising_gates",
                "pushing",
                "reviewing",
                "addressing",
                "landing_floor",
                "landing_judging",
                "landing_deciding",
                "ready",
                "merging",
                "releasing",
                "awaiting_external_steps",
                "escalated",
                "done",
                "rejected",
                "cancelled",
            ]
        );
    }

    #[test]
    fn state_classes_are_disjoint() {
        for state in JobState::ALL {
            let classes = [
                state.is_terminal(),
                state.is_human_wait(),
                state.is_stage(),
                state.is_step(),
            ];
            assert!(classes.iter().filter(|c| **c).count() <= 1, "{state}");
        }
    }

    #[test]
    fn resume_state_is_a_candidate() {
        for cause in EscalationCause::ALL {
            let raised_in: &[JobState] = match cause {
                EscalationCause::ReviewCap => &[JobState::Reviewing],
                EscalationCause::AttemptsExhausted | EscalationCause::ModelUnavailable => {
                    JobState::STAGES
                }
                EscalationCause::GateCap => &[JobState::Gating],
                EscalationCause::OutOfFootprint => &[JobState::Committing],
                EscalationCause::InputUnusable => JobState::STEPS,
                EscalationCause::MergeRefused | EscalationCause::MergedHeadMismatch => {
                    &[JobState::Merging]
                }
                EscalationCause::ParentTypeRefused => &[JobState::FilingChildren],
            };
            for from in raised_in {
                for step in [Step::Implement, Step::Address] {
                    let resume = cause.resume_state(*from, step);
                    assert!(
                        cause.resume_candidates().contains(&resume),
                        "{cause} {from}"
                    );
                }
            }
        }
    }

    #[test]
    fn outcomes_follow_the_retry_rules() {
        for outcome in AttemptOutcome::ALL {
            assert_eq!(
                outcome.counts_as_attempt(),
                outcome.event() == Some(Event::AttemptFailed),
                "{outcome}"
            );
        }
        assert!(!AttemptOutcome::Throttled.counts_as_attempt());
        assert!(AttemptOutcome::Throttled.retries());
        assert!(!AttemptOutcome::ModelUnavailable.retries());
        assert_eq!(AttemptOutcome::Succeeded.event(), None);
    }
}
