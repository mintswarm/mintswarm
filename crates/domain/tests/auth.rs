//! Auth records and their session, one-time and admin window rules.

#![cfg(test)]

use std::time::{Duration, SystemTime};

use mintswarm_domain::auth::{
    AcceptanceOrigin, AcceptancePolicy, AcceptedBranch, AcceptedState, ActionKindName,
    ActionOutcome, ActionRecord, ActionTarget, AdminPurpose, ClientKindName, DEFAULT_ADMIN_WINDOW,
    GitHubLink, HostConfirmation, Identity, LinkStateName, LoginBinding, ONE_TIME_LIFETIME,
    OneTimeRefusal, OwnershipChanges, PreSession, PreSessionPurpose, SecretRef, Session,
    SessionInvalid, SessionLimits, TokenHash, absolute_expiry,
};
use mintswarm_domain::ids::{
    ActionId, GitHubUserId, IdentityId, ProjectId, ProviderId, RevId, SessionId,
};
use serde::Serialize;
use serde::de::DeserializeOwned;

const MINUTE: Duration = Duration::from_secs(60);
const HOUR: Duration = Duration::from_secs(60 * 60);
const DAY: Duration = Duration::from_secs(24 * 60 * 60);

fn t0() -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000)
}

fn round_trip<T: Serialize + DeserializeOwned + PartialEq + std::fmt::Debug>(value: &T) {
    let json = serde_json::to_string(value).unwrap();
    let back: T = serde_json::from_str(&json).unwrap();
    assert_eq!(&back, value);
}

fn identity_id() -> IdentityId {
    IdentityId("identity-1".to_owned())
}

fn session_id() -> SessionId {
    SessionId("session-1".to_owned())
}

fn session() -> Session {
    let limits = SessionLimits::default();
    Session {
        id: session_id(),
        token_hash: TokenHash::of(b"session token"),
        identity: identity_id(),
        provider: ProviderId("github".to_owned()),
        client_kind: ClientKindName("CLIENT_KIND_WEB".to_owned()),
        host: Some("laptop".to_owned()),
        created_at: t0(),
        last_seen: t0(),
        idle_limit: limits.idle,
        absolute_expiry: absolute_expiry(t0(), &limits, None),
        revoked_at: None,
        login_fresh_at: Some(t0()),
        admin_window_at: None,
        admin_purpose: None,
        admitted_at: Some(t0()),
        interactive: true,
    }
}

fn pre_session() -> PreSession {
    PreSession {
        cookie_hash: TokenHash::of(b"cookie"),
        purpose: PreSessionPurpose::Login,
        state: Some("state-secret".to_owned()),
        pkce_verifier: Some("verifier-secret".to_owned()),
        nonce: Some("nonce-secret".to_owned()),
        return_to: Some("/jobs".to_owned()),
        session: None,
        created_at: t0(),
        used_at: None,
    }
}

fn host_confirmation() -> HostConfirmation {
    HostConfirmation {
        code_hash: TokenHash::of(b"123456"),
        session: session_id(),
        purpose: AdminPurpose("accept_owners".to_owned()),
        created_at: t0(),
        expires_at: HostConfirmation::expiry_for(t0()),
        used_at: None,
    }
}

#[test]
fn records_have_design_fields() {
    round_trip(&Identity {
        id: identity_id(),
        display_name: "Ada".to_owned(),
        created_at: t0(),
        disabled: false,
    });
    round_trip(&LoginBinding {
        provider: ProviderId("github".to_owned()),
        subject: "583231".to_owned(),
        identity: identity_id(),
    });
    round_trip(&pre_session());
    round_trip(&PreSession {
        purpose: PreSessionPurpose::DeviceFallback,
        state: None,
        pkce_verifier: None,
        nonce: None,
        return_to: None,
        session: Some(session_id()),
        used_at: Some(t0()),
        ..pre_session()
    });
    round_trip(&host_confirmation());
    for origin in [
        AcceptanceOrigin::Onboarding {
            operator: "root".to_owned(),
        },
        AcceptanceOrigin::AcceptOwners { by: identity_id() },
    ] {
        round_trip(&AcceptedState {
            project: ProjectId("project-1".to_owned()),
            branch: AcceptedBranch::Settings,
            commit: RevId("a".repeat(40)),
            origin,
            policy: AcceptancePolicy {
                changes: OwnershipChanges::Reviewed,
                bypass_by_owner: true,
            },
            drift: Some(RevId("b".repeat(40))),
        });
    }
    round_trip(&GitHubLink {
        identity: identity_id(),
        github_user: GitHubUserId(583_231),
        login: "octocat".to_owned(),
        app_id: 12_345,
        token_ref: SecretRef("github-link/identity-1".to_owned()),
        authorised_at: t0(),
        state: LinkStateName("LINK_STATE_ACTIVE".to_owned()),
    });
    round_trip(&session());
    round_trip(&ActionRecord {
        id: ActionId("action-1".to_owned()),
        identity: identity_id(),
        session: session_id(),
        github_user: GitHubUserId(583_231),
        action_kind: ActionKindName("ACTION_KIND_MERGE".to_owned()),
        target: ActionTarget("pull/12".to_owned()),
        requested_at: t0(),
        outcome: ActionOutcome::Started,
        github_object_id: None,
        echo_delivery_id: None,
    });
}

