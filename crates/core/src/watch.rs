//! The rules the watch log's stores and the watch service share (design 6.2, 5.6, 5.7, B20):
//! which events a [`Filter`] matches, how ids and [`Cursor`]s are made, parsed and compared,
//! and the [`Retention`] bounds. Everything works on the generated `mintswarm.watch.v1` types.
//!
//! [`normalise`] turns a verified GitHub webhook delivery into a `github.webhook` event, and
//! [`Senders`] classes its sender (design 5.3, 6.3).
//!
//! An id is a UUIDv7 that increases in commit order: a store calls [`next_id`] while it holds
//! its append lock. Byte order is id order, both in Rust (`Uuid`'s `Ord`) and in Postgres (a
//! `uuid` column compares bytewise), so stores keep ids as native `uuid`, never as text. A
//! cursor is `<store_epoch>.<id>`, both UUIDs.
//!
//! A store implements [`WatchLog`]; [`MemoryWatchLog`] is the in-memory one, and
//! `mintswarm_core::testing::watch_log` holds the cases every store passes. A store resolves a
//! read's cursor against its epoch and `pruned_through`, the id of the latest pruned event: a
//! cursor of another epoch reads as the store changed, one strictly below `pruned_through` as
//! expired, and any other resumes after its id. A prune removes an id-ordered prefix, so
//! `pruned_through` is an exact boundary. The raw payload is kept apart from the event, read
//! back only on request and pruned with it.

use std::fmt;
use std::time::Duration;

use mintswarm_proto::prost_types;
use mintswarm_proto::watch::v1;
use uuid::{Uuid, Variant};

mod filter;
mod github;
mod memory;
mod store;

pub use filter::Filter;
pub use github::{BODY_LIMIT, Normalised, Senders, WEBHOOK_KIND, normalise, untrusted};
pub use memory::MemoryWatchLog;
pub use store::{AppendOutcome, Gap, Page, Pruned, WatchLog};

/// A watch request or value that breaks the shared rules.
#[derive(Debug, thiserror::Error)]
pub enum WatchError {
    /// A malformed filter.
    #[error("invalid filter: {0}")]
    Filter(String),
    /// A cursor that does not parse, or holds a nil epoch or an id that is not a UUIDv7.
    #[error("invalid cursor: {0}")]
    Cursor(String),
    /// A zero or unrepresentable retention bound.
    #[error("invalid retention: {0}")]
    Retention(String),
    /// No id can follow the latest one.
    #[error("cannot make an id: {0}")]
    Id(String),
    /// A webhook delivery with an empty delivery id or event name, or a body that is not a JSON
    /// object.
    #[error("invalid webhook delivery: {0}")]
    Payload(String),
}

impl From<WatchError> for tonic::Status {
    fn from(error: WatchError) -> Self {
        match error {
            WatchError::Filter(_)
            | WatchError::Cursor(_)
            | WatchError::Retention(_)
            | WatchError::Payload(_) => tonic::Status::invalid_argument(error.to_string()),
            WatchError::Id(_) => tonic::Status::internal(error.to_string()),
        }
    }
}

/// Whether `id` is a UUIDv7 with the RFC 4122 variant.
fn is_v7(id: &Uuid) -> bool {
    id.get_version_num() == 7 && id.get_variant() == Variant::RFC4122
}

/// A position in the log, `<store_epoch>.<id>`: a non-nil epoch and a UUIDv7 id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Cursor {
    epoch: Uuid,
    id: Uuid,
}

impl Cursor {
    /// The cursor of `id` in the store `epoch`.
    pub fn new(epoch: Uuid, id: Uuid) -> Result<Self, WatchError> {
        if epoch.is_nil() {
            return Err(WatchError::Cursor("the store epoch is nil".to_owned()));
        }
        if !is_v7(&id) {
            return Err(WatchError::Cursor(format!("the id {id} is not a UUIDv7")));
        }
        Ok(Self { epoch, id })
    }

    /// Reads `<epoch>.<id>`, each part a hyphenated UUID in either case.
    pub fn parse(text: &str) -> Result<Self, WatchError> {
        let mut parts = text.split('.');
        let (Some(epoch), Some(id), None) = (parts.next(), parts.next(), parts.next()) else {
            return Err(WatchError::Cursor(
                "a cursor is two UUIDs joined by one `.`".to_owned(),
            ));
        };
        Self::new(parse_hyphenated(epoch)?, parse_hyphenated(id)?)
    }

