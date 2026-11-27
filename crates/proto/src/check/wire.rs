//! The wire checks: one [`Receive`] impl for each top-level `runner/v1`
//! message that a runner or the plane receives, stopping at the first
//! violation. A field these checks do not name is free.

use super::{At, Receive, ReceiveError, Rule, full_name, stage};
use crate::runner::v1::{
    AttemptStartFailedReason, AuthenticateRequest, AuthenticateResponse, ClaimRefusalReason,
    ConnectRequest, ConnectResponse, Directive, DirectiveDisposition, GateResult, GetBlobRequest,
    GetBlobResponse, GoAwayReason, GoodbyeReason, Harness, HeldAttempt, Hello, HelloAck,
    HelloRejectionStatus, PlaneAttemptState, ProbeResult, PutBlobRequest, PutBlobResponse,
    RegisterRequest, RegisterResponse, RunnerEvent, RunnerMode, RunnerState, StatBlobsRequest,
    StatBlobsResponse, StepKind, claim_result, connect_request, connect_response, directive,
    get_blob_response, hello_ack, lease_state, probe_result, progress, put_blob_request,
    runner_event,
};

/// Implements [`Receive`] for `$message` through `$check`, which checks a
/// borrowed message from the top.
macro_rules! receive {
    ($($message:ty => $check:ident;)*) => {$(
        impl Receive for $message {
            type Error = ReceiveError;

            fn receive(self) -> Result<Self, ReceiveError> {
                let name = full_name::<Self>();
                $check(&At::root(&name), &self)?;
                Ok(self)
            }
        }
    )*};
}

receive! {
    RegisterRequest => register_request;
    AuthenticateRequest => authenticate_request;
    StatBlobsRequest => stat_blobs_request;
    PutBlobRequest => put_blob_request;
    GetBlobRequest => get_blob_request;
    ConnectRequest => connect_request;
    RegisterResponse => register_response;
    AuthenticateResponse => authenticate_response;
    StatBlobsResponse => stat_blobs_response;
    PutBlobResponse => put_blob_response;
    GetBlobResponse => get_blob_response;
    ConnectResponse => connect_response;
}

type Checked = Result<(), ReceiveError>;

// Received by the plane.

fn register_request(at: &At<'_>, m: &RegisterRequest) -> Checked {
    at.secret("registration_token", &m.registration_token)?;
    at.non_empty("public_key", &m.public_key)?;
    let repository = at.present("repository", &m.repository)?;
    at.child("repository").text("remote", &repository.remote)?;
    at.text("runner_version", &m.runner_version)
}

fn authenticate_request(at: &At<'_>, m: &AuthenticateRequest) -> Checked {
    at.text("client_assertion_type", &m.client_assertion_type)?;
    at.secret("client_assertion", &m.client_assertion)
}

fn stat_blobs_request(at: &At<'_>, m: &StatBlobsRequest) -> Checked {
    at.non_empty("sha256", &m.sha256)?;
    at.sha256_each("sha256", &m.sha256)
}

fn put_blob_request(at: &At<'_>, m: &PutBlobRequest) -> Checked {
    match at.present("part", &m.part)? {
        put_blob_request::Part::Header(header) => {
            let at = at.child("part.header");
            at.sha256("sha256", &header.sha256)?;
            at.id("attempt_id", &header.attempt_id)?;
            at.non_zero("lease_epoch", header.lease_epoch)
        }
        put_blob_request::Part::Chunk(_) => Ok(()),
    }
}

fn get_blob_request(at: &At<'_>, m: &GetBlobRequest) -> Checked {
    at.sha256("sha256", &m.sha256)?;
    at.id("attempt_id", &m.attempt_id)?;
    at.non_zero("lease_epoch", m.lease_epoch)
}

