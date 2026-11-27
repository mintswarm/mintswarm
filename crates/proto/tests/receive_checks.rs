//! The wire receive checks: a valid instance of each received `runner/v1`
//! message is accepted unchanged, each rule gives its typed error naming the
//! message, the field and the rule, and no default instance panics.

#![cfg(test)]

use std::collections::BTreeMap;

use mintswarm_proto::check::{Receive, ReceiveError, Rule};
use mintswarm_proto::prost_types::{Duration, Timestamp};
use mintswarm_proto::runner::v1::{
    Ack, ArtefactPut, AttemptOutcome, AttemptOutcomeKind, AttemptStartFailed,
    AttemptStartFailedReason, AttemptStarted, AuthenticateRequest, AuthenticateResponse,
    CancelClaim, CapabilitiesUpdate, Claim, ClaimRefusalReason, ClaimRefused, ClaimResult,
    ConnectRequest, ConnectResponse, Directive, DirectiveDisposition, DirectiveReceipt, EventBatch,
    GateResult, GetBlobHeader, GetBlobRequest, GetBlobResponse, GoAway, GoAwayReason, Goodbye,
    GoodbyeReason, Harness, Heartbeat, HeartbeatAck, HeldAttempt, Hello, HelloAccepted, HelloAck,
    HelloRejected, HelloRejectionStatus, HelloRepository, HostMetrics, LeaseState, LoadSummary,
    NeedBlobs, NoWork, PlaneAttemptState, PlaneHeldAttempt, ProbeResult, Progress, PutBlobHeader,
    PutBlobRequest, PutBlobResponse, Reauthenticate, Reauthenticated, RecordPut, RegisterRequest,
    RegisterResponse, Repository, Resend, RunnerEvent, RunnerMode, RunnerState, Secret,
    SessionLimits, SessionTimings, StageResult, StageStatus, StatBlobsRequest, StatBlobsResponse,
    StepKind, Usage, Warning, connect_request, connect_response, directive, get_blob_response,
    hello_ack, lease_state, probe_result, progress, put_blob_request, runner_event,
};

const REQUEST: &str = "mintswarm.runner.v1.ConnectRequest";
const RESPONSE: &str = "mintswarm.runner.v1.ConnectResponse";

fn sha() -> String {
    "ab".repeat(32)
}

fn at() -> Option<Timestamp> {
    Some(Timestamp {
        seconds: 1_800_000_000,
        nanos: 0,
    })
}

fn secret(value: &str) -> Option<Secret> {
    Some(Secret {
        value: value.to_owned(),
    })
}

/// Asserts that `result` is refused with `message`, `field` and `rule`.
fn refused<M: std::fmt::Debug>(
    result: Result<M, ReceiveError>,
    message: &str,
    field: &str,
    rule: Rule,
) {
    let error = result.expect_err("the message is refused");
    assert_eq!(
        (error.message.as_str(), error.field.as_str(), &error.rule),
        (message, field, &rule),
        "{error}"
    );
}

/// Asserts that `message` is accepted unchanged.
fn accepted<M: Receive<Error = ReceiveError> + Clone + PartialEq + std::fmt::Debug>(message: M) {
    let received = message
        .clone()
        .receive()
        .unwrap_or_else(|error| panic!("{message:?} is accepted: {error}"));
    assert_eq!(received, message);
}

// Valid instances.

fn register_request() -> RegisterRequest {
    RegisterRequest {
        registration_token: secret("token"),
        public_key: vec![1, 2, 3],
        repository: Some(Repository {
            remote: "git@github.com:o/r.git".to_owned(),
        }),
        host: String::new(),
        os: String::new(),
        arch: String::new(),
        tags: Vec::new(),
        account: String::new(),
        workers_total: 2,
        runner_version: "0.1.0".to_owned(),
        protocol_minor: 0,
    }
}

