#![cfg(test)]
//! The acceptance tests of the job loop's transition table (§6).

use mintswarm_domain::job::{
    ATTEMPT_CAP, Action, AttemptOutcome, Escalation, EscalationCause, Event, Facts, GATE_ROUND_CAP,
    Guard, Job, JobKind, JobState, Row, Step, Target, TransitionError, rows_for, table, transition,
};
use std::collections::{BTreeSet, VecDeque};

fn targets(row: &Row) -> Vec<JobState> {
    row.targets()
}

const MAX_ROUNDS: u32 = 3;

/// A job and facts in `row.from` under which `row.guard` holds.
fn setup(row: &Row) -> (Job, Facts) {
    let mut job = Job::new(JobKind::Implementation);
    job.state = row.from;
    let mut facts = Facts::new(MAX_ROUNDS);
    let c = &mut job.counters;
    if row.from == JobState::AwaitingInput {
        c.resume = Some(JobState::Analysing);
    }
    if row.from == JobState::Escalated {
        c.escalation = Some(Escalation {
            cause: EscalationCause::AttemptsExhausted,
            resume: JobState::Analysing,
        });
    }
    match row.guard {
        Guard::Always => {}
        Guard::Dispatchable => facts.dispatchable = true,
        Guard::AnalysisSplit => facts.split_proposed = true,
        Guard::AnalysisNoSplit => {}
        Guard::KindDesign => job.kind = JobKind::Design,
        Guard::KindPlan => job.kind = JobKind::Plan,
        Guard::Replannable => {}
        Guard::ArtefactApproved => facts.approval_matches = true,
        Guard::PlanApproved => {
            facts.approval_matches = true;
            facts.base_current = true;
        }
        Guard::DecisionAuto => facts.decision_auto = true,
        Guard::DecisionHuman => {}
        Guard::TrustedAnswer => facts.trusted_answer = true,
        Guard::AttemptsLeft => {}
        Guard::AttemptsExhausted => c.attempts = ATTEMPT_CAP - 1,
        Guard::JobCancelled => facts.job_cancelled = true,
        Guard::JobNotCancelled => {}
        Guard::NotYetPushed => {}
        Guard::AgentRoundPush => facts.pushed_before = true,
        Guard::HumanRoundPush => {
            facts.pushed_before = true;
            c.human_round = true;
        }
        Guard::GateRoundsLeft => {}
        Guard::GateRoundsLeftSummarised => facts.gates_summary_agent = true,
        Guard::GateRoundsExhausted => c.gate_rounds = GATE_ROUND_CAP - 1,
        Guard::ReviewRoundsLeft => {
            facts.blocking_findings_open = true;
            c.review_round = 1;
        }
        Guard::ReviewCapReached => {
            facts.blocking_findings_open = true;
            c.review_round = MAX_ROUNDS;
        }
        Guard::NoBlockingFindings => {}
        Guard::MergeReady => facts.merge_ready = true,
        Guard::MergedHeadMatches => facts.merged_head_matches = true,
        Guard::MergedHeadDiffers => {}
        Guard::NoExternalSteps => {}
        Guard::ExternalSteps => facts.external_steps = true,
        Guard::EscalatedByReviewCap => {
            c.escalation = Some(Escalation {
                cause: EscalationCause::ReviewCap,
                resume: JobState::Addressing,
            });
        }
        Guard::EscalatedOtherwise => {}
    }
    (job, facts)
}

/// Applies `event`, which must be accepted.
fn step(job: &Job, event: Event, facts: &Facts) -> Job {
    match transition(job, event, facts) {
        Ok(t) => t.job,
        Err(e) => panic!("{e} (job {job:?})"),
    }
}

fn job_in(state: JobState) -> Job {
    let mut job = Job::new(JobKind::Implementation);
    job.state = state;
    job
}

