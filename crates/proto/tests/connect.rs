//! The `mintswarm.runner.v1` `Connect` stream: the envelopes have the cases
//! of design §5.1 at the design's field numbers, with 6 unused on both
//! sides; the session, claim, heartbeat and directive messages carry every
//! field of design §6.1, §6.2, §7.1, §7.3 and §7.6; and the enums have
//! exactly their values.
//!
//! The struct literals are exhaustive and the matches have no wildcard, so a
//! field or case that is added, dropped or renamed fails compilation.

#![cfg(test)]

use mintswarm_proto::runner::v1::{
    Ack, CancelClaim, CapabilitiesUpdate, Claim, ClaimRefusalReason, ClaimRefused, ClaimResult,
    ConnectRequest, ConnectResponse, Directive, EventBatch, GoAway, GoAwayReason, Goodbye,
    GoodbyeReason, Harness, Heartbeat, HeartbeatAck, HeldAttempt, Hello, HelloAccepted, HelloAck,
    HelloRejected, HelloRejectionStatus, HelloRepository, HostMetrics, LeaseState, LoadSummary,
    NeedBlobs, NoWork, PlaneAttemptState, PlaneHeldAttempt, ProbeResult, Reauthenticate,
    Reauthenticated, Resend, RunnerMode, RunnerState, Secret, SessionLimits, SessionTimings,
    StepKind, claim_result, connect_request, connect_response, directive, hello_ack, lease_state,
};
use prost::Message;

fn at(seconds: i64) -> Option<mintswarm_proto::prost_types::Timestamp> {
    Some(mintswarm_proto::prost_types::Timestamp { seconds, nanos: 0 })
}

fn harness() -> Harness {
    Harness {
        id: "claude-code".to_owned(),
        version: "2.1.0".to_owned(),
        logged_in: true,
        credential_mode: "subscription".to_owned(),
        models: vec!["opus".to_owned(), "sonnet".to_owned()],
    }
}

fn probe() -> ProbeResult {
    ProbeResult {
        id: "tool.cargo.version".to_owned(),
        value: "1.87.0".to_owned(),
        source: "cargo --version".to_owned(),
        observed_at: at(1_800_000_000),
    }
}

fn held() -> HeldAttempt {
    HeldAttempt {
        attempt_id: "attempt-1".to_owned(),
        lease_epoch: 3,
        phase: "running".to_owned(),
    }
}

fn load() -> LoadSummary {
    LoadSummary {
        cpu_pct: 42.5,
        load_1m: 1.25,
        mem_used_pct: 63.0,
        disk_low: false,
    }
}

fn hello() -> Hello {
    Hello {
        runner_version: "0.1.0".to_owned(),
        protocol_minor: 0,
        features: vec!["step_jobs".to_owned(), "host_metrics".to_owned()],
        step_kinds: vec![StepKind::Prepare.into(), StepKind::Gates.into()],
        outbox_epoch: "0f1e2d3c4b5a69788796a5b4c3d2e1f0".to_owned(),
        first_unacked_seq: 41,
        last_seq: 57,
        host: "build-1".to_owned(),
        os: "linux".to_owned(),
        arch: "aarch64".to_owned(),
        harnesses: vec![harness()],
        capabilities: vec![probe()],
        tags: vec!["gpu".to_owned()],
        account: "main".to_owned(),
        workers_total: 2,
        repository: Some(HelloRepository {
            remote: "https://github.com/mintswarm/mintswarm.git".to_owned(),
            default_branch_head: "0123456789abcdef0123456789abcdef01234567".to_owned(),
        }),
        mode: RunnerMode::Worker.into(),
        held_attempts: vec![held()],
    }
}

fn heartbeat() -> Heartbeat {
    Heartbeat {
        state: RunnerState::Online.into(),
        workers_free: 1,
        workers_total: 2,
        load: Some(load()),
        throttled_until: at(1_800_000_600),
        held: vec![held()],
        last_seq: 57,
        capabilities_digest: "ab".repeat(32),
    }
}

