//! The runner's side of the control-plane boundary: the `PlaneLink` trait and the values it carries.
//!
//! Every type here is a plain serde value with no shared pointer or database handle, so a wire
//! protocol can map it to its own encoding. Unknown fields are ignored on read, so that older
//! readers tolerate fields added later.

use std::fmt;
use std::future::Future;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::ids::{AttemptId, DirectiveId, JobId, LeaseEpoch, RevId, RunnerId, Seq, WorkerId};
use crate::records::Bound;
use crate::work::{StageJob, StageOutcome, StepJob, StepKind, Usage};

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

/// Whether a runner takes work or only watches.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunnerMode {
    #[default]
    Worker,
    Observer,
}

/// How far a held attempt has got on the runner.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttemptPhase {
    /// Claimed, but its harness has not started yet.
    Assigned,
    /// Its harness is running.
    Started,
}

/// An attempt the runner still holds when it connects, with the lease epoch it holds it under.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HeldAttempt {
    pub attempt: AttemptId,
    pub lease_epoch: LeaseEpoch,
    pub phase: AttemptPhase,
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
    #[serde(default)]
    pub mode: RunnerMode,
    /// Optional protocol features the runner supports, by name.
    #[serde(default)]
    pub features: Vec<String>,
    /// The step kinds the runner can execute.
    #[serde(default)]
    pub step_kinds: Vec<StepKind>,
    /// The minor version of the protocol the runner speaks.
    #[serde(default)]
    pub protocol_minor: u32,
    /// The attempts the runner still holds, so the plane can keep or revoke their leases.
    #[serde(default)]
    pub held_attempts: Vec<HeldAttempt>,
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
    /// The fence every write for the attempt carries.
    pub epoch: LeaseEpoch,
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
        lease_epoch: LeaseEpoch,
    },
    Interrupt {
        attempt: AttemptId,
        lease_epoch: LeaseEpoch,
        message: Option<String>,
    },
    Deliver {
        attempt: AttemptId,
        lease_epoch: LeaseEpoch,
        message: String,
    },
    Drain,
    UpgradeRequired,
}

/// An instruction returned by `heartbeat`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Directive {
    pub id: DirectiveId,
    pub kind: DirectiveKind,
}

/// The attempt and lease epoch a write is fenced by.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fence {
    pub attempt: AttemptId,
    pub lease_epoch: LeaseEpoch,
}

/// What the runner did with a directive.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DirectiveDisposition {
    /// Applied inside the running harness session.
    AppliedInSession,
    /// Applied at the attempt's next boundary.
    AppliedAtBoundary,
    /// The attempt was cancelled.
    Cancelled,
    /// The directive no longer applies, for example to an attempt that already ended.
    NotApplicable,
    /// The runner or its harness cannot carry the directive out.
    Unsupported,
}

/// The runner's acknowledgement of a directive, replayed until acknowledged.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DirectiveReceipt {
    pub seq: Seq,
    pub directive: DirectiveId,
    /// The attempt the directive was about; `None` for runner-level directives such as `Drain`.
    pub fence: Option<Fence>,
    pub disposition: DirectiveDisposition,
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
        step: StepKind,
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
    pub lease_epoch: LeaseEpoch,
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
    pub attempt: AttemptId,
    pub lease_epoch: LeaseEpoch,
    pub kind: RecordKind,
    pub version: u32,
    pub sha256: String,
    pub bytes: Vec<u8>,
    pub bound: Vec<Bound>,
}

/// What a credential is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CredentialPurpose {
    Fetch,
    Push,
}

/// A credential request's purpose with what it binds the credential to.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "purpose", rename_all = "snake_case")]
pub enum CredentialBinding {
    /// Read access to a remote.
    Fetch { remote: String },
    /// Write access to push one branch to one commit.
    Push {
        branch: String,
        commit: RevId,
        /// The remote head the push expects; `None` when the branch does not exist yet.
        expected_remote_head: Option<RevId>,
        /// The sha256 of the bundle of changes the push sends.
        changes_bundle_sha256: String,
    },
}

/// A request for a credential, fenced by the attempt's lease.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CredentialRequest {
    pub job: JobId,
    pub attempt: AttemptId,
    pub lease_epoch: LeaseEpoch,
    pub binding: CredentialBinding,
}

impl CredentialRequest {
    /// The purpose the binding states.
    #[must_use]
    pub fn purpose(&self) -> CredentialPurpose {
        match self.binding {
            CredentialBinding::Fetch { .. } => CredentialPurpose::Fetch,
            CredentialBinding::Push { .. } => CredentialPurpose::Push,
        }
    }
}

/// A short-lived credential. Its `Debug` output never shows the token.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Credential {
    pub token: String,
    /// RFC 3339.
    pub expires_at: String,
    /// The permissions the token grants, as the code host names them.
    #[serde(default)]
    pub permissions: Vec<String>,
}

impl fmt::Debug for Credential {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Credential")
            .field("token", &"[redacted]")
            .field("expires_at", &self.expires_at)
            .field("permissions", &self.permissions)
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

    /// Acknowledge a directive.
    fn receipt(
        &self,
        receipt: DirectiveReceipt,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send {
        let _ = receipt;
        async { Ok(()) }
    }

    /// The handle that wakes the runner when a directive is waiting, so it need not wait for the
    /// next heartbeat. A remote link fires it when a directive arrives; `LocalPlane` never does.
    fn directive_notify(&self) -> Arc<tokio::sync::Notify> {
        Arc::new(tokio::sync::Notify::new())
    }

    /// Get a credential to fetch from a remote or to push the job's branch to a commit.
    fn credential(
        &self,
        req: CredentialRequest,
    ) -> impl Future<Output = Result<Credential, Self::Error>> + Send;

    /// The last event and record accepted.
    fn acked(&self) -> impl Future<Output = Result<Seq, Self::Error>> + Send;
}