fn authenticate_request() -> AuthenticateRequest {
    AuthenticateRequest {
        client_assertion_type: "urn:ietf:params:oauth:client-assertion-type:jwt-bearer".to_owned(),
        client_assertion: secret("jwt"),
    }
}

fn put_blob_header() -> PutBlobHeader {
    PutBlobHeader {
        sha256: sha(),
        size: 3,
        media_type: String::new(),
        attempt_id: "a1".to_owned(),
        lease_epoch: 1,
    }
}

fn get_blob_request() -> GetBlobRequest {
    GetBlobRequest {
        sha256: sha(),
        attempt_id: "a1".to_owned(),
        lease_epoch: 1,
    }
}

fn probe() -> ProbeResult {
    ProbeResult {
        id: "harness.claude".to_owned(),
        source: String::new(),
        observed_at: at(),
        result: Some(probe_result::Result::Known(probe_result::Known {
            version: Some("1.0".to_owned()),
            facts: BTreeMap::from([(
                "logged_in".to_owned(),
                probe_result::Fact {
                    value: Some(probe_result::fact::Value::Flag(true)),
                },
            )]),
        })),
    }
}

fn harness() -> Harness {
    Harness {
        id: "claude".to_owned(),
        version: String::new(),
        logged_in: true,
        credential_mode: String::new(),
        models: Vec::new(),
    }
}

fn held() -> HeldAttempt {
    HeldAttempt {
        attempt_id: "a1".to_owned(),
        lease_epoch: 1,
        phase: String::new(),
    }
}

fn hello() -> Hello {
    Hello {
        runner_version: "0.1.0".to_owned(),
        protocol_minor: 0,
        features: Vec::new(),
        step_kinds: vec![StepKind::Prepare.into()],
        outbox_epoch: "e1".to_owned(),
        first_unacked_seq: 0,
        last_seq: 0,
        host: String::new(),
        os: String::new(),
        arch: String::new(),
        harnesses: vec![harness()],
        capabilities: vec![probe()],
        tags: Vec::new(),
        account: String::new(),
        workers_total: 1,
        repository: Some(HelloRepository {
            remote: "git@github.com:o/r.git".to_owned(),
            default_branch_head: String::new(),
        }),
        mode: RunnerMode::Worker.into(),
        held_attempts: vec![held()],
    }
}

fn job_event(body: runner_event::Body) -> RunnerEvent {
    RunnerEvent {
        seq: 1,
        at: at(),
        job_id: "j1".to_owned(),
        attempt_id: "a1".to_owned(),
        lease_epoch: 1,
        body: Some(body),
    }
}

fn runner_level(body: runner_event::Body) -> RunnerEvent {
    RunnerEvent {
        job_id: String::new(),
        attempt_id: String::new(),
        lease_epoch: 0,
        ..job_event(body)
    }
}

fn outcome(kind: i32) -> runner_event::Body {
    runner_event::Body::Outcome(AttemptOutcome {
        kind,
        reason: String::new(),
        head: None,
    })
}

fn warning() -> runner_event::Body {
    runner_event::Body::Warning(Warning {
        message: "low disk".to_owned(),
    })
}

fn stage_result() -> StageResult {
    StageResult {
        status: StageStatus::Complete.into(),
        artefact: "handoff".to_owned(),
        outputs: Vec::new(),
        questions: Vec::new(),
        reason: String::new(),
        commit: None,
        pr_title: None,
    }
}

fn events(events: Vec<RunnerEvent>) -> ConnectRequest {
    request(connect_request::Body::Events(EventBatch { events }))
}

fn request(body: connect_request::Body) -> ConnectRequest {
    ConnectRequest { body: Some(body) }
}

fn response(body: connect_response::Body) -> ConnectResponse {
    ConnectResponse { body: Some(body) }
}