#[test]
fn unknown_pairs_are_rejected() {
    let facts = Facts::new(MAX_ROUNDS);
    let mut accepted = 0;
    for &state in JobState::ALL {
        for &event in Event::ALL {
            let in_table = table().iter().any(|r| r.from == state && r.event == event);
            let job = job_in(state);
            if in_table {
                accepted += 1;
                assert!(rows_for(state, event).is_ok());
                assert_ne!(
                    transition(&job, event, &facts).err(),
                    Some(TransitionError::NotInTable { state, event })
                );
            } else {
                let refused = TransitionError::NotInTable { state, event };
                assert_eq!(rows_for(state, event).err(), Some(refused));
                assert_eq!(transition(&job, event, &facts).err(), Some(refused));
                assert_eq!(job, job_in(state));
            }
        }
    }
    assert_eq!(accepted, 145);
    for state in [JobState::Done, JobState::Rejected, JobState::Cancelled] {
        assert!(
            table().iter().all(|r| r.from != state),
            "{state} takes an event"
        );
    }
}

#[test]
fn every_row_is_exercised() {
    let mut hit = vec![false; table().len()];
    for (index, row) in table().iter().enumerate() {
        let (job, facts) = setup(row);
        let t = transition(&job, row.event, &facts).unwrap_or_else(|e| panic!("{e}: {row:?}"));
        assert_eq!(t.row, index, "{row:?}");
        assert_eq!(t.actions, &row.actions[..]);
        if let Target::Fixed(to) = row.target {
            assert_eq!(t.job.state, to);
        }
        hit[t.row] = true;
    }
    assert!(hit.iter().all(|h| *h));
}

fn reachable(from: JobState) -> BTreeSet<JobState> {
    let mut seen = BTreeSet::from([from]);
    let mut queue = VecDeque::from([from]);
    while let Some(state) = queue.pop_front() {
        for row in table().iter().filter(|r| r.from == state) {
            for to in targets(row) {
                if seen.insert(to) {
                    queue.push_back(to);
                }
            }
        }
    }
    seen
}

#[test]
fn every_state_reaches_terminal_or_human_wait() {
    for &state in JobState::ALL {
        let seen = reachable(state);
        assert!(
            seen.iter().any(|s| s.is_terminal() || s.is_human_wait()),
            "{state} reaches neither a terminal nor a human-wait state"
        );
        assert!(
            state.is_terminal() || seen.iter().any(|s| s.is_terminal()),
            "{state} never ends"
        );
    }
    let entered: BTreeSet<_> = table().iter().flat_map(targets).collect();
    for &state in JobState::ALL {
        assert_eq!(
            entered.contains(&state),
            state != JobState::Queued,
            "{state}"
        );
    }
}

// Criterion 4's "fourth gate round": the failure that ends round GATE_ROUND_CAP (the third)
// escalates, so a fourth round never starts.
#[test]
fn fourth_gate_round_escalates() {
    let facts = Facts::new(MAX_ROUNDS);
    let mut job = job_in(JobState::Gating);
    for round in 1..GATE_ROUND_CAP {
        job = step(&job, Event::GatesFailed, &facts);
        assert_eq!(
            (job.state, job.counters.gate_rounds),
            (JobState::Implementing, round)
        );
        job = step(&job, Event::StageComplete, &facts);
        job = step(&job, Event::Committed, &facts);
        assert_eq!(job.state, JobState::Gating);
    }
    job = step(&job, Event::GatesFailed, &facts);
    assert_eq!(job.state, JobState::Escalated);
    let escalation = job.counters.escalation.map(|e| (e.cause, e.resume));
    assert_eq!(
        escalation,
        Some((EscalationCause::GateCap, JobState::Implementing))
    );
    let refused = transition(&job, Event::GatesFailed, &facts).err();
    assert!(matches!(refused, Some(TransitionError::NotInTable { .. })));

    // In an address step the failed round goes back to `addressing`, through the summary too.
    let mut facts = facts;
    facts.gates_summary_agent = true;
    let mut job = job_in(JobState::Gating);
    job.counters.step = Step::Address;
    job = step(&job, Event::GatesFailed, &facts);
    assert_eq!(job.state, JobState::SummarisingGates);
    job = step(&job, Event::StageComplete, &facts);
    assert_eq!(job.state, JobState::Addressing);
}