/// One directive of every kind, in the order of design §7.6.
fn directives() -> Vec<Directive> {
    let kinds = vec![
        directive::Kind::Cancel(directive::Cancel {
            attempt_id: "attempt-1".to_owned(),
            lease_epoch: 3,
        }),
        directive::Kind::Interrupt(directive::Interrupt {
            attempt_id: "attempt-1".to_owned(),
            lease_epoch: 3,
            message: Some("stop and summarise".to_owned()),
        }),
        directive::Kind::Deliver(directive::Deliver {
            attempt_id: "attempt-1".to_owned(),
            lease_epoch: 3,
            message: "use the new fixture".to_owned(),
        }),
        directive::Kind::Drain(directive::Drain {
            deadline: at(1_800_003_600),
        }),
        directive::Kind::Upgrade(directive::UpgradeRequired {
            min_protocol_minor: 1,
            detail: "this plane needs minor 1".to_owned(),
        }),
    ];
    kinds
        .into_iter()
        .zip(1_u32..)
        .map(|(kind, n)| Directive {
            directive_id: format!("d-{n}"),
            kind: Some(kind),
        })
        .collect()
}

fn hello_accepted() -> HelloAccepted {
    HelloAccepted {
        session_id: "session-1".to_owned(),
        protocol_minor: 0,
        features: vec!["step_jobs".to_owned()],
        acked_seq: 40,
        limits: Some(SessionLimits {
            max_message_bytes: 4 * 1024 * 1024,
            max_blob_bytes: 64 * 1024 * 1024,
            max_in_flight_events: 1_024,
            max_in_flight_bytes: 4 * 1024 * 1024,
            claim_wait_seconds: 30,
        }),
        timings: Some(SessionTimings {
            heartbeat_seconds: 10,
            lease_seconds: 60,
            metrics_seconds: 30,
        }),
        held_attempts: vec![PlaneHeldAttempt {
            attempt_id: "attempt-1".to_owned(),
            lease_epoch: 3,
            state: PlaneAttemptState::Started.into(),
        }],
        pending_directives: directives(),
    }
}

/// Every runner envelope case, in field-number order.
fn request_bodies() -> Vec<connect_request::Body> {
    vec![
        connect_request::Body::Hello(hello()),
        connect_request::Body::Claim(Claim {
            request_id: "claim-1".to_owned(),
            worker_id: "worker-0".to_owned(),
        }),
        connect_request::Body::CancelClaim(CancelClaim {
            request_id: "claim-1".to_owned(),
        }),
        connect_request::Body::Heartbeat(heartbeat()),
        connect_request::Body::Events(EventBatch { events: Vec::new() }),
        connect_request::Body::Reauthenticate(Reauthenticate {
            access_token: Some(Secret {
                value: "token".to_owned(),
            }),
        }),
        connect_request::Body::Capabilities(CapabilitiesUpdate {
            capabilities: vec![probe()],
            harnesses: vec![harness()],
        }),
        connect_request::Body::Metrics(HostMetrics {
            observed_at: at(1_800_000_030),
            load: Some(load()),
        }),
        connect_request::Body::Goodbye(Goodbye {
            reason: GoodbyeReason::Drained.into(),
        }),
    ]
}

/// Every plane envelope case, in field-number order.
fn response_bodies() -> Vec<connect_response::Body> {
    let directive = directives().into_iter().next().expect("a directive");
    vec![
        connect_response::Body::HelloAck(HelloAck {
            result: Some(hello_ack::Result::Accepted(hello_accepted())),
        }),
        connect_response::Body::ClaimResult(ClaimResult {
            request_id: "claim-1".to_owned(),
            result: Some(claim_result::Result::NoWork(NoWork {})),
        }),
        connect_response::Body::HeartbeatAck(HeartbeatAck {
            leases: vec![LeaseState {
                attempt_id: "attempt-1".to_owned(),
                lease_epoch: 3,
                state: Some(lease_state::State::ExpiresAt(
                    mintswarm_proto::prost_types::Timestamp {
                        seconds: 1_800_000_060,
                        nanos: 0,
                    },
                )),
            }],
            plane_time: at(1_800_000_000),
            resend_capabilities: false,
        }),
        connect_response::Body::Ack(Ack { seq: 57 }),
        connect_response::Body::Directive(directive),
        connect_response::Body::Reauthenticated(Reauthenticated {
            expires_at: at(1_800_003_600),
        }),
        connect_response::Body::NeedBlobs(NeedBlobs {
            seq: 42,
            sha256: vec!["cd".repeat(32)],
        }),
        connect_response::Body::Resend(Resend { from: 41 }),
        connect_response::Body::GoAway(GoAway {
            reason: GoAwayReason::Superseded.into(),
            reconnect_after: None,
        }),
    ]
}