fn connect_request(at: &At<'_>, m: &ConnectRequest) -> Checked {
    use connect_request::Body;
    let Some(body) = &m.body else {
        return Err(at.fail("body", Rule::NoBody));
    };
    match body {
        Body::Hello(hello) => self::hello(&at.child("body.hello"), hello),
        Body::Claim(claim) => {
            let at = at.child("body.claim");
            at.id("request_id", &claim.request_id)?;
            at.id("worker_id", &claim.worker_id)
        }
        Body::CancelClaim(cancel) => at
            .child("body.cancel_claim")
            .id("request_id", &cancel.request_id),
        Body::Heartbeat(heartbeat) => {
            let at = at.child("body.heartbeat");
            at.required::<RunnerState>("state", heartbeat.state)?;
            held_attempts(&at, "held", &heartbeat.held)
        }
        Body::Events(batch) => {
            let at = at.child("body.events");
            at.non_empty("events", &batch.events)?;
            for (index, event) in batch.events.iter().enumerate() {
                runner_event(&at.item("events", index), event)?;
            }
            Ok(())
        }
        Body::Reauthenticate(reauthenticate) => at
            .child("body.reauthenticate")
            .secret("access_token", &reauthenticate.access_token),
        Body::Capabilities(update) => {
            let at = at.child("body.capabilities");
            probe_results(&at, &update.capabilities)?;
            harnesses(&at, &update.harnesses)
        }
        Body::Metrics(metrics) => {
            let at = at.child("body.metrics");
            at.present("observed_at", &metrics.observed_at)?;
            at.present("load", &metrics.load)?;
            Ok(())
        }
        Body::Goodbye(goodbye) => {
            at.child("body.goodbye")
                .required::<GoodbyeReason>("reason", goodbye.reason)?;
            Ok(())
        }
    }
}

fn hello(at: &At<'_>, m: &Hello) -> Checked {
    at.text("runner_version", &m.runner_version)?;
    at.id("outbox_epoch", &m.outbox_epoch)?;
    // A step kind this build does not know is kept: the plane never
    // dispatches it.
    for (index, &kind) in m.step_kinds.iter().enumerate() {
        if kind == StepKind::Unspecified as i32 {
            return Err(at.item("step_kinds", index).fail("", Rule::Unspecified));
        }
    }
    harnesses(at, &m.harnesses)?;
    probe_results(at, &m.capabilities)?;
    let repository = at.present("repository", &m.repository)?;
    at.child("repository").text("remote", &repository.remote)?;
    at.required::<RunnerMode>("mode", m.mode)?;
    held_attempts(at, "held_attempts", &m.held_attempts)
}

fn harnesses(at: &At<'_>, harnesses: &[Harness]) -> Checked {
    for (index, harness) in harnesses.iter().enumerate() {
        at.item("harnesses", index).id("id", &harness.id)?;
    }
    Ok(())
}

fn probe_results(at: &At<'_>, results: &[ProbeResult]) -> Checked {
    for (index, result) in results.iter().enumerate() {
        probe_result(&at.item("capabilities", index), result)?;
    }
    Ok(())
}

fn probe_result(at: &At<'_>, m: &ProbeResult) -> Checked {
    at.id("id", &m.id)?;
    at.present("observed_at", &m.observed_at)?;
    match at.present("result", &m.result)? {
        probe_result::Result::Unknown(_) => Ok(()),
        probe_result::Result::Known(known) => {
            let at = at.child("result.known");
            for (key, fact) in &known.facts {
                at.key("facts", key).present("value", &fact.value)?;
            }
            Ok(())
        }
    }
}

fn held_attempts(at: &At<'_>, name: &str, held: &[HeldAttempt]) -> Checked {
    for (index, attempt) in held.iter().enumerate() {
        let at = at.item(name, index);
        at.id("attempt_id", &attempt.attempt_id)?;
        at.non_zero("lease_epoch", attempt.lease_epoch)?;
    }
    Ok(())
}

