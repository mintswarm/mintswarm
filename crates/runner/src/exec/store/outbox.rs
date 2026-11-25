//! The outbox of design §12: external effects not yet acknowledged, one row per effect, in
//! order per job by `effect_id`.
//!
//! The store's [`OutboxMode`] decides the state an inserted effect gets, and [`due`] is the only
//! read meant for a sender: it never returns a `suppressed` row.

use std::path::Path;

use mintswarm_domain::ids::JobId;
use rusqlite::{Connection, OptionalExtension, Row, params};

use super::{BlobId, OutboxMode, StoreError, blobs, sql};

/// What an outbox row does on the outside.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum EffectKind {
    /// A comment on an issue or pull request.
    Comment,
    /// An artifact on an issue.
    Artifact,
    /// Assigning an issue or pull request.
    Assign,
    /// Releasing an assignment.
    Release,
    /// A review request on a pull request.
    ReviewRequest,
    /// Opening a pull request.
    OpenPr,
    /// Marking a draft pull request ready for review.
    MarkReady,
    /// Setting an issue's type.
    SetIssueType,
    /// Filing a child issue.
    FileChild,
    /// Pushing a branch.
    Push,
    /// An event or record awaiting `PlaneLink::acked`.
    LinkEvent,
}

impl EffectKind {
    /// Every kind, in the order of design §12.
    pub const ALL: [EffectKind; 11] = [
        EffectKind::Comment,
        EffectKind::Artifact,
        EffectKind::Assign,
        EffectKind::Release,
        EffectKind::ReviewRequest,
        EffectKind::OpenPr,
        EffectKind::MarkReady,
        EffectKind::SetIssueType,
        EffectKind::FileChild,
        EffectKind::Push,
        EffectKind::LinkEvent,
    ];

    /// The kind as stored in `outbox.kind`.
    pub fn as_str(self) -> &'static str {
        match self {
            EffectKind::Comment => "comment",
            EffectKind::Artifact => "artifact",
            EffectKind::Assign => "assign",
            EffectKind::Release => "release",
            EffectKind::ReviewRequest => "review_request",
            EffectKind::OpenPr => "open_pr",
            EffectKind::MarkReady => "mark_ready",
            EffectKind::SetIssueType => "set_issue_type",
            EffectKind::FileChild => "file_child",
            EffectKind::Push => "push",
            EffectKind::LinkEvent => "link_event",
        }
    }

    /// The kind stored as `text`.
    fn parse(text: &str) -> Result<EffectKind, StoreError> {
        EffectKind::ALL
            .into_iter()
            .find(|kind| kind.as_str() == text)
            .ok_or_else(|| StoreError::Value {
                column: "outbox.kind",
                value: text.to_owned(),
            })
    }
}

/// Where an outbox row is on its way out.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum EffectState {
    /// Not sent yet.
    Pending,
    /// Sent, not yet acknowledged.
    Sent,
    /// Sent and acknowledged.
    Acked,
    /// Given up on.
    Failed,
    /// Rendered and kept, but never to be sent.
    Suppressed,
}

impl EffectState {
    /// Every state.
    pub const ALL: [EffectState; 5] = [
        EffectState::Pending,
        EffectState::Sent,
        EffectState::Acked,
        EffectState::Failed,
        EffectState::Suppressed,
    ];

    /// The state as stored in `outbox.state`.
    pub fn as_str(self) -> &'static str {
        match self {
            EffectState::Pending => "pending",
            EffectState::Sent => "sent",
            EffectState::Acked => "acked",
            EffectState::Failed => "failed",
            EffectState::Suppressed => "suppressed",
        }
    }

    /// The state stored as `text`.
    fn parse(text: &str) -> Result<EffectState, StoreError> {
        EffectState::ALL
            .into_iter()
            .find(|state| state.as_str() == text)
            .ok_or_else(|| StoreError::Value {
                column: "outbox.state",
                value: text.to_owned(),
            })
    }
}

/// An effect to insert into the outbox.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewEffect {
    /// The job the effect belongs to.
    pub job_id: JobId,
    /// What the effect does.
    pub kind: EffectKind,
    /// The effect's key, unique within the job; a sender finds a sent effect by it.
    pub effect_key: String,
    /// The rendered effect, stored as a blob.
    pub payload: Vec<u8>,
}

/// One outbox row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Effect {
    /// The row's id, which orders the rows of a job.
    pub effect_id: i64,
    /// The job the effect belongs to.
    pub job_id: JobId,
    /// What the effect does.
    pub kind: EffectKind,
    /// The effect's key, unique within the job.
    pub effect_key: String,
    /// The blob holding the rendered effect.
    pub payload_blob: BlobId,
    /// Where the row is on its way out.
    pub state: EffectState,
    /// How many times sending it was tried.
    pub attempts: i64,
    /// When it may next be sent, in unix milliseconds; `None` for at once.
    pub next_at: Option<i64>,
    /// What sending it gave, as JSON.
    pub result: Option<String>,
}