/// The design's field number of a runner envelope case; wildcard-free.
fn request_number(body: &connect_request::Body) -> u8 {
    match body {
        connect_request::Body::Hello(_) => 1,
        connect_request::Body::Claim(_) => 2,
        connect_request::Body::CancelClaim(_) => 3,
        connect_request::Body::Heartbeat(_) => 4,
        connect_request::Body::Events(_) => 5,
        connect_request::Body::Reauthenticate(_) => 7,
        connect_request::Body::Capabilities(_) => 8,
        connect_request::Body::Metrics(_) => 9,
        connect_request::Body::Goodbye(_) => 10,
    }
}

/// The design's field number of a plane envelope case; wildcard-free.
fn response_number(body: &connect_response::Body) -> u8 {
    match body {
        connect_response::Body::HelloAck(_) => 1,
        connect_response::Body::ClaimResult(_) => 2,
        connect_response::Body::HeartbeatAck(_) => 3,
        connect_response::Body::Ack(_) => 4,
        connect_response::Body::Directive(_) => 5,
        connect_response::Body::Reauthenticated(_) => 7,
        connect_response::Body::NeedBlobs(_) => 8,
        connect_response::Body::Resend(_) => 9,
        connect_response::Body::GoAway(_) => 10,
    }
}

/// The first byte of an encoded message: the tag of its first field, which
/// for a length-delimited field numbered below 16 is `(n << 3) | 2`.
fn first_tag(encoded: &[u8]) -> u8 {
    *encoded.first().expect("the message is not empty")
}

#[test]
fn connect_request_cases_have_the_design_numbers() {
    let bodies = request_bodies();
    let numbers: Vec<u8> = bodies.iter().map(request_number).collect();
    assert_eq!(numbers, [1, 2, 3, 4, 5, 7, 8, 9, 10]);
    for body in bodies {
        let n = request_number(&body);
        let request = ConnectRequest { body: Some(body) };
        let encoded = request.encode_to_vec();
        assert_eq!(first_tag(&encoded), (n << 3) | 2, "case {n}");
        let decoded = ConnectRequest::decode(encoded.as_slice()).expect("decodes");
        assert_eq!(decoded, request);
    }
}

#[test]
fn connect_response_cases_have_the_design_numbers() {
    let bodies = response_bodies();
    let numbers: Vec<u8> = bodies.iter().map(response_number).collect();
    assert_eq!(numbers, [1, 2, 3, 4, 5, 7, 8, 9, 10]);
    for body in bodies {
        let n = response_number(&body);
        let response = ConnectResponse { body: Some(body) };
        let encoded = response.encode_to_vec();
        assert_eq!(first_tag(&encoded), (n << 3) | 2, "case {n}");
        let decoded = ConnectResponse::decode(encoded.as_slice()).expect("decodes");
        assert_eq!(decoded, response);
    }
}

