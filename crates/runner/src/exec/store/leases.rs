//! Leases on named resources, design §12: job leases and the runner's local resource leases.
//!
//! A lease is live while now is before its `expires_at`. From `expires_at` on it is expired:
//! its holder cannot renew it, and anyone can acquire it.

use std::fmt;

use mintswarm_domain::ids::{AttemptId, JobId};
use rusqlite::{Connection, OptionalExtension, params};

use super::{StoreError, sql};

/// A resource a lease is held on.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum LeaseResource {
    /// A job, `job:<id>`.
    Job(JobId),
    /// An attempt, `attempt:<id>`.
    Attempt(AttemptId),
    /// A worktree, `worktree:<name>`.
    Worktree(String),
    /// A snapshot, `snapshot:<name>`.
    Snapshot(String),
    /// Fetching into the trusted store, `store-fetch`.
    StoreFetch,
}

impl fmt::Display for LeaseResource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LeaseResource::Job(id) => write!(f, "job:{}", id.0),
            LeaseResource::Attempt(id) => write!(f, "attempt:{}", id.0),
            LeaseResource::Worktree(name) => write!(f, "worktree:{name}"),
            LeaseResource::Snapshot(name) => write!(f, "snapshot:{name}"),
            LeaseResource::StoreFetch => f.write_str("store-fetch"),
        }
    }
}

/// A live lease.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Lease {
    /// The resource it is held on.
    pub resource: LeaseResource,
    /// Who holds it.
    pub holder: String,
    /// When it expires, in unix milliseconds.
    pub expires_at: i64,
}

/// The deadline `ttl_ms` after `now_ms`; the time to live must be positive.
fn deadline(now_ms: i64, ttl_ms: i64) -> Result<i64, StoreError> {
    if ttl_ms <= 0 {
        return Err(StoreError::LeaseTtl(ttl_ms));
    }
    Ok(now_ms.saturating_add(ttl_ms))
}

/// Take the lease on `resource` for `holder` when there is none or it has expired.
pub(super) fn acquire(
    conn: &Connection,
    now_ms: i64,
    resource: &LeaseResource,
    holder: &str,
    ttl_ms: i64,
) -> Result<Lease, StoreError> {
    let expires_at = deadline(now_ms, ttl_ms)?;
    let key = resource.to_string();
    let changed = conn
        .execute(
            "INSERT INTO leases (resource, holder, expires_at) VALUES (?1, ?2, ?3) \
             ON CONFLICT (resource) DO UPDATE \
             SET holder = excluded.holder, expires_at = excluded.expires_at \
             WHERE leases.expires_at <= ?4",
            params![key, holder, expires_at, now_ms],
        )
        .map_err(sql("cannot acquire a lease"))?;
    if changed == 0 {
        return Err(StoreError::LeaseHeld(key));
    }
    Ok(Lease {
        resource: resource.clone(),
        holder: holder.to_owned(),
        expires_at,
    })
}

/// Extend `holder`'s live lease on `resource` to `ttl_ms` after `now_ms`.
pub(super) fn renew(
    conn: &Connection,
    now_ms: i64,
    resource: &LeaseResource,
    holder: &str,
    ttl_ms: i64,
) -> Result<Lease, StoreError> {
    let expires_at = deadline(now_ms, ttl_ms)?;
    let key = resource.to_string();
    let changed = conn
        .execute(
            "UPDATE leases SET expires_at = ?1 \
             WHERE resource = ?2 AND holder = ?3 AND expires_at > ?4",
            params![expires_at, key, holder, now_ms],
        )
        .map_err(sql("cannot renew a lease"))?;
    if changed == 0 {
        return Err(StoreError::LeaseNotHeld(key));
    }
    Ok(Lease {
        resource: resource.clone(),
        holder: holder.to_owned(),
        expires_at,
    })
}

/// Remove `holder`'s lease on `resource`, live or expired.
pub(super) fn release(
    conn: &Connection,
    resource: &LeaseResource,
    holder: &str,
) -> Result<(), StoreError> {
    let key = resource.to_string();
    let changed = conn
        .execute(
            "DELETE FROM leases WHERE resource = ?1 AND holder = ?2",
            params![key, holder],
        )
        .map_err(sql("cannot release a lease"))?;
    if changed == 0 {
        return Err(StoreError::LeaseNotHeld(key));
    }
    Ok(())
}

/// The live lease on `resource` at `now_ms`.
pub(super) fn lease(
    conn: &Connection,
    now_ms: i64,
    resource: &LeaseResource,
) -> Result<Option<Lease>, StoreError> {
    let found = conn
        .query_row(
            "SELECT holder, expires_at FROM leases WHERE resource = ?1 AND expires_at > ?2",
            params![resource.to_string(), now_ms],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
        )
        .optional()
        .map_err(sql("cannot read a lease"))?;
    Ok(found.map(|(holder, expires_at)| Lease {
        resource: resource.clone(),
        holder,
        expires_at,
    }))
}