#[test]
fn pre_session_debug_redacts_secrets() {
    let debug = format!("{:?}", pre_session());
    for secret in ["state-secret", "verifier-secret", "nonce-secret"] {
        assert!(!debug.contains(secret), "{secret} in {debug}");
    }
    let hash = TokenHash::of(b"cookie");
    let hex: String = hash.0.iter().map(|b| format!("{b:02x}")).collect();
    assert!(!format!("{hash:?}").contains(&hex));
}

#[test]
fn proto_names_serialise_as_bare_names() {
    let client = ClientKindName("CLIENT_KIND_CLI".to_owned());
    let state = LinkStateName("LINK_STATE_ACTIVE".to_owned());
    let action = ActionKindName("ACTION_KIND_MERGE".to_owned());
    assert_eq!(
        serde_json::to_string(&client).unwrap(),
        "\"CLIENT_KIND_CLI\""
    );
    assert_eq!(
        serde_json::to_string(&state).unwrap(),
        "\"LINK_STATE_ACTIVE\""
    );
    assert_eq!(
        serde_json::to_string(&action).unwrap(),
        "\"ACTION_KIND_MERGE\""
    );
    round_trip(&client);
    round_trip(&state);
    round_trip(&action);
}

#[test]
fn action_outcome_has_four_values() {
    for outcome in [
        ActionOutcome::Started,
        ActionOutcome::Done,
        ActionOutcome::Refused,
        ActionOutcome::Failed,
    ] {
        let name = match outcome {
            ActionOutcome::Started => "started",
            ActionOutcome::Done => "done",
            ActionOutcome::Refused => "refused",
            ActionOutcome::Failed => "failed",
        };
        assert_eq!(
            serde_json::to_string(&outcome).unwrap(),
            format!("\"{name}\"")
        );
        round_trip(&outcome);
    }
}

#[test]
fn session_idle_boundary() {
    let session = session();
    assert_eq!(session.validity(t0() + 23 * HOUR + 59 * MINUTE), Ok(()));
    assert_eq!(session.validity(t0() + DAY), Err(SessionInvalid::Idle));
}

#[test]
fn session_absolute_limit() {
    let mut session = session();
    let at = t0() + 14 * DAY;
    session.last_seen = at;
    assert_eq!(session.validity(at - MINUTE), Ok(()));
    assert_eq!(session.validity(at), Err(SessionInvalid::Expired));
}

#[test]
fn absolute_expiry_capped_by_token() {
    let limits = SessionLimits::default();
    let earlier = t0() + 3 * DAY;
    let later = t0() + 30 * DAY;
    assert_eq!(absolute_expiry(t0(), &limits, Some(earlier)), earlier);
    assert_eq!(absolute_expiry(t0(), &limits, Some(later)), t0() + 14 * DAY);
    assert_eq!(absolute_expiry(t0(), &limits, None), t0() + 14 * DAY);

    let mut session = session();
    session.absolute_expiry = absolute_expiry(t0(), &limits, Some(earlier));
    session.last_seen = earlier;
    assert_eq!(session.validity(earlier - MINUTE), Ok(()));
    assert_eq!(session.validity(earlier), Err(SessionInvalid::Expired));
}

#[test]
fn revoked_session_invalid() {
    let mut session = session();
    session.revoked_at = Some(t0());
    assert_eq!(session.validity(t0()), Err(SessionInvalid::Revoked));
}