fn every_event() -> Vec<RunnerEvent> {
    vec![
        job_event(runner_event::Body::Started(AttemptStarted {})),
        job_event(runner_event::Body::StartFailed(AttemptStartFailed {
            reason: AttemptStartFailedReason::Duplicate.into(),
            detail: String::new(),
        })),
        job_event(outcome(AttemptOutcomeKind::Succeeded.into())),
        job_event(runner_event::Body::Usage(Usage {
            tokens: None,
            cost: None,
            limits: Vec::new(),
            source: String::new(),
            observed_at: None,
        })),
        job_event(runner_event::Body::Progress(Progress {
            event: Some(progress::Event::GateFinished(progress::GateFinished {
                name: "rust-test".to_owned(),
                result: GateResult::Passed.into(),
            })),
        })),
        runner_level(warning()),
        job_event(warning()),
        runner_level(runner_event::Body::Record(RecordPut {
            kind: "floor".to_owned(),
            version: 1,
            sha256: sha(),
            size: 1,
        })),
        job_event(runner_event::Body::Artefact(ArtefactPut {
            name: "plan".to_owned(),
            revision: 1,
            sha256: sha(),
            size: 1,
        })),
        runner_level(runner_event::Body::Receipt(DirectiveReceipt {
            directive_id: "d1".to_owned(),
            disposition: DirectiveDisposition::AppliedInSession.into(),
            detail: String::new(),
        })),
        job_event(runner_event::Body::StageResult(stage_result())),
    ]
}

fn every_request() -> Vec<ConnectRequest> {
    vec![
        request(connect_request::Body::Hello(hello())),
        request(connect_request::Body::Claim(Claim {
            request_id: "r1".to_owned(),
            worker_id: "w1".to_owned(),
        })),
        request(connect_request::Body::CancelClaim(CancelClaim {
            request_id: "r1".to_owned(),
        })),
        request(connect_request::Body::Heartbeat(Heartbeat {
            state: RunnerState::Online.into(),
            workers_free: 1,
            workers_total: 1,
            load: None,
            throttled_until: None,
            held: vec![held()],
            last_seq: 0,
            capabilities_digest: String::new(),
        })),
        events(every_event()),
        request(connect_request::Body::Reauthenticate(Reauthenticate {
            access_token: secret("token"),
        })),
        request(connect_request::Body::Capabilities(CapabilitiesUpdate {
            capabilities: vec![probe()],
            harnesses: vec![harness()],
        })),
        request(connect_request::Body::Metrics(HostMetrics {
            observed_at: at(),
            load: Some(LoadSummary::default()),
        })),
        request(connect_request::Body::Goodbye(Goodbye {
            reason: GoodbyeReason::Drained.into(),
        })),
    ]
}

fn cancel() -> Directive {
    Directive {
        directive_id: "d1".to_owned(),
        kind: Some(directive::Kind::Cancel(directive::Cancel {
            attempt_id: "a1".to_owned(),
            lease_epoch: 1,
        })),
    }
}

fn accepted_hello() -> HelloAccepted {
    HelloAccepted {
        session_id: "s1".to_owned(),
        protocol_minor: 0,
        features: Vec::new(),
        acked_seq: 0,
        limits: Some(SessionLimits::default()),
        timings: Some(SessionTimings::default()),
        held_attempts: vec![PlaneHeldAttempt {
            attempt_id: "a1".to_owned(),
            lease_epoch: 1,
            state: PlaneAttemptState::Started.into(),
        }],
        pending_directives: vec![cancel()],
    }
}

fn directives() -> Vec<Directive> {
    let with = |kind| Directive {
        directive_id: "d1".to_owned(),
        kind: Some(kind),
    };
    vec![
        cancel(),
        with(directive::Kind::Interrupt(directive::Interrupt {
            attempt_id: "a1".to_owned(),
            lease_epoch: 1,
            message: None,
        })),
        with(directive::Kind::Deliver(directive::Deliver {
            attempt_id: "a1".to_owned(),
            lease_epoch: 1,
            message: "look again".to_owned(),
        })),
        with(directive::Kind::Drain(directive::Drain { deadline: at() })),
        with(directive::Kind::Upgrade(directive::UpgradeRequired {
            min_protocol_minor: 1,
            detail: String::new(),
        })),
    ]
}