// Criterion 4's "fourth failed attempt": the failure that ends attempt ATTEMPT_CAP (the third)
// escalates, so a fourth attempt never starts.
#[test]
fn fourth_failed_attempt_escalates() {
    let facts = Facts::new(MAX_ROUNDS);
    let failures: Vec<_> = AttemptOutcome::ALL
        .iter()
        .filter(|o| o.counts_as_attempt())
        .collect();
    assert_eq!(failures.len(), 5);
    for outcome in failures {
        let event = outcome.event().expect("a failure raises an event");
        let mut job = job_in(JobState::Implementing);
        for attempt in 1..ATTEMPT_CAP {
            job = step(&job, event, &facts);
            assert_eq!(
                (job.state, job.counters.attempts),
                (JobState::Implementing, attempt)
            );
        }
        job = step(&job, event, &facts);
        assert_eq!(job.state, JobState::Escalated, "{outcome}");
        let escalation = job.counters.escalation.map(|e| (e.cause, e.resume));
        assert_eq!(
            escalation,
            Some((EscalationCause::AttemptsExhausted, JobState::Implementing))
        );
        assert!(transition(&job, event, &facts).is_err());
    }
}

#[test]
fn model_unavailable_escalates_at_once() {
    let facts = Facts::new(MAX_ROUNDS);
    for &stage in JobState::STAGES {
        let job = job_in(stage);
        let event = AttemptOutcome::ModelUnavailable.event().expect("an event");
        let next = step(&job, event, &facts);
        assert_eq!(next.state, JobState::Escalated);
        assert_eq!(next.counters.attempts, 0);
        let cause = next.counters.escalation.map(|e| (e.cause, e.resume));
        assert_eq!(cause, Some((EscalationCause::ModelUnavailable, stage)));
    }
}

#[test]
fn throttled_is_not_an_attempt() {
    let facts = Facts::new(MAX_ROUNDS);
    let mut job = job_in(JobState::Reviewing);
    job.counters.attempts = 1;
    let event = AttemptOutcome::Throttled.event().expect("an event");
    let next = step(&job, event, &facts);
    assert_eq!(
        (next.state, next.counters.attempts),
        (JobState::Reviewing, 1)
    );
    assert!(next.paused);

    let mut queued = Job::new(JobKind::Implementation);
    queued.paused = true;
    let mut facts = facts;
    facts.dispatchable = true;
    assert_eq!(
        transition(&queued, Event::Dispatch, &facts).err(),
        Some(TransitionError::GuardRefused {
            state: JobState::Queued,
            event: Event::Dispatch
        })
    );
    queued.paused = false;
    assert_eq!(
        step(&queued, Event::Dispatch, &facts).state,
        JobState::Preparing
    );
}

/// A job at the review cap, escalated with findings open.
fn at_review_cap() -> (Job, Facts) {
    let mut facts = Facts::new(MAX_ROUNDS);
    facts.blocking_findings_open = true;
    facts.pushed_before = true;
    let mut job = job_in(JobState::Reviewing);
    job.counters.review_round = 1;
    while job.state != JobState::Escalated {
        job = step(&job, Event::VerdictChangesRequired, &facts);
        if job.state == JobState::Addressing {
            job = to_next_review(&job, &facts);
        }
    }
    assert_eq!(job.counters.review_round, MAX_ROUNDS);
    let cause = job.counters.escalation.map(|e| e.cause);
    assert_eq!(cause, Some(EscalationCause::ReviewCap));
    (job, facts)
}

/// From an implement or address step, through commit, gates and push, to the next review round.
fn to_next_review(job: &Job, facts: &Facts) -> Job {
    let mut job = step(job, Event::StageComplete, facts);
    for event in [Event::Committed, Event::GatesPassed, Event::Pushed] {
        job = step(&job, event, facts);
    }
    assert_eq!(job.state, JobState::Reviewing);
    job
}

