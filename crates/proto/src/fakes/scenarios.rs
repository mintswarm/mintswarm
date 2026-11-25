//! The runner protocol's conformance scenarios, as data (design §13).
//!
//! One list drives the fake runner, the fake plane, the conformance suite and
//! the end-to-end run. A scenario is a name and an ordered list of abstract
//! steps: actions one side takes, faults injected between them, and
//! expectations the other side must meet. Each interpreter maps the steps onto
//! its own component; the doc comments here are the contract it follows.
//!
//! Attempts are symbolic labels, not ids: an interpreter binds each label to a
//! real attempt when it first sees it. Sequence numbers count every outbox
//! item, `attempt_started` included, from 1 on a fresh outbox.

/// A symbolic attempt label, bound to a real attempt by each interpreter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Attempt(pub &'static str);

impl Attempt {
    /// The first attempt of a scenario.
    pub const A: Self = Self("A");
    /// A second attempt of the same job.
    pub const B: Self = Self("B");
}

/// A step of an attempt's work at which a scenario interrupts it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkStep {
    /// The runner commits the agent's changes.
    Commit,
    /// The runner runs the gates.
    Gates,
    /// The runner pushes the branch.
    Push,
}

/// The terminal outcome of an attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// The attempt was cancelled by the plane.
    Cancelled,
    /// The attempt failed.
    Failed,
    /// The attempt lost its lease.
    Lost,
}

/// A named protocol scenario.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Scenario {
    /// The unique name, in `snake_case`.
    pub name: &'static str,
    /// What the scenario checks, and the design section it encodes.
    pub summary: &'static str,
    /// The steps, in the order they happen.
    pub steps: &'static [Step],
}

/// One step of a scenario.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// Something one side does.
    Action(Action),
    /// A fault injected between actions.
    Fault(Fault),
    /// What must hold once the steps before it have happened.
    Expect(Expectation),
}

/// Something the runner, the plane or the clock does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// The runner opens the stream and sends `Hello` with its outbox state.
    RunnerConnects,
    /// A runner below the plane's minimum version opens the stream.
    OutdatedRunnerConnects,
    /// The runner appends `count` events to its outbox and sends them.
    RunnerEmitsEvents {
        /// How many events.
        count: u64,
    },
    /// The plane assigns `attempt` to the runner under lease `epoch`.
    PlaneSendsAssignment {
        /// The attempt assigned.
        attempt: Attempt,
        /// The lease epoch.
        epoch: u64,
    },
    /// The plane assigns `attempt` for the same job while `running` still runs.
    PlaneSendsSecondAttempt {
        /// The new attempt.
        attempt: Attempt,
        /// The attempt already running.
        running: Attempt,
        /// The lease epoch of the new attempt.
        epoch: u64,
    },
    /// The runner begins `step` of `attempt`.
    RunnerStartsStep {
        /// The attempt.
        attempt: Attempt,
        /// The step begun.
        step: WorkStep,
    },
    /// The runner reports progress for `attempt`, stamped with lease `epoch`.
    RunnerReportsProgress {
        /// The attempt.
        attempt: Attempt,
        /// The epoch the event carries.
        epoch: u64,
    },
    /// The runner uploads a blob for `attempt`, without committing it yet.
    RunnerUploadsBlob {
        /// The attempt.
        attempt: Attempt,
    },
    /// The runner sends a `RecordPut` for `attempt` that references its blobs.
    RunnerSendsRecordPut {
        /// The attempt.
        attempt: Attempt,
    },
    /// The plane revokes the lease of `attempt`.
    PlaneRevokesLease {
        /// The attempt.
        attempt: Attempt,
    },
    /// The plane grants `attempt` again under the higher lease `epoch`.
    PlaneRegrantsLease {
        /// The attempt.
        attempt: Attempt,
        /// The new epoch.
        epoch: u64,
    },
    /// The plane sends a `Cancel` directive for `attempt`.
    PlaneIssuesCancel {
        /// The attempt.
        attempt: Attempt,
    },
    /// The plane grants a push credential for `attempt`.
    PlaneGrantsPushCredential {
        /// The attempt.
        attempt: Attempt,
        /// Seconds until the credential expires.
        expires_in_seconds: u64,
    },
    /// The clock advances on both sides.
    TimePasses {
        /// How many seconds.
        seconds: u64,
    },
}

