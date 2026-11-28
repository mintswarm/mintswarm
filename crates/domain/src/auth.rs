//! Authentication and acting records: identities and their login bindings, sessions,
//! pre-sessions, host confirmations, GitHub links, accepted ownership states and action records.
//!
//! The records are plain data, persisted by `core`. The action kind, the client kind and the
//! link state are the auth.v1 enums `ActionKind`, `ClientKind` and `LinkState`: `crates/proto`
//! depends on this crate, so the records hold each value as the enum's proto value name, and
//! `core` parses it with the generated `from_str_name` at the store boundary.

use std::fmt;
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::ids::{ActionId, GitHubUserId, IdentityId, ProjectId, ProviderId, RevId, SessionId};

mod rules;

pub use rules::{
    DEFAULT_ABSOLUTE_LIMIT, DEFAULT_ADMIN_WINDOW, DEFAULT_IDLE_LIMIT, ONE_TIME_LIFETIME,
    OneTimeRefusal, SessionInvalid, SessionLimits, absolute_expiry,
};

macro_rules! name {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub String);
    };
}

name!(
    /// An action kind as its auth.v1 `ActionKind` value name, which `core` parses with
    /// `from_str_name`.
    ActionKindName
);
name!(
    /// A client kind as its auth.v1 `ClientKind` value name, which `core` parses with
    /// `from_str_name`.
    ClientKindName
);
name!(
    /// A GitHub link state as its auth.v1 `LinkState` value name, which `core` parses with
    /// `from_str_name`.
    LinkStateName
);
name!(
    /// The administration action kind a host confirmation or an admin window is bound to; W9
    /// fixes its values.
    AdminPurpose
);
name!(
    /// A `SecretStore` reference to a secret entry, such as a GitHub link's token entry.
    SecretRef
);
name!(
    /// The target of an action, such as the pull request or issue a write goes to.
    ActionTarget
);

/// A SHA-256 digest of a secret value, such as a session token or a one-time code. Its `Debug`
/// output never shows the bytes.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TokenHash(pub [u8; 32]);

impl TokenHash {
    /// The SHA-256 digest of `value`.
    #[must_use]
    pub fn of(value: &[u8]) -> Self {
        Self(Sha256::digest(value).into())
    }
}

impl fmt::Debug for TokenHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("TokenHash([redacted])")
    }
}

/// A person's identity in the plane.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Identity {
    pub id: IdentityId,
    pub display_name: String,
    pub created_at: SystemTime,
    /// A disabled identity cannot sign in or act.
    pub disabled: bool,
}

/// A login provider's stable subject bound to an identity; unique on (`provider`, `subject`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoginBinding {
    pub provider: ProviderId,
    /// The provider's own stable id: the GitHub numeric user id as decimal, OIDC `iss` plus
    /// `sub`, the proxy header value, or the token's name.
    pub subject: String,
    pub identity: IdentityId,
}

/// The browser flow a pre-session belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PreSessionPurpose {
    /// Signing in.
    Login,
    /// Linking a GitHub account to a signed-in identity.
    Link,
    /// A fresh sign-in of a signed-in session, which opens its admin window.
    FreshSignIn,
    /// The web fallback of the device flow.
    DeviceFallback,
}

/// The server-side state of one browser flow, single-use with a lifetime of
/// [`ONE_TIME_LIFETIME`].
///
/// Its serde form is for storage only; it is never logged, and its `Debug` output redacts
/// `state`, the PKCE verifier and the `nonce`.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreSession {
    /// The hash of the pre-session cookie's random value.
    pub cookie_hash: TokenHash,
    pub purpose: PreSessionPurpose,
    /// The OAuth `state`; absent on the web device fallback, which has no redirect.
    pub state: Option<String>,
    /// The PKCE verifier; absent where the flow uses no PKCE.
    pub pkce_verifier: Option<String>,
    /// The OIDC `nonce`; present only for an OIDC provider.
    pub nonce: Option<String>,
    /// The validated `return_to`; absent when the flow names none.
    pub return_to: Option<String>,
    /// The session the flow belongs to; present for link and fresh sign-in only.
    pub session: Option<SessionId>,
    pub created_at: SystemTime,
    /// When the pre-session was used; it is single-use.
    pub used_at: Option<SystemTime>,
}

impl fmt::Debug for PreSession {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let redacted = |value: &Option<String>| value.as_ref().map(|_| "[redacted]");
        f.debug_struct("PreSession")
            .field("cookie_hash", &self.cookie_hash)
            .field("purpose", &self.purpose)
            .field("state", &redacted(&self.state))
            .field("pkce_verifier", &redacted(&self.pkce_verifier))
            .field("nonce", &redacted(&self.nonce))
            .field("return_to", &self.return_to)
            .field("session", &self.session)
            .field("created_at", &self.created_at)
            .field("used_at", &self.used_at)
            .finish()
    }
}