#[test]
fn steer_resets_review_rounds() {
    let (job, facts) = at_review_cap();
    let job = step(&job, Event::Steer, &facts);
    assert_eq!(
        (job.state, job.counters.review_round),
        (JobState::Addressing, 0)
    );
    assert_eq!(job.counters.escalation, None);
    let job = to_next_review(&job, &facts);
    assert_eq!(job.counters.review_round, 1);

    // Outside the review cap, `steer` resumes the failed step with fresh counts.
    let mut job = job_in(JobState::Escalated);
    job.counters.review_round = 2;
    job.counters.attempts = ATTEMPT_CAP;
    job.counters.escalation = Some(Escalation {
        cause: EscalationCause::AttemptsExhausted,
        resume: JobState::Planning,
    });
    let job = step(&job, Event::Steer, &facts);
    assert_eq!(job.state, JobState::Planning);
    assert_eq!((job.counters.review_round, job.counters.attempts), (0, 0));
}

#[test]
fn replan_resets_review_rounds() {
    let (job, mut facts) = at_review_cap();
    let t = transition(&job, Event::Replan, &facts).expect("replan is accepted");
    assert!(t.actions.contains(&Action::PrToDraft));
    let mut job = t.job;
    assert_eq!(
        (job.state, job.counters.review_round),
        (JobState::Planning, 0)
    );
    assert!(job.counters.plan_approval_forced);
    facts.decision_auto = true;
    for event in [
        Event::StageComplete,
        Event::FloorRecorded,
        Event::StageComplete,
    ] {
        job = step(&job, event, &facts);
    }
    assert_eq!(job.state, JobState::Deciding);
    job = step(&job, Event::Decided, &facts);
    assert_eq!(job.state, JobState::AwaitingPlanApproval);
    facts.approval_matches = true;
    facts.base_current = true;
    job = step(&job, Event::Approved, &facts);
    assert_eq!(job.state, JobState::Implementing);
    assert!(!job.counters.plan_approval_forced);
    let job = to_next_review(&job, &facts);
    assert_eq!(job.counters.review_round, 1);
}

#[test]
fn continue_keeps_count_and_raises_cap_by_one() {
    let (job, facts) = at_review_cap();
    let job = step(&job, Event::Continue, &facts);
    assert_eq!(job.state, JobState::Addressing);
    assert_eq!(job.counters.review_round, MAX_ROUNDS);
    assert_eq!(job.counters.review_cap_extensions, 1);
    let job = to_next_review(&job, &facts);
    assert_eq!(job.counters.review_round, MAX_ROUNDS + 1);
    let job = step(&job, Event::VerdictChangesRequired, &facts);
    assert_eq!(job.state, JobState::Escalated);
    let cause = job.counters.escalation.map(|e| e.cause);
    assert_eq!(cause, Some(EscalationCause::ReviewCap));
}

#[test]
fn accept_goes_through_landing() {
    let (job, facts) = at_review_cap();
    let t = transition(&job, Event::Accept, &facts).expect("accept is accepted");
    assert_eq!(t.job.state, JobState::LandingFloor);
    assert!(t.actions.contains(&Action::RecordDocumentedLimits));
    let mut job = t.job;
    let path = [
        (Event::FloorRecorded, JobState::LandingJudging),
        (Event::StageComplete, JobState::LandingDeciding),
        (Event::Decided, JobState::Ready),
    ];
    for (event, state) in path {
        job = step(&job, event, &facts);
        assert_eq!(job.state, state);
    }
    let escalated = table().iter().filter(|r| r.from == JobState::Escalated);
    for row in escalated {
        assert_ne!(row.target, Target::Fixed(JobState::Ready), "{row:?}");
    }
}

#[test]
fn accept_refused_outside_review_cap() {
    let facts = Facts::new(MAX_ROUNDS);
    let others = EscalationCause::ALL
        .iter()
        .filter(|c| **c != EscalationCause::ReviewCap);
    for &cause in others {
        let mut job = job_in(JobState::Escalated);
        job.counters.escalation = Some(Escalation {
            cause,
            resume: JobState::Implementing,
        });
        assert_eq!(
            transition(&job, Event::Accept, &facts).err(),
            Some(TransitionError::GuardRefused {
                state: JobState::Escalated,
                event: Event::Accept
            }),
            "{cause}"
        );
        assert_eq!(job.state, JobState::Escalated);
    }
}

