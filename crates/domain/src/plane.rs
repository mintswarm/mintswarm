//! The runner's side of the control-plane boundary: the `PlaneLink` trait and the values it carries.
//!
//! Every type here is a plain serde value with no shared pointer or database handle, so a wire
//! protocol can map it to its own encoding. Unknown fields are ignored on read, so that older
//! readers tolerate fields added later.

use std::fmt;
use std::future::Future;

use serde::{Deserialize, Serialize};

use crate::ids::{AttemptId, JobId, RevId, RunnerId, Seq, WorkerId};
use crate::job::{StageJob, StageOutcome, Step, StepJob, Usage};
use crate::records::Bound;

/// A harness a runner can drive.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HarnessInfo {
    /// `claude` or `codex`.
    pub id: String,
    pub version: String,
}

/// A capability probe's result, as the runner reports it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProbeSummary {
    pub id: String,
    pub passed: bool,
}

/// What a runner says about itself when it connects.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunnerInfo {
    pub runner: RunnerId,
    pub version: String,
    pub host: String,
    pub harnesses: Vec<HarnessInfo>,
    pub probes: Vec<ProbeSummary>,
    /// The nickname of the harness account the runner uses.
    pub account: Option<String>,
    pub workers: u32,
    pub repository: String,
}

/// The answer to `hello`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HelloAck {
    pub runner: RunnerId,
    /// The last event and record the other side accepted, so the outbox replays the rest.
    pub acked: Seq,
}

/// What an assignment asks the runner to execute.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "job", rename_all = "snake_case")]
pub enum Work {
    Stage(StageJob),
    Step(StepJob),
}

/// How long the runner holds an assignment without renewing it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Lease {
    /// RFC 3339.
    pub expires_at: String,
}

/// A stage or step job given to a worker, with its lease.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Assignment {
    pub work: Work,
    pub lease: Lease,
}

/// A runner's periodic report, which also renews its leases.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Heartbeat {
    pub runner: RunnerId,
    pub workers_free: u32,
    pub workers_total: u32,
    pub load: f64,
    /// RFC 3339; set while the harness account is throttled.
    pub throttled_until: Option<String>,
    pub active: Vec<AttemptId>,
}

/// What a directive asks.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DirectiveKind {
    Cancel {
        attempt: AttemptId,
    },
    Interrupt {
        attempt: AttemptId,
        message: Option<String>,
    },
    Deliver {
        attempt: AttemptId,
        message: String,
    },
    Drain,
    UpgradeRequired,
}

/// An instruction returned by `heartbeat`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Directive {
    pub kind: DirectiveKind,
}

/// Whether a step succeeded.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepStatus {
    Succeeded,
    Failed,
}

/// How an attempt ended.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum JobOutcome {
    Stage(StageOutcome),
    Step {
        step: Step,
        status: StepStatus,
        reason: String,
    },
}

/// An event about a job, replayed until acknowledged.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct JobEvent {
    pub job: JobId,
    pub seq: Seq,
    pub attempt: AttemptId,
    pub outcome: Option<JobOutcome>,
    pub usage: Option<Usage>,
    pub warnings: Vec<String>,
}

/// The kinds of trusted record, as design §12 lists them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordKind {
    Floor,
    Decision,
    Round,
    Facts,
    LandingFloor,
    LandingDecision,
    Bundle,
    Packages,
    Prefetch,
    Ctrf,
    MergeReady,
}

/// A self-describing record, bound by sha256 to its bytes and to the inputs it read.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Record {
    pub job: JobId,
    pub seq: Seq,
    pub kind: RecordKind,
    pub version: u32,
    pub sha256: String,
    pub bytes: Vec<u8>,
    pub bound: Vec<Bound>,
}

/// A credential for one push. Its `Debug` output never shows the token.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Credential {
    pub token: String,
    /// RFC 3339.
    pub expires_at: String,
}

impl fmt::Debug for Credential {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Credential")
            .field("token", &"[redacted]")
            .field("expires_at", &self.expires_at)
            .finish()
    }
}

/// The runner's link to whoever owns the job loop: `LocalPlane` standalone, a remote plane later.
pub trait PlaneLink: Send + Sync {
    type Error: std::error::Error + Send + Sync + 'static;

    /// Introduce the runner.
    fn hello(&self, info: RunnerInfo)
    -> impl Future<Output = Result<HelloAck, Self::Error>> + Send;

    /// Ask for work for a worker.
    fn claim(
        &self,
        worker: WorkerId,
    ) -> impl Future<Output = Result<Option<Assignment>, Self::Error>> + Send;

    /// Report the runner's state and renew its leases; returns what to do next.
    fn heartbeat(
        &self,
        hb: Heartbeat,
    ) -> impl Future<Output = Result<Vec<Directive>, Self::Error>> + Send;

    /// Report a job event.
    fn report(&self, ev: JobEvent) -> impl Future<Output = Result<(), Self::Error>> + Send;

    /// Hand over a record.
    fn put_record(&self, rec: Record) -> impl Future<Output = Result<(), Self::Error>> + Send;

    /// Get a credential to push the job's branch to a commit.
    fn push_credential(
        &self,
        job: JobId,
        commit: RevId,
    ) -> impl Future<Output = Result<Credential, Self::Error>> + Send;

    /// The last event and record accepted.
    fn acked(&self) -> impl Future<Output = Result<Seq, Self::Error>> + Send;
}
