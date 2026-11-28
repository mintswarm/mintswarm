//! The session limits, the one-time lifetime of pre-sessions and host confirmations, and the
//! admin window rule.
//!
//! Every rule takes `now` and its limits, so the server config can set them per provider.
//! Elapsed time saturates at zero when `now` is earlier than the stored instant, so a clock
//! stepped back counts a record as fresh rather than failing.

use std::fmt;
use std::time::{Duration, SystemTime};

use super::{AdminPurpose, HostConfirmation, PreSession, Session};

/// How long a session may stay unused by default: 24 hours.
pub const DEFAULT_IDLE_LIMIT: Duration = Duration::from_secs(24 * 60 * 60);

/// How long a session lasts after it was created by default, whatever its activity: 14 days.
pub const DEFAULT_ABSOLUTE_LIMIT: Duration = Duration::from_secs(14 * 24 * 60 * 60);

/// The lifetime of a pre-session and of a host confirmation's code: 10 minutes.
pub const ONE_TIME_LIFETIME: Duration = Duration::from_secs(10 * 60);

/// How long an admin window stays open by default: 2 hours.
pub const DEFAULT_ADMIN_WINDOW: Duration = Duration::from_secs(2 * 60 * 60);

/// The idle and absolute limits a session is created with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SessionLimits {
    /// How long a session may stay unused.
    pub idle: Duration,
    /// How long a session lasts after it was created, whatever its activity.
    pub absolute: Duration,
}

impl Default for SessionLimits {
    fn default() -> Self {
        Self {
            idle: DEFAULT_IDLE_LIMIT,
            absolute: DEFAULT_ABSOLUTE_LIMIT,
        }
    }
}

/// The absolute expiry of a session created at `created_at`: the earlier of `created_at` plus
/// the absolute limit and the expiry of the token-provider token it was created with, if any.
///
/// When `created_at` plus the limit cannot be represented, the session expires at
/// `created_at`, so an unrepresentable limit fails closed.
#[must_use]
pub fn absolute_expiry(
    created_at: SystemTime,
    limits: &SessionLimits,
    token_expiry: Option<SystemTime>,
) -> SystemTime {
    let limit = created_at
        .checked_add(limits.absolute)
        .unwrap_or(created_at);
    match token_expiry {
        Some(token) if token < limit => token,
        _ => limit,
    }
}

/// Why a session is not valid.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionInvalid {
    /// The session was revoked.
    Revoked,
    /// The session stayed unused for its idle limit or longer.
    Idle,
    /// The session reached its absolute expiry.
    Expired,
}

impl fmt::Display for SessionInvalid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Revoked => "session revoked",
            Self::Idle => "session idle for too long",
            Self::Expired => "session expired",
        })
    }
}

impl std::error::Error for SessionInvalid {}

/// Why a pre-session or a host confirmation is refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OneTimeRefusal {
    /// It was already used.
    Used,
    /// Its lifetime is over.
    Expired,
}

impl fmt::Display for OneTimeRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Used => "already used",
            Self::Expired => "expired",
        })
    }
}

impl std::error::Error for OneTimeRefusal {}

/// The time from `then` to `now`, zero when `now` is earlier.
fn elapsed(then: SystemTime, now: SystemTime) -> Duration {
    now.duration_since(then).unwrap_or(Duration::ZERO)
}

impl Session {
    /// Whether the session is valid at `now`. The checks run in this order and the first that
    /// matches is returned:
    ///
    /// 1. [`SessionInvalid::Revoked`] when `revoked_at` is set;
    /// 2. [`SessionInvalid::Idle`] when the time since `last_seen` is `idle_limit` or more;
    /// 3. [`SessionInvalid::Expired`] when `now` is at or after `absolute_expiry`.
    ///
    /// # Errors
    ///
    /// The first [`SessionInvalid`] that matches.
    pub fn validity(&self, now: SystemTime) -> Result<(), SessionInvalid> {
        if self.revoked_at.is_some() {
            return Err(SessionInvalid::Revoked);
        }
        if elapsed(self.last_seen, now) >= self.idle_limit {
            return Err(SessionInvalid::Idle);
        }
        if now >= self.absolute_expiry {
            return Err(SessionInvalid::Expired);
        }
        Ok(())
    }

    /// Whether the admin window is open at `now` for `purpose`: `admin_window_at` is set and no
    /// more than `length` ago, inclusive at exactly `length`.
    ///
    /// A window with no `admin_purpose`, opened by a fresh sign-in, is open for every purpose;
    /// a window a host confirmation opened is open only for its own purpose. W9 fixes the
    /// purpose values.
    #[must_use]
    pub fn admin_window_open(
        &self,
        now: SystemTime,
        length: Duration,
        purpose: &AdminPurpose,
    ) -> bool {
        self.admin_window_at
            .is_some_and(|at| elapsed(at, now) <= length)
            && self
                .admin_purpose
                .as_ref()
                .is_none_or(|confirmed| confirmed == purpose)
    }
}

impl PreSession {
    /// Whether the pre-session may be used at `now`.
    ///
    /// It is refused as [`OneTimeRefusal::Used`] when `used_at` is set, and as
    /// [`OneTimeRefusal::Expired`] when it is [`ONE_TIME_LIFETIME`] old or older. Exactly
    /// 10 minutes is refused, so the boundary is the same as a host confirmation's
    /// `now >= expires_at`.
    ///
    /// # Errors
    ///
    /// The [`OneTimeRefusal`] that applies, `Used` first.
    pub fn check(&self, now: SystemTime) -> Result<(), OneTimeRefusal> {
        if self.used_at.is_some() {
            return Err(OneTimeRefusal::Used);
        }
        if elapsed(self.created_at, now) >= ONE_TIME_LIFETIME {
            return Err(OneTimeRefusal::Expired);
        }
        Ok(())
    }
}

impl HostConfirmation {
    /// The expiry of a code issued at `created_at`: [`ONE_TIME_LIFETIME`] later, or
    /// `created_at` itself when that cannot be represented, so it fails closed.
    #[must_use]
    pub fn expiry_for(created_at: SystemTime) -> SystemTime {
        created_at
            .checked_add(ONE_TIME_LIFETIME)
            .unwrap_or(created_at)
    }

    /// Whether the code may be used at `now`.
    ///
    /// It is refused as [`OneTimeRefusal::Used`] when `used_at` is set, and as
    /// [`OneTimeRefusal::Expired`] when `now` is at or after `expires_at`. Exactly 10 minutes
    /// after issue is refused, the same boundary as a pre-session's.
    ///
    /// # Errors
    ///
    /// The [`OneTimeRefusal`] that applies, `Used` first.
    pub fn check(&self, now: SystemTime) -> Result<(), OneTimeRefusal> {
        if self.used_at.is_some() {
            return Err(OneTimeRefusal::Used);
        }
        if now >= self.expires_at {
            return Err(OneTimeRefusal::Expired);
        }
        Ok(())
    }
}