fn runner_event(at: &At<'_>, m: &RunnerEvent) -> Checked {
    use runner_event::Body;
    at.non_zero("seq", m.seq)?;
    at.present("at", &m.at)?;
    let body = at.present("body", &m.body)?;
    match body {
        Body::Record(_) | Body::Warning(_) | Body::Receipt(_) => runner_fence(at, m)?,
        _ => {
            at.id("job_id", &m.job_id)?;
            at.id("attempt_id", &m.attempt_id)?;
            at.non_zero("lease_epoch", m.lease_epoch)?;
        }
    }
    match body {
        Body::Started(_) | Body::Usage(_) => Ok(()),
        Body::StartFailed(failed) => {
            at.child("body.start_failed")
                .required::<AttemptStartFailedReason>("reason", failed.reason)?;
            Ok(())
        }
        Body::Outcome(outcome) => {
            // An unknown kind is accepted, and read as failed (design §10).
            if outcome.kind == 0 {
                return Err(at.child("body.outcome").fail("kind", Rule::Unspecified));
            }
            Ok(())
        }
        Body::Progress(progress) => {
            let at = at.child("body.progress");
            match at.present("event", &progress.event)? {
                progress::Event::GateFinished(finished) => {
                    at.child("event.gate_finished")
                        .required::<GateResult>("result", finished.result)?;
                    Ok(())
                }
                _ => Ok(()),
            }
        }
        Body::Warning(warning) => at.child("body.warning").text("message", &warning.message),
        Body::Record(record) => {
            let at = at.child("body.record");
            at.text("kind", &record.kind)?;
            at.sha256("sha256", &record.sha256)
        }
        Body::Artefact(artefact) => {
            let at = at.child("body.artefact");
            at.text("name", &artefact.name)?;
            at.sha256("sha256", &artefact.sha256)
        }
        Body::Receipt(receipt) => {
            let at = at.child("body.receipt");
            at.id("directive_id", &receipt.directive_id)?;
            at.required::<DirectiveDisposition>("disposition", receipt.disposition)?;
            Ok(())
        }
        Body::StageResult(result) => {
            match stage::violations(&at.child("body.stage_result"), result)
                .into_iter()
                .next()
            {
                Some(error) => Err(error),
                None => Ok(()),
            }
        }
    }
}

/// A body that may be runner-level carries the whole fence or none of it.
fn runner_fence(at: &At<'_>, m: &RunnerEvent) -> Checked {
    let set = [
        !m.job_id.is_empty(),
        !m.attempt_id.is_empty(),
        m.lease_epoch != 0,
    ];
    if set.iter().all(|&s| s) || set.iter().all(|&s| !s) {
        return Ok(());
    }
    let names = ["job_id", "attempt_id", "lease_epoch"];
    let empty = set
        .iter()
        .zip(names)
        .find_map(|(&s, name)| (!s).then_some(name))
        .unwrap_or("lease_epoch");
    Err(at.fail(empty, Rule::PartialFence))
}

// Received by the runner.

fn register_response(at: &At<'_>, m: &RegisterResponse) -> Checked {
    at.id("runner_id", &m.runner_id)?;
    at.text("project", &m.project)?;
    at.text("audience", &m.audience)
}

fn authenticate_response(at: &At<'_>, m: &AuthenticateResponse) -> Checked {
    at.secret("access_token", &m.access_token)?;
    at.present("expires_at", &m.expires_at)?;
    Ok(())
}

/// An empty `missing` list is the plane holding every blob, so it is valid.
fn stat_blobs_response(at: &At<'_>, m: &StatBlobsResponse) -> Checked {
    at.sha256_each("missing", &m.missing)
}

fn put_blob_response(at: &At<'_>, m: &PutBlobResponse) -> Checked {
    at.sha256("sha256", &m.sha256)
}

fn get_blob_response(at: &At<'_>, m: &GetBlobResponse) -> Checked {
    match at.present("part", &m.part)? {
        get_blob_response::Part::Header(header) => {
            at.child("part.header").sha256("sha256", &header.sha256)
        }
        get_blob_response::Part::Chunk(_) => Ok(()),
    }
}