#[test]
fn session_validity_order() {
    let mut session = session();
    let late = t0() + 30 * DAY;
    assert_eq!(session.validity(late), Err(SessionInvalid::Idle));
    session.revoked_at = Some(t0());
    assert_eq!(session.validity(late), Err(SessionInvalid::Revoked));
}

#[test]
fn session_clock_stepped_back_counts_as_fresh() {
    assert_eq!(session().validity(t0() - HOUR), Ok(()));
}

#[test]
fn pre_session_one_time() {
    let pre = pre_session();
    assert_eq!(
        pre.check(t0() + 9 * MINUTE + Duration::from_secs(59)),
        Ok(())
    );
    assert_eq!(
        pre.check(t0() + ONE_TIME_LIFETIME),
        Err(OneTimeRefusal::Expired)
    );
    assert_eq!(
        pre.check(t0() + 10 * MINUTE + Duration::from_secs(1)),
        Err(OneTimeRefusal::Expired)
    );
    let used = PreSession {
        used_at: Some(t0()),
        ..pre_session()
    };
    assert_eq!(used.check(t0()), Err(OneTimeRefusal::Used));
}

#[test]
fn host_confirmation_one_time() {
    let confirmation = host_confirmation();
    assert_eq!(confirmation.expires_at, t0() + 10 * MINUTE);
    assert_eq!(
        confirmation.check(t0() + 9 * MINUTE + Duration::from_secs(59)),
        Ok(())
    );
    assert_eq!(
        confirmation.check(t0() + 10 * MINUTE),
        Err(OneTimeRefusal::Expired)
    );
    assert_eq!(
        confirmation.check(t0() + 10 * MINUTE + Duration::from_secs(1)),
        Err(OneTimeRefusal::Expired)
    );
    let used = HostConfirmation {
        used_at: Some(t0()),
        ..host_confirmation()
    };
    assert_eq!(used.check(t0()), Err(OneTimeRefusal::Used));
}

#[test]
fn rule_errors_are_std_errors() {
    let errors: [&dyn std::error::Error; 5] = [
        &SessionInvalid::Revoked,
        &SessionInvalid::Idle,
        &SessionInvalid::Expired,
        &OneTimeRefusal::Used,
        &OneTimeRefusal::Expired,
    ];
    for error in errors {
        assert!(!error.to_string().is_empty());
        assert!(error.source().is_none());
    }
}

fn purpose(name: &str) -> AdminPurpose {
    AdminPurpose(name.to_owned())
}

#[test]
fn admin_window_default_length() {
    assert_eq!(DEFAULT_ADMIN_WINDOW, 2 * HOUR);
    let mut session = session();
    let any = purpose("accept_owners");
    assert!(!session.admin_window_open(t0(), DEFAULT_ADMIN_WINDOW, &any));
    session.admin_window_at = Some(t0());
    assert!(session.admin_window_open(t0(), DEFAULT_ADMIN_WINDOW, &any));
    assert!(session.admin_window_open(t0() + 2 * HOUR, DEFAULT_ADMIN_WINDOW, &any));
    assert!(!session.admin_window_open(
        t0() + 2 * HOUR + Duration::from_secs(1),
        DEFAULT_ADMIN_WINDOW,
        &any
    ));
}

#[test]
fn admin_window_configured_length() {
    let mut session = session();
    session.admin_window_at = Some(t0());
    let length = 30 * MINUTE;
    let any = purpose("accept_owners");
    assert!(session.admin_window_open(t0() + length, length, &any));
    assert!(!session.admin_window_open(t0() + length + Duration::from_secs(1), length, &any));
}

#[test]
fn admin_window_purpose() {
    let mut session = session();
    session.admin_window_at = Some(t0());
    assert!(session.admin_window_open(t0(), DEFAULT_ADMIN_WINDOW, &purpose("a")));
    assert!(session.admin_window_open(t0(), DEFAULT_ADMIN_WINDOW, &purpose("b")));
    session.admin_purpose = Some(purpose("a"));
    assert!(session.admin_window_open(t0(), DEFAULT_ADMIN_WINDOW, &purpose("a")));
    assert!(!session.admin_window_open(t0(), DEFAULT_ADMIN_WINDOW, &purpose("b")));
}