#[test]
fn hello_carries_the_design_fields() {
    let Hello {
        runner_version,
        protocol_minor,
        features,
        step_kinds,
        outbox_epoch,
        first_unacked_seq,
        last_seq,
        host,
        os,
        arch,
        harnesses,
        capabilities,
        tags,
        account,
        workers_total,
        repository,
        mode,
        held_attempts,
    } = hello();
    assert_eq!(runner_version, "0.1.0");
    assert_eq!(protocol_minor, 0);
    assert_eq!(features.len(), 2);
    assert_eq!(
        step_kinds,
        [i32::from(StepKind::Prepare), i32::from(StepKind::Gates)]
    );
    assert_eq!(outbox_epoch.len(), 32);
    assert!(first_unacked_seq <= last_seq);
    assert_eq!(
        (host.as_str(), os.as_str(), arch.as_str()),
        ("build-1", "linux", "aarch64")
    );
    let [
        Harness {
            id,
            version,
            logged_in,
            credential_mode,
            models,
        },
    ] = harnesses.as_slice()
    else {
        panic!("one harness, got {harnesses:?}");
    };
    assert_eq!((id.as_str(), version.as_str()), ("claude-code", "2.1.0"));
    assert!(*logged_in);
    assert_eq!(credential_mode, "subscription");
    assert_eq!(models.len(), 2);
    let [
        ProbeResult {
            id,
            value,
            source,
            observed_at,
        },
    ] = capabilities.as_slice()
    else {
        panic!("one probe result, got {capabilities:?}");
    };
    assert_eq!(
        (id.as_str(), value.as_str()),
        ("tool.cargo.version", "1.87.0")
    );
    assert!(!source.is_empty());
    assert!(observed_at.is_some());
    assert_eq!(tags, ["gpu"]);
    assert_eq!(account, "main");
    assert_eq!(workers_total, 2);
    let HelloRepository {
        remote,
        default_branch_head,
    } = repository.expect("repository is set");
    assert!(remote.ends_with(".git"));
    assert_eq!(default_branch_head.len(), 40);
    assert_eq!(mode, i32::from(RunnerMode::Worker));
    let [
        HeldAttempt {
            attempt_id,
            lease_epoch,
            phase,
        },
    ] = held_attempts.as_slice()
    else {
        panic!("one held attempt, got {held_attempts:?}");
    };
    assert_eq!((attempt_id.as_str(), *lease_epoch), ("attempt-1", 3));
    assert_eq!(phase, "running");
}

/// The oneof name of a `HelloAck` result; wildcard-free.
fn hello_ack_name(result: &hello_ack::Result) -> &'static str {
    match result {
        hello_ack::Result::Rejected(_) => "rejected",
        hello_ack::Result::Accepted(_) => "accepted",
    }
}

#[test]
fn hello_ack_is_a_rejection_or_an_acceptance() {
    let rejected = HelloAck {
        result: Some(hello_ack::Result::Rejected(HelloRejected {
            status: HelloRejectionStatus::UpgradeRequired.into(),
            detail: "upgrade to minor 1".to_owned(),
            min_protocol_minor: 1,
            plane_version: "0.2.0".to_owned(),
        })),
    };
    let accepted = HelloAck {
        result: Some(hello_ack::Result::Accepted(hello_accepted())),
    };
    let names: Vec<&str> = [&rejected, &accepted]
        .iter()
        .filter_map(|ack| ack.result.as_ref().map(hello_ack_name))
        .collect();
    assert_eq!(names, ["rejected", "accepted"]);

    let HelloAccepted {
        session_id,
        protocol_minor,
        features,
        acked_seq,
        limits,
        timings,
        held_attempts,
        pending_directives,
    } = hello_accepted();
    assert_eq!(session_id, "session-1");
    assert_eq!(protocol_minor, 0);
    assert_eq!(features, ["step_jobs"]);
    assert_eq!(acked_seq, 40);
    let SessionLimits {
        max_message_bytes,
        max_blob_bytes,
        max_in_flight_events,
        max_in_flight_bytes,
        claim_wait_seconds,
    } = limits.expect("limits are set");
    assert_eq!(max_message_bytes, 4 * 1024 * 1024);
    assert_eq!(max_blob_bytes, 64 * 1024 * 1024);
    assert_eq!(max_in_flight_events, 1_024);
    assert_eq!(max_in_flight_bytes, 4 * 1024 * 1024);
    assert_eq!(claim_wait_seconds, 30);
    let SessionTimings {
        heartbeat_seconds,
        lease_seconds,
        metrics_seconds,
    } = timings.expect("timings are set");
    assert_eq!(
        (heartbeat_seconds, lease_seconds, metrics_seconds),
        (10, 60, 30)
    );
    assert_eq!(held_attempts.len(), 1);
    assert_eq!(pending_directives.len(), 5);

    for ack in [rejected, accepted] {
        let decoded = HelloAck::decode(ack.encode_to_vec().as_slice()).expect("decodes");
        assert_eq!(decoded, ack);
    }
}