/// A fault injected between actions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fault {
    /// The connection closes on both sides.
    ConnectionDrops,
    /// The runner's side of the connection dies while the plane's stays open.
    ConnectionGoesHalfOpen,
    /// The runner process restarts and keeps its outbox and state.
    RunnerRestarts,
    /// The runner restarts with its state lost, so it starts a new `outbox_epoch`.
    RunnerLosesState,
    /// The plane process restarts, and the stream drops.
    PlaneRestarts,
    /// The plane drops every blob not yet referenced by a committed record.
    PlaneLosesUncommittedBlobs,
    /// The plane sends no `Ack` until the stream is replaced.
    PlaneWithholdsAcks,
    /// The runner sends its next batch without the batch's first item.
    RunnerSkipsSeq,
    /// The forge rejects the next push for authentication.
    PushRejectedForAuthentication,
}

/// What must hold once the steps before it have happened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Expectation {
    /// The plane accepts `Hello` and reports `acked_seq` as acknowledged.
    HelloAccepted {
        /// The highest seq the plane holds.
        acked_seq: u64,
    },
    /// The plane rejects `Hello` with `upgrade_required`.
    HelloRejectedUpgradeRequired,
    /// The runner stops reconnecting until it is upgraded.
    RunnerStopsReconnecting,
    /// The plane closes the older stream of the runner in favour of the new one.
    OldStreamSuperseded,
    /// The plane acknowledges every outbox item up to `seq`.
    Acked {
        /// The acknowledged seq.
        seq: u64,
    },
    /// The runner resends its outbox from `seq` onwards.
    RunnerResendsFrom {
        /// The first seq resent.
        seq: u64,
    },
    /// The plane holds exactly `count` events, each stored once.
    PlaneStoresEventsOnce {
        /// How many events.
        count: u64,
    },
    /// The plane asks the runner to resend from `from`.
    PlaneRequestsResend {
        /// The first missing seq.
        from: u64,
    },
    /// The plane answers the record at `seq` with `need_blobs`.
    PlaneAsksForBlobs {
        /// The seq of the record.
        seq: u64,
    },
    /// The runner uploads again the blobs of `attempt` the plane asked for,
    /// then resends the record that references them.
    RunnerReuploadsBlobs {
        /// The attempt.
        attempt: Attempt,
    },
    /// The runner reports `attempt_started` for `attempt`.
    AttemptStarted {
        /// The attempt.
        attempt: Attempt,
    },
    /// The runner ignores a repeated assignment of `attempt`.
    RunnerIgnoresDuplicate {
        /// The attempt.
        attempt: Attempt,
    },
    /// The runner refuses `attempt` with `start_failed{duplicate}`.
    StartFailedDuplicate {
        /// The refused attempt.
        attempt: Attempt,
    },
    /// The lease of `attempt` stays held at `epoch`, and the attempt runs on.
    LeaseKept {
        /// The attempt.
        attempt: Attempt,
        /// The unchanged epoch.
        epoch: u64,
    },
    /// The runner stops the work of `attempt`.
    RunnerAbortsAttempt {
        /// The attempt.
        attempt: Attempt,
    },
    /// The plane records `outcome` for `attempt` at `epoch`.
    Outcome {
        /// The attempt.
        attempt: Attempt,
        /// The epoch of the outcome.
        epoch: u64,
        /// The outcome.
        outcome: Outcome,
    },
    /// The outcome of `attempt` carries the head commit it reached.
    OutcomeCarriesHead {
        /// The attempt.
        attempt: Attempt,
    },
    /// The plane acknowledges but discards events of `attempt` stamped `epoch`.
    EventFenced {
        /// The attempt.
        attempt: Attempt,
        /// The stale epoch.
        epoch: u64,
    },
    /// The plane marks `attempt` at `epoch` lost.
    PlaneMarksLost {
        /// The attempt.
        attempt: Attempt,
        /// The epoch.
        epoch: u64,
    },
    /// The runner asks the plane for a fresh push credential for `attempt`.
    RunnerRequestsPushCredential {
        /// The attempt.
        attempt: Attempt,
    },
    /// The runner refreshes its access token on the open stream.
    RunnerReauthenticates,
    /// The stream is not closed.
    StreamStaysOpen,
    /// The runner acts on a directive for `attempt` without waiting for a heartbeat.
    DirectiveActedOnBeforeHeartbeat {
        /// The attempt.
        attempt: Attempt,
    },
}

use Action::*;
use Attempt as At;
use Expectation as E;
use Fault::*;
use Step::{Action as Do, Expect as Then, Fault as Inject};

const A: At = At::A;
const B: At = At::B;
const CONNECT: Step = Do(RunnerConnects);
const ASSIGN_A1: Step = Do(PlaneSendsAssignment {
    attempt: A,
    epoch: 1,
});
const A_STARTED: Step = Then(E::AttemptStarted { attempt: A });

