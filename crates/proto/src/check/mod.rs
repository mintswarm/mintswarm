//! The receive checks: one checked edge for every `runner/v1` message that a
//! runner or the plane receives, and for the stage result block (design D15).
//!
//! Proto3 admits states the protocol rules out: an absent message, an
//! `*_UNSPECIFIED` enum, an empty id. [`Receive::receive`] turns a received
//! message into the same message, accepted, or a typed error that names the
//! message, the field and the rule. A received message is checked once, at the
//! edge, before anything reads it.
//!
//! The wire checks stop at the first violation, since a failed check only
//! closes the stream (design §5.3 rule 6). The [`StageResult`] check returns
//! every violation at once, so that a stage can be told every fix in one reply.
//!
//! The checks are stage-independent: the artefact name, the outputs path and a
//! failed outcome's reason are checked by the rules that know the stage.
//!
//! [`StageResult`]: crate::runner::v1::StageResult

use std::fmt;

use prost::Name;

use crate::common::v1::Secret;
use crate::runner::v1::{AttemptOutcome, AttemptOutcomeKind};

mod stage;
mod wire;

pub use stage::QuestionViolation;

/// A message that is checked when it is received.
pub trait Receive: Sized {
    /// What a refused message gives.
    type Error;

    /// Returns the message unchanged when it keeps every rule of its type, and
    /// the violation otherwise.
    fn receive(self) -> Result<Self, Self::Error>;
}

/// A received message that breaks a rule.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReceiveError {
    /// The full proto name of the received top-level message, such as
    /// `mintswarm.runner.v1.ConnectRequest`.
    pub message: String,
    /// The dotted path of the field inside it, such as
    /// `body.hello.capabilities[0].result`; a `oneof` is named by its field
    /// and then its case.
    pub field: String,
    /// The rule the field breaks.
    pub rule: Rule,
}

impl fmt::Display for ReceiveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}: {}", self.message, self.field, self.rule)
    }
}

impl std::error::Error for ReceiveError {}

/// A rule of the receive checks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Rule {
    /// A required field is absent or empty: a `None` message, an absent
    /// `oneof` case, or an empty required string, bytes or list.
    Missing,
    /// A `Connect` envelope has no `body` case this build knows. Prost decodes
    /// a case added by a newer peer and an absent body alike, so callers
    /// ignore and count such an envelope (design §5.3 rule 5) rather than
    /// close the stream.
    NoBody,
    /// A required enum is `*_UNSPECIFIED`.
    Unspecified,
    /// A required enum holds a value this build does not know.
    UnknownValue {
        /// The value as received.
        raw: i32,
    },
    /// A required id is empty.
    EmptyId,
    /// A sha256 is not exactly 64 lowercase hex characters.
    MalformedSha256,
    /// A required non-zero number, such as `seq` or `lease_epoch`, is zero.
    Zero,
    /// A runner event sets some of its fence fields (`job_id`, `attempt_id`,
    /// `lease_epoch`) but not all of them.
    PartialFence,
    /// A question of a stage result, or the result's status, breaks the stage
    /// contract.
    Question(QuestionViolation),
}

impl fmt::Display for Rule {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Rule::Missing => f.write_str("is missing"),
            Rule::NoBody => f.write_str("has no body case this build knows"),
            Rule::Unspecified => f.write_str("is unspecified"),
            Rule::UnknownValue { raw } => write!(f, "has the unknown value {raw}"),
            Rule::EmptyId => f.write_str("is an empty id"),
            Rule::MalformedSha256 => f.write_str("is not a sha256 of 64 lowercase hex characters"),
            Rule::Zero => f.write_str("is zero"),
            Rule::PartialFence => f.write_str("is empty while other fence fields are set"),
            Rule::Question(violation) => violation.fmt(f),
        }
    }
}

impl AttemptOutcome {
    /// The outcome's kind, with a value unknown to this build read as
    /// `Failed` (design §10); [`kind`](Self::kind) keeps the raw value.
    ///
    /// Read the kind through this helper: prost's generated `kind()` reads an
    /// unknown value as `Unspecified`, which loses the failed reading. A
    /// received outcome is never `Unspecified`, since its check refuses `0`.
    pub fn effective_kind(&self) -> AttemptOutcomeKind {
        AttemptOutcomeKind::try_from(self.kind).unwrap_or(AttemptOutcomeKind::Failed)
    }
}