fn every_response() -> Vec<ConnectResponse> {
    let mut out = vec![
        response(connect_response::Body::HelloAck(HelloAck {
            result: Some(hello_ack::Result::Accepted(accepted_hello())),
        })),
        response(connect_response::Body::HelloAck(HelloAck {
            result: Some(hello_ack::Result::Rejected(HelloRejected {
                status: HelloRejectionStatus::Revoked.into(),
                detail: String::new(),
                min_protocol_minor: 0,
                plane_version: String::new(),
            })),
        })),
        response(connect_response::Body::ClaimResult(ClaimResult {
            request_id: "r1".to_owned(),
            result: Some(claim_result_refused()),
        })),
        response(connect_response::Body::ClaimResult(ClaimResult {
            request_id: "r1".to_owned(),
            result: Some(mintswarm_proto::runner::v1::claim_result::Result::NoWork(
                NoWork {},
            )),
        })),
        response(connect_response::Body::HeartbeatAck(HeartbeatAck {
            leases: vec![LeaseState {
                attempt_id: "a1".to_owned(),
                lease_epoch: 1,
                state: Some(lease_state::State::ExpiresAt(Timestamp::default())),
            }],
            plane_time: at(),
            resend_capabilities: false,
        })),
        response(connect_response::Body::Ack(Ack { seq: 1 })),
        response(connect_response::Body::Reauthenticated(Reauthenticated {
            expires_at: at(),
        })),
        response(connect_response::Body::NeedBlobs(NeedBlobs {
            seq: 1,
            sha256: vec![sha()],
        })),
        response(connect_response::Body::Resend(Resend { from: 1 })),
        response(connect_response::Body::GoAway(GoAway {
            reason: GoAwayReason::Shutdown.into(),
            reconnect_after: Some(Duration::default()),
        })),
    ];
    out.extend(
        directives()
            .into_iter()
            .map(|d| response(connect_response::Body::Directive(d))),
    );
    out
}

fn claim_result_refused() -> mintswarm_proto::runner::v1::claim_result::Result {
    mintswarm_proto::runner::v1::claim_result::Result::Refused(ClaimRefused {
        reason: ClaimRefusalReason::Draining.into(),
    })
}

#[test]
fn a_valid_instance_of_each_type_is_accepted_unchanged() {
    accepted(register_request());
    accepted(authenticate_request());
    accepted(StatBlobsRequest {
        sha256: vec![sha()],
    });
    accepted(PutBlobRequest {
        part: Some(put_blob_request::Part::Header(put_blob_header())),
    });
    accepted(PutBlobRequest {
        part: Some(put_blob_request::Part::Chunk(Vec::new())),
    });
    accepted(get_blob_request());
    every_request().into_iter().for_each(accepted);
    accepted(RegisterResponse {
        runner_id: "r1".to_owned(),
        project: "mintswarm".to_owned(),
        default_tags: Vec::new(),
        audience: "plane".to_owned(),
    });
    accepted(AuthenticateResponse {
        access_token: secret("token"),
        expires_at: at(),
    });
    accepted(StatBlobsResponse {
        missing: vec![sha()],
    });
    accepted(PutBlobResponse {
        sha256: sha(),
        size: 1,
    });
    accepted(GetBlobResponse {
        part: Some(get_blob_response::Part::Header(GetBlobHeader {
            sha256: sha(),
            size: 1,
            media_type: String::new(),
        })),
    });
    every_response().into_iter().for_each(accepted);
}