/// The oneof name of a `ClaimResult` result; wildcard-free.
fn claim_result_name(result: &claim_result::Result) -> &'static str {
    match result {
        claim_result::Result::NoWork(_) => "no_work",
        claim_result::Result::Refused(_) => "refused",
    }
}

#[test]
fn claim_result_is_no_work_or_refused() {
    let results = [
        ClaimResult {
            request_id: "claim-1".to_owned(),
            result: Some(claim_result::Result::NoWork(NoWork {})),
        },
        ClaimResult {
            request_id: "claim-2".to_owned(),
            result: Some(claim_result::Result::Refused(ClaimRefused {
                reason: ClaimRefusalReason::Observer.into(),
            })),
        },
    ];
    let names: Vec<&str> = results
        .iter()
        .filter_map(|result| result.result.as_ref().map(claim_result_name))
        .collect();
    assert_eq!(names, ["no_work", "refused"]);
}

#[test]
fn heartbeat_carries_the_design_fields() {
    let Heartbeat {
        state,
        workers_free,
        workers_total,
        load,
        throttled_until,
        held,
        last_seq,
        capabilities_digest,
    } = heartbeat();
    assert_eq!(state, i32::from(RunnerState::Online));
    assert!(workers_free <= workers_total);
    let LoadSummary {
        cpu_pct,
        load_1m,
        mem_used_pct,
        disk_low,
    } = load.expect("load is set");
    assert!(cpu_pct > 0.0 && load_1m > 0.0 && mem_used_pct > 0.0);
    assert!(!disk_low);
    assert!(throttled_until.is_some());
    assert_eq!(held.len(), 1);
    assert_eq!(last_seq, 57);
    assert_eq!(capabilities_digest.len(), 64);
}

/// The oneof name of a lease's state; wildcard-free.
fn lease_state_name(state: &lease_state::State) -> &'static str {
    match state {
        lease_state::State::ExpiresAt(_) => "expires_at",
        lease_state::State::Revoked(_) => "revoked",
    }
}

#[test]
fn heartbeat_ack_carries_the_design_fields() {
    let ack = HeartbeatAck {
        leases: vec![
            LeaseState {
                attempt_id: "attempt-1".to_owned(),
                lease_epoch: 3,
                state: Some(lease_state::State::ExpiresAt(
                    mintswarm_proto::prost_types::Timestamp {
                        seconds: 1_800_000_060,
                        nanos: 0,
                    },
                )),
            },
            LeaseState {
                attempt_id: "attempt-2".to_owned(),
                lease_epoch: 1,
                state: Some(lease_state::State::Revoked(lease_state::Revoked {})),
            },
        ],
        plane_time: at(1_800_000_000),
        resend_capabilities: true,
    };
    let names: Vec<&str> = ack
        .leases
        .iter()
        .filter_map(|lease| lease.state.as_ref().map(lease_state_name))
        .collect();
    assert_eq!(names, ["expires_at", "revoked"]);
    let decoded = HeartbeatAck::decode(ack.encode_to_vec().as_slice()).expect("decodes");
    assert_eq!(decoded, ack);
}

/// The oneof name of a directive's kind; wildcard-free.
fn directive_name(kind: &directive::Kind) -> &'static str {
    match kind {
        directive::Kind::Cancel(_) => "cancel",
        directive::Kind::Interrupt(_) => "interrupt",
        directive::Kind::Deliver(_) => "deliver",
        directive::Kind::Drain(_) => "drain",
        directive::Kind::Upgrade(_) => "upgrade",
    }
}

#[test]
fn directive_has_every_kind() {
    let all = directives();
    let names: Vec<&str> = all
        .iter()
        .filter_map(|directive| directive.kind.as_ref().map(directive_name))
        .collect();
    assert_eq!(
        names,
        ["cancel", "interrupt", "deliver", "drain", "upgrade"]
    );
    let ids: Vec<&str> = all.iter().map(|d| d.directive_id.as_str()).collect();
    assert_eq!(ids, ["d-1", "d-2", "d-3", "d-4", "d-5"]);
    for directive in all {
        let decoded = Directive::decode(directive.encode_to_vec().as_slice()).expect("decodes");
        assert_eq!(decoded, directive);
    }
}