/// The columns [`from_row`] reads, in its order.
const COLUMNS: &str =
    "effect_id, job_id, kind, effect_key, payload_blob, state, attempts, next_at, result";

/// Insert `effect`, or return the id of the row with its `(job_id, effect_key)`.
pub(super) fn insert(
    conn: &Connection,
    root: &Path,
    mode: OutboxMode,
    effect: &NewEffect,
) -> Result<i64, StoreError> {
    let existing = conn
        .query_row(
            "SELECT effect_id FROM outbox WHERE job_id = ?1 AND effect_key = ?2",
            params![effect.job_id.0, effect.effect_key],
            |row| row.get(0),
        )
        .optional()
        .map_err(sql("cannot look up an outbox effect"))?;
    if let Some(effect_id) = existing {
        return Ok(effect_id);
    }
    let payload = blobs::put(root, &effect.payload)?;
    let state = match mode {
        OutboxMode::Live => EffectState::Pending,
        OutboxMode::Suppressed => EffectState::Suppressed,
    };
    conn.execute(
        "INSERT INTO outbox (job_id, kind, effect_key, payload_blob, state) \
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            effect.job_id.0,
            effect.kind.as_str(),
            effect.effect_key,
            payload.as_str(),
            state.as_str(),
        ],
    )
    .map_err(sql("cannot insert an outbox effect"))?;
    Ok(conn.last_insert_rowid())
}

/// The row `effect_id`, whatever its state.
pub(super) fn effect(conn: &Connection, effect_id: i64) -> Result<Option<Effect>, StoreError> {
    let raw = conn
        .query_row(
            &format!("SELECT {COLUMNS} FROM outbox WHERE effect_id = ?1"),
            [effect_id],
            RawEffect::from_row,
        )
        .optional()
        .map_err(sql("cannot read an outbox effect"))?;
    raw.map(RawEffect::into_effect).transpose()
}

/// The `pending` and `sent` rows due at `now_ms`, at most `limit`, by job and then
/// `effect_id`.
pub(super) fn due(conn: &Connection, now_ms: i64, limit: u32) -> Result<Vec<Effect>, StoreError> {
    let mut statement = conn
        .prepare(&format!(
            "SELECT {COLUMNS} FROM outbox \
             WHERE state IN ('pending', 'sent') AND (next_at IS NULL OR next_at <= ?1) \
             ORDER BY job_id, effect_id LIMIT ?2"
        ))
        .map_err(sql("cannot read the due outbox effects"))?;
    let rows = statement
        .query_map(params![now_ms, limit], RawEffect::from_row)
        .map_err(sql("cannot read the due outbox effects"))?;
    let mut effects = Vec::new();
    for raw in rows {
        let raw = raw.map_err(sql("cannot read the due outbox effects"))?;
        effects.push(raw.into_effect()?);
    }
    Ok(effects)
}

/// An outbox row as SQLite gives it, before its text columns are parsed.
struct RawEffect {
    /// `effect_id`.
    effect_id: i64,
    /// `job_id`.
    job_id: String,
    /// `kind`.
    kind: String,
    /// `effect_key`.
    effect_key: String,
    /// `payload_blob`.
    payload_blob: String,
    /// `state`.
    state: String,
    /// `attempts`.
    attempts: i64,
    /// `next_at`.
    next_at: Option<i64>,
    /// `result`.
    result: Option<String>,
}

impl RawEffect {
    /// Read the columns of [`COLUMNS`] from `row`.
    fn from_row(row: &Row<'_>) -> rusqlite::Result<RawEffect> {
        Ok(RawEffect {
            effect_id: row.get(0)?,
            job_id: row.get(1)?,
            kind: row.get(2)?,
            effect_key: row.get(3)?,
            payload_blob: row.get(4)?,
            state: row.get(5)?,
            attempts: row.get(6)?,
            next_at: row.get(7)?,
            result: row.get(8)?,
        })
    }

    /// Parse the text columns.
    fn into_effect(self) -> Result<Effect, StoreError> {
        Ok(Effect {
            effect_id: self.effect_id,
            job_id: JobId(self.job_id),
            kind: EffectKind::parse(&self.kind)?,
            effect_key: self.effect_key,
            payload_blob: self.payload_blob.parse()?,
            state: EffectState::parse(&self.state)?,
            attempts: self.attempts,
            next_at: self.next_at,
            result: self.result,
        })
    }
}