#[test]
fn a_missing_required_field_is_missing() {
    let message = "mintswarm.runner.v1.RegisterRequest";
    let mut m = register_request();
    m.registration_token = None;
    refused(m.receive(), message, "registration_token", Rule::Missing);
    let mut m = register_request();
    m.registration_token = secret("");
    refused(
        m.receive(),
        message,
        "registration_token.value",
        Rule::Missing,
    );
    let mut m = register_request();
    m.repository = Some(Repository::default());
    refused(m.receive(), message, "repository.remote", Rule::Missing);
    refused(
        PutBlobRequest { part: None }.receive(),
        "mintswarm.runner.v1.PutBlobRequest",
        "part",
        Rule::Missing,
    );
    refused(
        StatBlobsRequest { sha256: Vec::new() }.receive(),
        "mintswarm.runner.v1.StatBlobsRequest",
        "sha256",
        Rule::Missing,
    );
    let mut h = hello();
    h.repository = None;
    refused(
        request(connect_request::Body::Hello(h)).receive(),
        REQUEST,
        "body.hello.repository",
        Rule::Missing,
    );
    let mut d = cancel();
    d.kind = Some(directive::Kind::Drain(directive::Drain { deadline: None }));
    refused(
        response(connect_response::Body::Directive(d)).receive(),
        RESPONSE,
        "body.directive.kind.drain.deadline",
        Rule::Missing,
    );
}

#[test]
fn a_probe_result_with_no_result_case_is_missing() {
    let mut h = hello();
    h.capabilities[0].result = None;
    refused(
        request(connect_request::Body::Hello(h)).receive(),
        REQUEST,
        "body.hello.capabilities[0].result",
        Rule::Missing,
    );
}

#[test]
fn a_fact_with_no_value_is_missing() {
    let mut p = probe();
    p.result = Some(probe_result::Result::Known(probe_result::Known {
        version: None,
        facts: BTreeMap::from([("models".to_owned(), probe_result::Fact { value: None })]),
    }));
    refused(
        request(connect_request::Body::Capabilities(CapabilitiesUpdate {
            capabilities: vec![p],
            harnesses: Vec::new(),
        }))
        .receive(),
        REQUEST,
        "body.capabilities.capabilities[0].result.known.facts[\"models\"].value",
        Rule::Missing,
    );
}

#[test]
fn an_envelope_with_no_body_is_no_body() {
    refused(
        ConnectRequest { body: None }.receive(),
        REQUEST,
        "body",
        Rule::NoBody,
    );
    refused(
        ConnectResponse { body: None }.receive(),
        RESPONSE,
        "body",
        Rule::NoBody,
    );
}

#[test]
fn an_unspecified_required_enum_is_unspecified() {
    let mut h = hello();
    h.mode = RunnerMode::Unspecified.into();
    refused(
        request(connect_request::Body::Hello(h)).receive(),
        REQUEST,
        "body.hello.mode",
        Rule::Unspecified,
    );
    refused(
        request(connect_request::Body::Goodbye(Goodbye { reason: 0 })).receive(),
        REQUEST,
        "body.goodbye.reason",
        Rule::Unspecified,
    );
    let mut h = hello();
    h.step_kinds = vec![StepKind::Gates.into(), StepKind::Unspecified.into()];
    refused(
        request(connect_request::Body::Hello(h)).receive(),
        REQUEST,
        "body.hello.step_kinds[1]",
        Rule::Unspecified,
    );
    refused(
        events(vec![job_event(outcome(0))]).receive(),
        REQUEST,
        "body.events.events[0].body.outcome.kind",
        Rule::Unspecified,
    );
}

#[test]
fn an_unknown_required_enum_is_an_unknown_value() {
    refused(
        response(connect_response::Body::GoAway(GoAway {
            reason: 77,
            reconnect_after: None,
        }))
        .receive(),
        RESPONSE,
        "body.go_away.reason",
        Rule::UnknownValue { raw: 77 },
    );
}