#[test]
fn go_away_keeps_an_unset_reconnect_after_apart_from_zero() {
    let unset = GoAway {
        reason: GoAwayReason::Shutdown.into(),
        reconnect_after: None,
    };
    let zero = GoAway {
        reason: GoAwayReason::Shutdown.into(),
        reconnect_after: Some(mintswarm_proto::prost_types::Duration::default()),
    };
    let decoded_unset = GoAway::decode(unset.encode_to_vec().as_slice()).expect("decodes");
    let decoded_zero = GoAway::decode(zero.encode_to_vec().as_slice()).expect("decodes");
    assert_eq!(decoded_unset.reconnect_after, None);
    assert_eq!(
        decoded_zero.reconnect_after,
        Some(mintswarm_proto::prost_types::Duration::default())
    );
    assert_ne!(decoded_unset, decoded_zero);
}

#[test]
fn claim_refusal_reasons() {
    let all = [
        ClaimRefusalReason::Unspecified,
        ClaimRefusalReason::Draining,
        ClaimRefusalReason::Throttled,
        ClaimRefusalReason::Observer,
        ClaimRefusalReason::Paused,
    ];
    for reason in all {
        // Wildcard-free: a value added to the enum fails compilation here.
        match reason {
            ClaimRefusalReason::Unspecified
            | ClaimRefusalReason::Draining
            | ClaimRefusalReason::Throttled
            | ClaimRefusalReason::Observer
            | ClaimRefusalReason::Paused => {}
        }
    }
    let names = [
        "CLAIM_REFUSAL_REASON_UNSPECIFIED",
        "CLAIM_REFUSAL_REASON_DRAINING",
        "CLAIM_REFUSAL_REASON_THROTTLED",
        "CLAIM_REFUSAL_REASON_OBSERVER",
        "CLAIM_REFUSAL_REASON_PAUSED",
    ];
    let resolved: Vec<Option<ClaimRefusalReason>> = names
        .iter()
        .map(|name| ClaimRefusalReason::from_str_name(name))
        .collect();
    assert_eq!(resolved, all.map(Some));
}

#[test]
fn go_away_reasons() {
    let all = [
        GoAwayReason::Unspecified,
        GoAwayReason::Superseded,
        GoAwayReason::TokenExpired,
        GoAwayReason::Shutdown,
    ];
    for reason in all {
        // Wildcard-free: a value added to the enum fails compilation here.
        match reason {
            GoAwayReason::Unspecified
            | GoAwayReason::Superseded
            | GoAwayReason::TokenExpired
            | GoAwayReason::Shutdown => {}
        }
    }
    let names = [
        "GO_AWAY_REASON_UNSPECIFIED",
        "GO_AWAY_REASON_SUPERSEDED",
        "GO_AWAY_REASON_TOKEN_EXPIRED",
        "GO_AWAY_REASON_SHUTDOWN",
    ];
    let resolved: Vec<Option<GoAwayReason>> = names
        .iter()
        .map(|name| GoAwayReason::from_str_name(name))
        .collect();
    assert_eq!(resolved, all.map(Some));
}

#[test]
fn hello_rejection_statuses() {
    let all = [
        HelloRejectionStatus::Unspecified,
        HelloRejectionStatus::UpgradeRequired,
        HelloRejectionStatus::Revoked,
        HelloRejectionStatus::RepositoryMismatch,
    ];
    for status in all {
        // Wildcard-free: a value added to the enum fails compilation here.
        match status {
            HelloRejectionStatus::Unspecified
            | HelloRejectionStatus::UpgradeRequired
            | HelloRejectionStatus::Revoked
            | HelloRejectionStatus::RepositoryMismatch => {}
        }
    }
    let names = [
        "HELLO_REJECTION_STATUS_UNSPECIFIED",
        "HELLO_REJECTION_STATUS_UPGRADE_REQUIRED",
        "HELLO_REJECTION_STATUS_REVOKED",
        "HELLO_REJECTION_STATUS_REPOSITORY_MISMATCH",
    ];
    let resolved: Vec<Option<HelloRejectionStatus>> = names
        .iter()
        .map(|name| HelloRejectionStatus::from_str_name(name))
        .collect();
    assert_eq!(resolved, all.map(Some));
}