    /// The store epoch.
    pub fn epoch(&self) -> Uuid {
        self.epoch
    }

    /// The event's id.
    pub fn id(&self) -> Uuid {
        self.id
    }
}

/// Parses one cursor part, accepting only the 36-character hyphenated form.
fn parse_hyphenated(part: &str) -> Result<Uuid, WatchError> {
    let bytes = part.as_bytes();
    let hyphens = [8, 13, 18, 23].iter().all(|&i| bytes.get(i) == Some(&b'-'));
    if bytes.len() != 36 || !hyphens {
        return Err(WatchError::Cursor(format!(
            "`{part}` is not a hyphenated UUID"
        )));
    }
    Uuid::try_parse(part).map_err(|_| WatchError::Cursor(format!("`{part}` is not a UUID")))
}

impl fmt::Display for Cursor {
    /// Writes `<epoch>.<id>`, lowercase and hyphenated.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}", self.epoch, self.id)
    }
}

/// The 74 bits of a UUIDv7 below its timestamp that are not version or variant: `rand_a`
/// (12 bits, above the variant) and `rand_b` (62 bits).
const RAND_A: u128 = 0xfff << 64;
const RAND_B: u128 = (1 << 62) - 1;

/// The id to append after `latest`: a fresh UUIDv7, or the value just after `latest` when the
/// fresh one does not sort after it (the same millisecond, or the clock stepped back).
///
/// Fails when `latest` is not a UUIDv7, or when no id follows it.
pub fn next_id(latest: Option<Uuid>) -> Result<Uuid, WatchError> {
    let fresh = Uuid::now_v7();
    let Some(latest) = latest else {
        return Ok(fresh);
    };
    if !is_v7(&latest) {
        return Err(WatchError::Id(format!(
            "the latest id {latest} is not a UUIDv7"
        )));
    }
    if fresh > latest {
        return Ok(fresh);
    }
    // The timestamp, rand_a and rand_b as one 122-bit counter, then one more.
    let value = latest.as_u128();
    let counter = ((value >> 80) << 74) | (((value & RAND_A) >> 64) << 62) | (value & RAND_B);
    let next = counter + 1;
    if next >> 122 != 0 {
        return Err(WatchError::Id(format!("no id follows {latest}")));
    }
    let version_and_variant = value & !((0xffff_ffff_ffff << 80) | RAND_A | RAND_B);
    let id = ((next >> 74) << 80) | (((next >> 62) & 0xfff) << 64) | (next & RAND_B);
    Ok(Uuid::from_u128(id | version_and_variant))
}

/// The retention bounds: how old and how many events a store keeps. Both are positive; there
/// is no unlimited form.
#[derive(Debug, Clone, PartialEq)]
pub struct Retention(v1::Retention);

impl Retention {
    /// Bounds of `max_age` and `max_events`; fails for a zero bound, or an age a
    /// `google.protobuf.Duration` cannot hold.
    pub fn new(max_age: Duration, max_events: u64) -> Result<Self, WatchError> {
        if max_age.is_zero() {
            return Err(WatchError::Retention("the age bound is zero".to_owned()));
        }
        if max_events == 0 {
            return Err(WatchError::Retention("the event bound is zero".to_owned()));
        }
        let age = prost_types::Duration::try_from(max_age)
            .map_err(|_| WatchError::Retention(format!("the age bound {max_age:?} is too long")))?;
        Ok(Self(v1::Retention {
            max_age: Some(age),
            max_events,
            oldest_cursor: String::new(),
        }))
    }

    /// The oldest age an event is kept to.
    pub fn max_age(&self) -> Duration {
        self.0
            .max_age
            .and_then(|age| Duration::try_from(age).ok())
            .unwrap_or_default()
    }

    /// The most events kept.
    pub fn max_events(&self) -> u64 {
        self.0.max_events
    }

    /// The bounds as `GetServerInfoResponse` sends them, with the oldest stored event's cursor.
    pub fn to_proto(&self, oldest_cursor: String) -> v1::Retention {
        v1::Retention {
            oldest_cursor,
            ..self.0.clone()
        }
    }
}