fn connect_response(at: &At<'_>, m: &ConnectResponse) -> Checked {
    use connect_response::Body;
    let Some(body) = &m.body else {
        return Err(at.fail("body", Rule::NoBody));
    };
    match body {
        Body::HelloAck(ack) => hello_ack(&at.child("body.hello_ack"), ack),
        Body::ClaimResult(result) => {
            let at = at.child("body.claim_result");
            at.id("request_id", &result.request_id)?;
            match at.present("result", &result.result)? {
                claim_result::Result::NoWork(_) => Ok(()),
                claim_result::Result::Refused(refused) => {
                    at.child("result.refused")
                        .required::<ClaimRefusalReason>("reason", refused.reason)?;
                    Ok(())
                }
            }
        }
        Body::HeartbeatAck(ack) => {
            let at = at.child("body.heartbeat_ack");
            for (index, lease) in ack.leases.iter().enumerate() {
                let at = at.item("leases", index);
                at.id("attempt_id", &lease.attempt_id)?;
                at.non_zero("lease_epoch", lease.lease_epoch)?;
                let _: &lease_state::State = at.present("state", &lease.state)?;
            }
            at.present("plane_time", &ack.plane_time)?;
            Ok(())
        }
        Body::Directive(directive) => self::directive(&at.child("body.directive"), directive),
        Body::Reauthenticated(reauthenticated) => {
            at.child("body.reauthenticated")
                .present("expires_at", &reauthenticated.expires_at)?;
            Ok(())
        }
        Body::NeedBlobs(need) => {
            let at = at.child("body.need_blobs");
            at.non_empty("sha256", &need.sha256)?;
            at.sha256_each("sha256", &need.sha256)
        }
        Body::GoAway(go_away) => {
            at.child("body.go_away")
                .required::<GoAwayReason>("reason", go_away.reason)?;
            Ok(())
        }
        Body::Ack(_) | Body::Resend(_) => Ok(()),
    }
}

fn hello_ack(at: &At<'_>, m: &HelloAck) -> Checked {
    match at.present("result", &m.result)? {
        hello_ack::Result::Rejected(rejected) => {
            at.child("result.rejected")
                .required::<HelloRejectionStatus>("status", rejected.status)?;
            Ok(())
        }
        hello_ack::Result::Accepted(accepted) => {
            let at = at.child("result.accepted");
            at.id("session_id", &accepted.session_id)?;
            at.present("limits", &accepted.limits)?;
            at.present("timings", &accepted.timings)?;
            for (index, attempt) in accepted.held_attempts.iter().enumerate() {
                let at = at.item("held_attempts", index);
                at.id("attempt_id", &attempt.attempt_id)?;
                at.non_zero("lease_epoch", attempt.lease_epoch)?;
                at.required::<PlaneAttemptState>("state", attempt.state)?;
            }
            for (index, directive) in accepted.pending_directives.iter().enumerate() {
                self::directive(&at.item("pending_directives", index), directive)?;
            }
            Ok(())
        }
    }
}

fn directive(at: &At<'_>, m: &Directive) -> Checked {
    use directive::Kind;
    at.id("directive_id", &m.directive_id)?;
    match at.present("kind", &m.kind)? {
        Kind::Cancel(cancel) => {
            let at = at.child("kind.cancel");
            at.id("attempt_id", &cancel.attempt_id)?;
            at.non_zero("lease_epoch", cancel.lease_epoch)
        }
        Kind::Interrupt(interrupt) => {
            let at = at.child("kind.interrupt");
            at.id("attempt_id", &interrupt.attempt_id)?;
            at.non_zero("lease_epoch", interrupt.lease_epoch)
        }
        Kind::Deliver(deliver) => {
            let at = at.child("kind.deliver");
            at.id("attempt_id", &deliver.attempt_id)?;
            at.non_zero("lease_epoch", deliver.lease_epoch)?;
            at.text("message", &deliver.message)
        }
        Kind::Drain(drain) => {
            at.child("kind.drain")
                .present("deadline", &drain.deadline)?;
            Ok(())
        }
        Kind::Upgrade(_) => Ok(()),
    }
}