/// Where a check is: the top-level message's full name and the path of the
/// field being checked.
#[derive(Clone, Debug)]
struct At<'a> {
    message: &'a str,
    path: String,
}

impl<'a> At<'a> {
    /// The top of the message named `message`.
    fn root(message: &'a str) -> Self {
        At {
            message,
            path: String::new(),
        }
    }

    /// The path of `name` under this one; `name` may be empty for this path.
    fn join(&self, name: &str) -> String {
        match (self.path.is_empty(), name.is_empty()) {
            (true, _) => name.to_owned(),
            (false, true) => self.path.clone(),
            (false, false) => format!("{}.{name}", self.path),
        }
    }

    /// The field `name` under this one.
    fn child(&self, name: &str) -> At<'a> {
        At {
            message: self.message,
            path: self.join(name),
        }
    }

    /// Entry `index` of the repeated field `name` under this one.
    fn item(&self, name: &str, index: usize) -> At<'a> {
        At {
            message: self.message,
            path: format!("{}[{index}]", self.join(name)),
        }
    }

    /// Entry `key` of the map field `name` under this one.
    fn key(&self, name: &str, key: &str) -> At<'a> {
        At {
            message: self.message,
            path: format!("{}[{key:?}]", self.join(name)),
        }
    }

    /// The error for `name` under this path breaking `rule`.
    fn fail(&self, name: &str, rule: Rule) -> ReceiveError {
        ReceiveError {
            message: self.message.to_owned(),
            field: self.join(name),
            rule,
        }
    }

    /// `EmptyId` when `value` is empty.
    fn id(&self, name: &str, value: &str) -> Result<(), ReceiveError> {
        if value.is_empty() {
            return Err(self.fail(name, Rule::EmptyId));
        }
        Ok(())
    }

    /// `Missing` when the required string `value` is empty.
    fn text(&self, name: &str, value: &str) -> Result<(), ReceiveError> {
        if value.is_empty() {
            return Err(self.fail(name, Rule::Missing));
        }
        Ok(())
    }

    /// `Missing` when the required list or bytes `value` is empty.
    fn non_empty<T>(&self, name: &str, value: &[T]) -> Result<(), ReceiveError> {
        if value.is_empty() {
            return Err(self.fail(name, Rule::Missing));
        }
        Ok(())
    }

    /// `Zero` when the required number `value` is zero.
    fn non_zero(&self, name: &str, value: u64) -> Result<(), ReceiveError> {
        if value == 0 {
            return Err(self.fail(name, Rule::Zero));
        }
        Ok(())
    }

    /// `MalformedSha256` unless `value` is exactly 64 lowercase hex characters.
    fn sha256(&self, name: &str, value: &str) -> Result<(), ReceiveError> {
        if !is_sha256(value) {
            return Err(self.fail(name, Rule::MalformedSha256));
        }
        Ok(())
    }

    /// Each entry of `values` is a well-formed sha256.
    fn sha256_each(&self, name: &str, values: &[String]) -> Result<(), ReceiveError> {
        for (index, value) in values.iter().enumerate() {
            self.item(name, index).sha256("", value)?;
        }
        Ok(())
    }

    /// The message in `value`, or `Missing` when it is absent.
    fn present<'v, T>(&self, name: &str, value: &'v Option<T>) -> Result<&'v T, ReceiveError> {
        value.as_ref().ok_or_else(|| self.fail(name, Rule::Missing))
    }

    /// The required enum `raw` as `E`: `Unspecified` for `0` and
    /// `UnknownValue` for a value this build does not know.
    fn required<E: TryFrom<i32>>(&self, name: &str, raw: i32) -> Result<E, ReceiveError> {
        enum_value(raw).map_err(|rule| self.fail(name, rule))
    }

    /// A `Secret` that is present and holds a value.
    fn secret(&self, name: &str, value: &Option<Secret>) -> Result<(), ReceiveError> {
        let secret = self.present(name, value)?;
        self.child(name).text("value", &secret.value)
    }
}

/// The required enum `raw` as `E`, or the rule it breaks.
fn enum_value<E: TryFrom<i32>>(raw: i32) -> Result<E, Rule> {
    if raw == 0 {
        return Err(Rule::Unspecified);
    }
    E::try_from(raw).map_err(|_| Rule::UnknownValue { raw })
}

/// Whether `value` is exactly 64 lowercase hex characters.
fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// The full proto name of `M`.
fn full_name<M: Name>() -> String {
    M::full_name()
}