#[test]
fn an_unknown_step_kind_is_kept() {
    let mut h = hello();
    h.step_kinds = vec![StepKind::Gates.into(), 99];
    let received = request(connect_request::Body::Hello(h))
        .receive()
        .expect("an unknown step kind is accepted");
    let Some(connect_request::Body::Hello(h)) = received.body else {
        panic!("the body is kept");
    };
    assert_eq!(h.step_kinds, vec![i32::from(StepKind::Gates), 99]);
}

#[test]
fn an_empty_id_is_an_empty_id() {
    refused(
        request(connect_request::Body::Claim(Claim {
            request_id: "r1".to_owned(),
            worker_id: String::new(),
        }))
        .receive(),
        REQUEST,
        "body.claim.worker_id",
        Rule::EmptyId,
    );
    let mut h = hello();
    h.harnesses[0].id = String::new();
    refused(
        request(connect_request::Body::Hello(h)).receive(),
        REQUEST,
        "body.hello.harnesses[0].id",
        Rule::EmptyId,
    );
    refused(
        RegisterResponse::default().receive(),
        "mintswarm.runner.v1.RegisterResponse",
        "runner_id",
        Rule::EmptyId,
    );
    let mut ack = accepted_hello();
    ack.pending_directives[0].directive_id = String::new();
    refused(
        response(connect_response::Body::HelloAck(HelloAck {
            result: Some(hello_ack::Result::Accepted(ack)),
        }))
        .receive(),
        RESPONSE,
        "body.hello_ack.result.accepted.pending_directives[0].directive_id",
        Rule::EmptyId,
    );
}

#[test]
fn a_malformed_sha256_is_malformed() {
    for bad in [
        "AB".repeat(32),
        "ab".repeat(31),
        "ab".repeat(33),
        "xy".repeat(32),
        String::new(),
    ] {
        refused(
            PutBlobResponse {
                sha256: bad.clone(),
                size: 1,
            }
            .receive(),
            "mintswarm.runner.v1.PutBlobResponse",
            "sha256",
            Rule::MalformedSha256,
        );
        refused(
            StatBlobsRequest {
                sha256: vec![sha(), bad.clone()],
            }
            .receive(),
            "mintswarm.runner.v1.StatBlobsRequest",
            "sha256[1]",
            Rule::MalformedSha256,
        );
        refused(
            PutBlobRequest {
                part: Some(put_blob_request::Part::Header(PutBlobHeader {
                    sha256: bad,
                    ..put_blob_header()
                })),
            }
            .receive(),
            "mintswarm.runner.v1.PutBlobRequest",
            "part.header.sha256",
            Rule::MalformedSha256,
        );
    }
}

#[test]
fn a_zero_seq_or_lease_epoch_is_zero() {
    refused(
        GetBlobRequest {
            lease_epoch: 0,
            ..get_blob_request()
        }
        .receive(),
        "mintswarm.runner.v1.GetBlobRequest",
        "lease_epoch",
        Rule::Zero,
    );
    let mut event = job_event(runner_event::Body::Started(AttemptStarted {}));
    event.seq = 0;
    refused(
        events(vec![event]).receive(),
        REQUEST,
        "body.events.events[0].seq",
        Rule::Zero,
    );
}

#[test]
fn a_job_event_needs_its_fence() {
    refused(
        events(vec![runner_level(runner_event::Body::Started(
            AttemptStarted {},
        ))])
        .receive(),
        REQUEST,
        "body.events.events[0].job_id",
        Rule::EmptyId,
    );
}

#[test]
fn a_partial_fence_is_refused() {
    let mut event = job_event(warning());
    event.attempt_id = String::new();
    refused(
        events(vec![event]).receive(),
        REQUEST,
        "body.events.events[0].attempt_id",
        Rule::PartialFence,
    );
    let mut event = runner_level(warning());
    event.lease_epoch = 3;
    refused(
        events(vec![event]).receive(),
        REQUEST,
        "body.events.events[0].job_id",
        Rule::PartialFence,
    );
}