#[test]
fn replan_ends_a_human_round() {
    let mut facts = Facts::new(MAX_ROUNDS);
    facts.pushed_before = true;
    let job = job_in(JobState::Ready);
    let mut job = step(&job, Event::ReviewChangesRequested, &facts);
    assert_eq!(job.state, JobState::Addressing);
    assert!(job.counters.human_round);
    for _ in 0..ATTEMPT_CAP {
        job = step(&job, Event::AttemptFailed, &facts);
    }
    assert_eq!(job.state, JobState::Escalated);
    job = step(&job, Event::Replan, &facts);
    assert!(!job.counters.human_round);
    for event in [
        Event::StageComplete,
        Event::FloorRecorded,
        Event::StageComplete,
        Event::Decided,
    ] {
        job = step(&job, event, &facts);
    }
    assert_eq!(job.state, JobState::AwaitingPlanApproval);
    facts.approval_matches = true;
    facts.base_current = true;
    job = step(&job, Event::Approved, &facts);
    let job = to_next_review(&job, &facts);
    assert_eq!(job.counters.review_round, 1);
}

#[test]
fn replan_refused_for_design_and_plan_jobs() {
    let facts = Facts::new(MAX_ROUNDS);
    for kind in [JobKind::Design, JobKind::Plan] {
        let mut job = Job::new(kind);
        job.state = JobState::Escalated;
        job.counters.escalation = Some(Escalation {
            cause: EscalationCause::AttemptsExhausted,
            resume: JobState::Decomposing,
        });
        assert_eq!(
            transition(&job, Event::Replan, &facts).err(),
            Some(TransitionError::GuardRefused {
                state: JobState::Escalated,
                event: Event::Replan
            }),
            "{kind}"
        );
    }
}

#[test]
fn cancel_ends_every_live_state() {
    let facts = Facts::new(MAX_ROUNDS);
    for &state in JobState::ALL {
        if state.is_terminal() {
            continue;
        }
        let mut job = job_in(state);
        job.counters.escalation = Some(Escalation {
            cause: EscalationCause::GateCap,
            resume: JobState::Implementing,
        });
        let t = transition(&job, Event::Cancel, &facts).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(t.job.state, JobState::Cancelled, "{state}");
        assert!(t.actions.contains(&Action::ClosePrKeepBranch), "{state}");
    }
}

#[test]
fn merged_at_another_head_escalates() {
    let facts = Facts::new(MAX_ROUNDS);
    let job = step(&job_in(JobState::Merging), Event::Merged, &facts);
    assert_eq!(job.state, JobState::Escalated);
    let escalation = job.counters.escalation.map(|e| (e.cause, e.resume));
    assert_eq!(
        escalation,
        Some((EscalationCause::MergedHeadMismatch, JobState::Releasing))
    );
    let job = step(&job, Event::Continue, &facts);
    assert_eq!(job.state, JobState::Releasing);
}

#[test]
fn replan_refused_once_merged_at_another_head() {
    let facts = Facts::new(MAX_ROUNDS);
    let job = step(&job_in(JobState::Merging), Event::Merged, &facts);
    assert_eq!(job.state, JobState::Escalated);
    assert_eq!(
        transition(&job, Event::Replan, &facts).err(),
        Some(TransitionError::GuardRefused {
            state: JobState::Escalated,
            event: Event::Replan
        })
    );
}

#[test]
fn replan_stays_open_after_a_refused_merge() {
    let facts = Facts::new(MAX_ROUNDS);
    let job = step(&job_in(JobState::Merging), Event::MergeRefused, &facts);
    assert_eq!(job.state, JobState::Escalated);
    assert_eq!(
        job.counters.escalation.map(|e| e.cause),
        Some(EscalationCause::MergeRefused)
    );
    let job = step(&job, Event::Replan, &facts);
    assert_eq!(job.state, JobState::Planning);
}