/// A one-time code a host operator confirms an administration action with, bound to a
/// session and a purpose; single-use, and valid until `expires_at`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostConfirmation {
    /// The hash of the one-time code.
    pub code_hash: TokenHash,
    pub session: SessionId,
    /// The administration action kind the code confirms.
    pub purpose: AdminPurpose,
    pub created_at: SystemTime,
    /// [`ONE_TIME_LIFETIME`] after issue; the code's own lifetime, separate from the admin
    /// window.
    pub expires_at: SystemTime,
    /// When the code was used; it is single-use.
    pub used_at: Option<SystemTime>,
}

/// The branch an accepted state is kept for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AcceptedBranch {
    /// The repository's default branch.
    Default,
    /// The branch that holds the project's settings.
    Settings,
}

/// How ownership changes are accepted.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OwnershipChanges {
    /// GitHub's own branch rules decide.
    Github,
    /// A change counts only once reviewed by an owner.
    Reviewed,
}

/// The ownership policy an accepted state moved under.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AcceptancePolicy {
    pub changes: OwnershipChanges,
    /// Whether an owner may bypass the review of their own change.
    pub bypass_by_owner: bool,
}

/// How an accepted state was first accepted, with who performed it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AcceptanceOrigin {
    /// At onboarding, by the host operator running `mintswarm-server setup ownership`; a host
    /// command has no session identity, so the operator is recorded by name.
    Onboarding { operator: String },
    /// By an "accept owners" action of an identity.
    AcceptOwners { by: IdentityId },
}

/// The accepted ownership state of one branch of a repository; one per (`project`, `branch`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AcceptedState {
    pub project: ProjectId,
    pub branch: AcceptedBranch,
    /// The accepted commit.
    pub commit: RevId,
    pub origin: AcceptanceOrigin,
    /// The policy in force when the state last moved.
    pub policy: AcceptancePolicy,
    /// The first commit that did not count; `None` is no drift.
    pub drift: Option<RevId>,
}

/// An identity's link to a GitHub account; at most one per identity. The tokens live only in
/// the `SecretStore`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitHubLink {
    pub identity: IdentityId,
    /// The GitHub user id, the link's key.
    pub github_user: GitHubUserId,
    /// The current GitHub login.
    pub login: String,
    /// The id of the GitHub App the link's token was issued by.
    pub app_id: u64,
    /// The `SecretStore` reference of the link's token entry.
    pub token_ref: SecretRef,
    /// The time of the last interactive authorisation by this GitHub user id.
    pub authorised_at: SystemTime,
    pub state: LinkStateName,
}

/// A signed-in session of an identity.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Session {
    pub id: SessionId,
    /// The hash of the session token.
    pub token_hash: TokenHash,
    pub identity: IdentityId,
    pub provider: ProviderId,
    pub client_kind: ClientKindName,
    /// The requesting host of a device-flow session; absent when the login named none.
    pub host: Option<String>,
    pub created_at: SystemTime,
    pub last_seen: SystemTime,
    /// How long the session may stay unused.
    pub idle_limit: Duration,
    /// When the session ends whatever its activity, capped by a token provider's token expiry;
    /// see [`absolute_expiry`].
    pub absolute_expiry: SystemTime,
    pub revoked_at: Option<SystemTime>,
    /// When the session last signed in afresh.
    pub login_fresh_at: Option<SystemTime>,
    /// When the admin window last opened or was reset; `None` is never opened.
    pub admin_window_at: Option<SystemTime>,
    /// The confirmed purpose of a window a host confirmation opened; `None` for a window a
    /// fresh sign-in opened.
    pub admin_purpose: Option<AdminPurpose>,
    /// The last admission check.
    pub admitted_at: Option<SystemTime>,
    /// False for token-provider sessions.
    pub interactive: bool,
}

/// The outcome of an action.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionOutcome {
    /// The write was started.
    Started,
    /// The write succeeded.
    Done,
    /// The plane refused the action.
    Refused,
    /// The write failed.
    Failed,
}

/// One GitHub write the plane makes as a user; an audit and correlation record, not a decision
/// record.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActionRecord {
    pub id: ActionId,
    pub identity: IdentityId,
    pub session: SessionId,
    pub github_user: GitHubUserId,
    pub action_kind: ActionKindName,
    pub target: ActionTarget,
    pub requested_at: SystemTime,
    pub outcome: ActionOutcome,
    /// The GitHub object id the write returned; absent until it returns one.
    pub github_object_id: Option<String>,
    /// The delivery id of the matched webhook echo; absent until matched.
    pub echo_delivery_id: Option<String>,
}