#[test]
fn a_nested_stage_result_reports_its_first_violation() {
    let mut result = stage_result();
    result.status = StageStatus::Unspecified.into();
    refused(
        events(vec![job_event(runner_event::Body::StageResult(result))]).receive(),
        REQUEST,
        "body.events.events[0].body.stage_result.status",
        Rule::Unspecified,
    );
}

#[test]
fn an_unknown_outcome_kind_is_accepted_as_failed_with_the_raw_value_kept() {
    let received = events(vec![job_event(outcome(99))])
        .receive()
        .expect("an unknown outcome kind is accepted");
    let Some(connect_request::Body::Events(batch)) = received.body else {
        panic!("the body is kept");
    };
    let Some(runner_event::Body::Outcome(outcome)) = &batch.events[0].body else {
        panic!("the outcome is kept");
    };
    assert_eq!(outcome.kind, 99);
    assert_eq!(outcome.effective_kind(), AttemptOutcomeKind::Failed);
    let known = AttemptOutcome {
        kind: AttemptOutcomeKind::TimedOut.into(),
        reason: String::new(),
        head: None,
    };
    assert_eq!(known.effective_kind(), AttemptOutcomeKind::TimedOut);
}

#[test]
fn the_default_instance_of_every_checked_type_is_refused() {
    fn refuses<M: Receive<Error = ReceiveError> + Default + std::fmt::Debug>() {
        let error = M::default()
            .receive()
            .expect_err("the default instance is refused");
        assert!(!error.field.is_empty(), "{error}");
    }
    refuses::<RegisterRequest>();
    refuses::<AuthenticateRequest>();
    refuses::<StatBlobsRequest>();
    refuses::<PutBlobRequest>();
    refuses::<GetBlobRequest>();
    refuses::<ConnectRequest>();
    refuses::<RegisterResponse>();
    refuses::<AuthenticateResponse>();
    refuses::<PutBlobResponse>();
    refuses::<GetBlobResponse>();
    refuses::<ConnectResponse>();
}

#[test]
fn the_default_stat_blobs_response_is_accepted() {
    // The plane holding every blob; exempt from the default-instance rule.
    accepted(StatBlobsResponse::default());
}

#[test]
fn each_envelope_with_a_default_body_case_returns_without_a_panic() {
    use connect_request::Body as Req;
    use connect_response::Body as Res;
    for body in [
        Req::Hello(Hello::default()),
        Req::Claim(Claim::default()),
        Req::CancelClaim(CancelClaim::default()),
        Req::Heartbeat(Heartbeat::default()),
        Req::Events(EventBatch::default()),
        Req::Events(EventBatch {
            events: vec![RunnerEvent::default()],
        }),
        Req::Reauthenticate(Reauthenticate::default()),
        Req::Capabilities(CapabilitiesUpdate::default()),
        Req::Metrics(HostMetrics::default()),
        Req::Goodbye(Goodbye::default()),
    ] {
        let _ = request(body).receive();
    }
    for body in [
        Res::HelloAck(HelloAck::default()),
        Res::ClaimResult(ClaimResult::default()),
        Res::HeartbeatAck(HeartbeatAck::default()),
        Res::Ack(Ack::default()),
        Res::Directive(Directive::default()),
        Res::Reauthenticated(Reauthenticated::default()),
        Res::NeedBlobs(NeedBlobs::default()),
        Res::Resend(Resend::default()),
        Res::GoAway(GoAway::default()),
    ] {
        let _ = response(body).receive();
    }
}

#[test]
fn errors_display_the_message_the_field_and_the_rule() {
    let error = ConnectRequest { body: None }
        .receive()
        .expect_err("no body is refused");
    assert_eq!(
        error.to_string(),
        "mintswarm.runner.v1.ConnectRequest: body: has no body case this build knows"
    );
}