/// Every protocol scenario, in a fixed order.
pub static SCENARIOS: &[Scenario] = &[
    Scenario {
        name: "reconnect_and_replay",
        summary: "§7.4: a restarted runner replays its unacknowledged events and the plane stores each once",
        steps: &[
            CONNECT,
            Do(RunnerEmitsEvents { count: 5 }),
            Then(E::Acked { seq: 5 }),
            Inject(PlaneWithholdsAcks),
            Do(RunnerEmitsEvents { count: 3 }),
            Inject(RunnerRestarts),
            CONNECT,
            Then(E::HelloAccepted { acked_seq: 5 }),
            Then(E::RunnerResendsFrom { seq: 6 }),
            Then(E::PlaneStoresEventsOnce { count: 8 }),
            Then(E::Acked { seq: 8 }),
        ],
    },
    Scenario {
        name: "lease_lost_mid_step",
        summary: "§7.4: a lease revoked during the gates aborts the attempt as lost and fences its events",
        steps: &[
            CONNECT,
            ASSIGN_A1,
            A_STARTED,
            Do(RunnerStartsStep {
                attempt: A,
                step: WorkStep::Gates,
            }),
            Do(PlaneRevokesLease { attempt: A }),
            Then(E::RunnerAbortsAttempt { attempt: A }),
            Then(E::Outcome {
                attempt: A,
                epoch: 1,
                outcome: Outcome::Lost,
            }),
            Do(RunnerReportsProgress {
                attempt: A,
                epoch: 1,
            }),
            Then(E::EventFenced {
                attempt: A,
                epoch: 1,
            }),
        ],
    },
    Scenario {
        name: "duplicate_assignment",
        summary: "§7.4: a repeated assignment is ignored and a second attempt of a running job is refused",
        steps: &[
            CONNECT,
            ASSIGN_A1,
            A_STARTED,
            ASSIGN_A1,
            Then(E::RunnerIgnoresDuplicate { attempt: A }),
            Do(PlaneSendsSecondAttempt {
                attempt: B,
                running: A,
                epoch: 1,
            }),
            Then(E::StartFailedDuplicate { attempt: B }),
        ],
    },
    Scenario {
        name: "cancel_during_commit",
        summary: "§7.4: a cancel during a commit ends the attempt cancelled with the head it reached",
        steps: &[
            CONNECT,
            ASSIGN_A1,
            A_STARTED,
            Do(RunnerStartsStep {
                attempt: A,
                step: WorkStep::Commit,
            }),
            Do(PlaneIssuesCancel { attempt: A }),
            Then(E::Outcome {
                attempt: A,
                epoch: 1,
                outcome: Outcome::Cancelled,
            }),
            Then(E::OutcomeCarriesHead { attempt: A }),
        ],
    },
    Scenario {
        name: "push_credential_expired",
        summary: "§7.4, §7.5: a credential near expiry is refreshed, and a push rejected twice for authentication fails the attempt",
        steps: &[
            CONNECT,
            ASSIGN_A1,
            A_STARTED,
            Do(RunnerStartsStep {
                attempt: A,
                step: WorkStep::Push,
            }),
            Do(PlaneGrantsPushCredential {
                attempt: A,
                expires_in_seconds: 360,
            }),
            Do(TimePasses { seconds: 120 }),
            Then(E::RunnerRequestsPushCredential { attempt: A }),
            Do(PlaneGrantsPushCredential {
                attempt: A,
                expires_in_seconds: 3600,
            }),
            Inject(PushRejectedForAuthentication),
            Then(E::RunnerRequestsPushCredential { attempt: A }),
            Do(PlaneGrantsPushCredential {
                attempt: A,
                expires_in_seconds: 3600,
            }),
            Inject(PushRejectedForAuthentication),
            Then(E::Outcome {
                attempt: A,
                epoch: 1,
                outcome: Outcome::Failed,
            }),
        ],
    },
    Scenario {
        name: "superseded_stream",
        summary: "§6.4: a new stream from the same runner supersedes a half-open one",
        steps: &[
            CONNECT,
            Inject(ConnectionGoesHalfOpen),
            CONNECT,
            Then(E::OldStreamSuperseded),
            Then(E::HelloAccepted { acked_seq: 0 }),
        ],
    },
    Scenario {
        name: "plane_restart_within_grace",
        summary: "§7.3: a runner back within the 120 s grace after a plane restart keeps its lease",
        steps: &[
            CONNECT,
            ASSIGN_A1,
            A_STARTED,
            Inject(PlaneRestarts),
            Do(TimePasses { seconds: 90 }),
            CONNECT,
            Then(E::LeaseKept {
                attempt: A,
                epoch: 1,
            }),
        ],
    },
    Scenario {
        name: "plane_restart_beyond_grace",
        summary: "§7.3, §6.3: a runner back after the 120 s grace finds its lease re-granted and aborts as lost",
        steps: &[
            CONNECT,
            ASSIGN_A1,
            A_STARTED,
            Inject(PlaneRestarts),
            Do(TimePasses { seconds: 150 }),
            Do(PlaneRegrantsLease {
                attempt: A,
                epoch: 2,
            }),
            CONNECT,
            Then(E::RunnerAbortsAttempt { attempt: A }),
            Then(E::Outcome {
                attempt: A,
                epoch: 1,
                outcome: Outcome::Lost,
            }),
            Do(RunnerReportsProgress {
                attempt: A,
                epoch: 1,
            }),
            Then(E::EventFenced {
                attempt: A,
                epoch: 1,
            }),
        ],
    },
    Scenario {
        name: "stale_event_after_regrant",
        summary: "§7.4: an event stamped with an old epoch after a re-grant is acknowledged and fenced",
        steps: &[
            CONNECT,
            ASSIGN_A1,
            A_STARTED,
            Do(PlaneRegrantsLease {
                attempt: A,
                epoch: 2,
            }),
            Do(RunnerReportsProgress {
                attempt: A,
                epoch: 1,
            }),
            Then(E::EventFenced {
                attempt: A,
                epoch: 1,
            }),
            Then(E::Acked { seq: 2 }),
        ],
    },
    Scenario {
        name: "need_blobs",
        summary: "§8.2: a record whose blobs the plane lost is answered with need_blobs, and acknowledged once the runner uploads them again",
        steps: &[
            CONNECT,
            ASSIGN_A1,
            A_STARTED,
            Do(RunnerUploadsBlob { attempt: A }),
            Inject(PlaneLosesUncommittedBlobs),
            Do(RunnerSendsRecordPut { attempt: A }),
            Then(E::PlaneAsksForBlobs { seq: 2 }),
            Then(E::RunnerReuploadsBlobs { attempt: A }),
            Then(E::Acked { seq: 2 }),
        ],
    },
    Scenario {
        name: "seq_gap",
        summary: "§8.3: a gap in seq makes the plane ask for a resend from the first missing item",
        steps: &[
            CONNECT,
            Do(RunnerEmitsEvents { count: 3 }),
            Then(E::Acked { seq: 3 }),
            Inject(RunnerSkipsSeq),
            Do(RunnerEmitsEvents { count: 2 }),
            Then(E::PlaneRequestsResend { from: 4 }),
            Then(E::RunnerResendsFrom { seq: 4 }),
            Then(E::Acked { seq: 5 }),
        ],
    },
    Scenario {
        name: "token_refresh",
        summary: "§9.4: the runner refreshes its access token before expiry without closing the stream",
        steps: &[
            CONNECT,
            Do(TimePasses { seconds: 2460 }),
            Then(E::RunnerReauthenticates),
            Then(E::StreamStaysOpen),
        ],
    },
    Scenario {
        name: "upgrade_required",
        summary: "§10, §6.2: a runner below the minimum version is refused with upgrade_required and stops reconnecting",
        steps: &[
            Do(OutdatedRunnerConnects),
            Then(E::HelloRejectedUpgradeRequired),
            Then(E::RunnerStopsReconnecting),
        ],
    },
    Scenario {
        name: "new_outbox_epoch",
        summary: "§8.3, §6.3: a runner back with a new outbox_epoch has its earlier attempts marked lost",
        steps: &[
            CONNECT,
            ASSIGN_A1,
            A_STARTED,
            Inject(RunnerLosesState),
            CONNECT,
            Then(E::PlaneMarksLost {
                attempt: A,
                epoch: 1,
            }),
            Then(E::HelloAccepted { acked_seq: 0 }),
        ],
    },
    Scenario {
        name: "directive_latency",
        summary: "§13, §7.6: a cancel issued while the runner replays and uploads is acted on before the next heartbeat",
        steps: &[
            CONNECT,
            ASSIGN_A1,
            A_STARTED,
            Inject(ConnectionDrops),
            Do(RunnerEmitsEvents { count: 1000 }),
            CONNECT,
            Inject(PlaneWithholdsAcks),
            Do(RunnerUploadsBlob { attempt: A }),
            Do(PlaneIssuesCancel { attempt: A }),
            Then(E::DirectiveActedOnBeforeHeartbeat { attempt: A }),
            Then(E::Outcome {
                attempt: A,
                epoch: 1,
                outcome: Outcome::Cancelled,
            }),
        ],
    },
];