#[test]
fn runner_modes_and_states() {
    let modes = [
        RunnerMode::Unspecified,
        RunnerMode::Worker,
        RunnerMode::Observer,
    ];
    for mode in modes {
        // Wildcard-free: a value added to the enum fails compilation here.
        match mode {
            RunnerMode::Unspecified | RunnerMode::Worker | RunnerMode::Observer => {}
        }
    }
    let resolved: Vec<Option<RunnerMode>> = [
        "RUNNER_MODE_UNSPECIFIED",
        "RUNNER_MODE_WORKER",
        "RUNNER_MODE_OBSERVER",
    ]
    .iter()
    .map(|name| RunnerMode::from_str_name(name))
    .collect();
    assert_eq!(resolved, modes.map(Some));

    let states = [
        RunnerState::Unspecified,
        RunnerState::Online,
        RunnerState::Draining,
        RunnerState::Paused,
    ];
    for state in states {
        // Wildcard-free: a value added to the enum fails compilation here.
        match state {
            RunnerState::Unspecified
            | RunnerState::Online
            | RunnerState::Draining
            | RunnerState::Paused => {}
        }
    }
    let resolved: Vec<Option<RunnerState>> = [
        "RUNNER_STATE_UNSPECIFIED",
        "RUNNER_STATE_ONLINE",
        "RUNNER_STATE_DRAINING",
        "RUNNER_STATE_PAUSED",
    ]
    .iter()
    .map(|name| RunnerState::from_str_name(name))
    .collect();
    assert_eq!(resolved, states.map(Some));
}

#[test]
fn step_kinds() {
    let all = [
        StepKind::Unspecified,
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
    for kind in all {
        // Wildcard-free: a value added to the enum fails compilation here.
        match kind {
            StepKind::Unspecified
            | StepKind::Prepare
            | StepKind::Commit
            | StepKind::Gates
            | StepKind::Push
            | StepKind::Bundle
            | StepKind::KeepRound
            | StepKind::DependencyInputs
            | StepKind::LandingFacts
            | StepKind::TrackerEffects
            | StepKind::FileChildren => {}
        }
    }
    let names: Vec<&str> = all.iter().map(|kind| kind.as_str_name()).collect();
    assert_eq!(
        names,
        [
            "STEP_KIND_UNSPECIFIED",
            "STEP_KIND_PREPARE",
            "STEP_KIND_COMMIT",
            "STEP_KIND_GATES",
            "STEP_KIND_PUSH",
            "STEP_KIND_BUNDLE",
            "STEP_KIND_KEEP_ROUND",
            "STEP_KIND_DEPENDENCY_INPUTS",
            "STEP_KIND_LANDING_FACTS",
            "STEP_KIND_TRACKER_EFFECTS",
            "STEP_KIND_FILE_CHILDREN",
        ]
    );
}

#[test]
fn smaller_enums() {
    for state in [
        PlaneAttemptState::Unspecified,
        PlaneAttemptState::Assigned,
        PlaneAttemptState::Started,
    ] {
        // Wildcard-free: a value added to the enum fails compilation here.
        match state {
            PlaneAttemptState::Unspecified
            | PlaneAttemptState::Assigned
            | PlaneAttemptState::Started => {}
        }
    }
    for reason in [GoodbyeReason::Unspecified, GoodbyeReason::Drained] {
        // Wildcard-free: a value added to the enum fails compilation here.
        match reason {
            GoodbyeReason::Unspecified | GoodbyeReason::Drained => {}
        }
    }
    assert_eq!(
        PlaneAttemptState::from_str_name("PLANE_ATTEMPT_STATE_ASSIGNED"),
        Some(PlaneAttemptState::Assigned)
    );
    assert_eq!(
        GoodbyeReason::from_str_name("GOODBYE_REASON_DRAINED"),
        Some(GoodbyeReason::Drained)
    );
}
