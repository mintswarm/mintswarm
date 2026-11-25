//! Failures of an outside command that the runner's classifier did not recognise, beyond design
//! §12: one row per adapter and signature, counted, for a later step to learn from.
//!
//! The store takes plain strings and knows nothing of the adapters: the caller gives the
//! adapter's name, the signature and an excerpt it has already redacted.

use rusqlite::{Connection, params};

use super::{StoreError, sql};

/// A failure to record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewUnrecognised {
    /// The adapter that saw it, such as `git`.
    pub adapter: String,
    /// What tells this failure apart from others of the adapter, such as a hash of its excerpt.
    pub signature: String,
    /// The start of what the command wrote, redacted by the caller; never a credential.
    pub excerpt: String,
    /// The command's exit status, in our words.
    pub status: String,
}

/// A recorded failure.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unrecognised {
    /// The adapter that saw it.
    pub adapter: String,
    /// Its signature.
    pub signature: String,
    /// The latest excerpt recorded for it.
    pub excerpt: String,
    /// The latest status recorded for it.
    pub status: String,
    /// How many times it was recorded.
    pub count: i64,
    /// When it was first recorded, in unix milliseconds.
    pub first_seen_at: i64,
    /// When it was last recorded, in unix milliseconds.
    pub last_seen_at: i64,
}

/// Insert `failure` with a count of 1, or, when its adapter and signature are recorded already,
/// add 1 to the count and keep the latest excerpt, status and time.
pub(super) fn record(
    conn: &Connection,
    now_ms: i64,
    failure: &NewUnrecognised,
) -> Result<(), StoreError> {
    conn.execute(
        "INSERT INTO unrecognised_failures \
         (adapter, signature, excerpt, status, count, first_seen_at, last_seen_at) \
         VALUES (?1, ?2, ?3, ?4, 1, ?5, ?5) \
         ON CONFLICT (adapter, signature) DO UPDATE \
         SET count = count + 1, excerpt = excluded.excerpt, status = excluded.status, \
         last_seen_at = excluded.last_seen_at",
        params![
            failure.adapter,
            failure.signature,
            failure.excerpt,
            failure.status,
            now_ms
        ],
    )
    .map_err(sql("cannot record an unrecognised failure"))?;
    Ok(())
}

/// The failures recorded for `adapter`, the most often seen first; ties by signature.
pub(super) fn list(conn: &Connection, adapter: &str) -> Result<Vec<Unrecognised>, StoreError> {
    let mut statement = conn
        .prepare(
            "SELECT adapter, signature, excerpt, status, count, first_seen_at, last_seen_at \
             FROM unrecognised_failures WHERE adapter = ?1 \
             ORDER BY count DESC, signature",
        )
        .map_err(sql("cannot read the unrecognised failures"))?;
    let rows = statement
        .query_map([adapter], |row| {
            Ok(Unrecognised {
                adapter: row.get(0)?,
                signature: row.get(1)?,
                excerpt: row.get(2)?,
                status: row.get(3)?,
                count: row.get(4)?,
                first_seen_at: row.get(5)?,
                last_seen_at: row.get(6)?,
            })
        })
        .map_err(sql("cannot read the unrecognised failures"))?;
    rows.collect::<Result<_, _>>()
        .map_err(sql("cannot read the unrecognised failures"))
}
