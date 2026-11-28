//! OTLP for facts: the one Rust code that builds or reads OTLP records, and the reference for
//! the journal line format.
//!
//! A [`Fact`] encodes to one export request, an [`Export`], with one resource, one scope and
//! one record. [`encode_line`] writes it as a journal line and [`decode_line`] reads one back
//! to the same [`Fact`], so the plane and the report tool need no second decoder. A runner's
//! `Facts` event carries the same messages.
//!
//! # The request
//!
//! - **Resource:** `service.name`, `service.version`, `service.instance.id` and `host.name`,
//!   in that order, from the caller's [`Origin`].
//! - **Scope:** name [`SCOPE_NAME`](mintswarm_domain::facts::SCOPE_NAME), version
//!   [`CATALOGUE_VERSION`](mintswarm_domain::facts::CATALOGUE_VERSION) as a decimal string,
//!   and [`SCHEMA_URL`](mintswarm_domain::facts::SCHEMA_URL) as the `schemaUrl` of the
//!   `ScopeSpans` or `ScopeLogs`.
//! - **Record attributes,** in this order:
//!   1. the five `mintswarm.item.*` keys;
//!   2. the [envelope keys](mintswarm_domain::facts::envelope) that are present;
//!   3. on a log record only, `mintswarm.fact.id`, the record's fact id in hex; spans do not
//!      carry it;
//!   4. the kind's attributes in the catalogue's field order.
//!
//!   A string becomes a `stringValue`, an integer an `intValue`, a double a `doubleValue`, a
//!   boolean a `boolValue` and an array an `arrayValue`. An integer attribute above `i64::MAX`
//!   is refused by [`encode_line`].
//! - **A log record** has `timeUnixNano`, `eventName` (the kind's event name), `traceId` and
//!   the `spanId` of the span it concerns, or of the item's root.
//! - **A span** has `traceId`, `spanId`, `parentSpanId` (the parent span, or the item's root;
//!   absent on the root itself), `name` (the kind's template with `{key}` filled from the
//!   attribute), `kind` `SPAN_KIND_INTERNAL`, its start and end times, its links and its
//!   status. A link carries the target item's trace id and the span id of the target's span,
//!   or of its root; its attributes are in field order, with the `mintswarm.link.to.*` keys in
//!   the place of `target`.
//! - **Status:** OK for a stage run whose `mintswarm.outcome.status` is `complete` or
//!   `needs_input`, ERROR for `failed`. An UNSET status, for `unknown` and for every kind
//!   without an outcome, is omitted; `"status":{}` is refused on reading.
//!
//! # The line
//!
//! - OTLP/JSON: lowerCamelCase keys, enums as integers, 64-bit integers as strings, fields
//!   with their default value omitted, and `traceId`, `spanId` and `parentSpanId` in lowercase
//!   hex instead of base64, at span, link and log-record level.
//! - Compact, with object keys sorted, and non-ASCII text written as raw UTF-8. Arrays keep
//!   their order.
//! - [`encode_line`] returns the line without a trailing `\n`; the journal adds it.
//!
//! Decoding refuses a request with other than exactly one resource, scope and record, a scope
//! name or version other than the catalogue's, and any record that is not exactly the
//! encoding of the fact it decodes to, its ids and fact id included.

use std::fmt;

use mintswarm_domain::facts::Fact;

use crate::opentelemetry::proto::collector::logs::v1::ExportLogsServiceRequest;
use crate::opentelemetry::proto::collector::trace::v1::ExportTraceServiceRequest;

mod fact;
pub mod json;

/// One OTLP export request: spans or log records.
#[derive(Clone, Debug, PartialEq)]
pub enum Export {
    Traces(ExportTraceServiceRequest),
    Logs(ExportLogsServiceRequest),
}

/// Who wrote a record: the resource attributes of its request.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Origin {
    /// `service.name`: the writer, for example `mintswarm-runner`.
    pub service_name: String,
    /// `service.version`.
    pub service_version: String,
    /// `service.instance.id`: a UUID per process.
    pub instance_id: String,
    /// `host.name`.
    pub host_name: String,
}

/// Why a fact or a line was refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    /// The text is not the OTLP/JSON form of an export request.
    Json(String),
    /// The request does not have the shape of a fact: one resource, one scope of the catalogue
    /// and one record, without an empty status.
    Shape(String),
    /// An attribute value has no OTLP encoding, or an OTLP value no attribute decoding.
    Value(String),
    /// The record does not decode to a fact.
    Fact(String),
    /// An id or the fact id differs from the one the decoded fact derives.
    Ids(String),
    /// The record differs from the encoding of the fact it decodes to.
    Differs,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Json(detail) => write!(f, "not an OTLP/JSON export request: {detail}"),
            Error::Shape(detail) => write!(f, "not the request of one fact: {detail}"),
            Error::Value(detail) => write!(f, "an attribute value does not encode: {detail}"),
            Error::Fact(detail) => write!(f, "the record is not a fact: {detail}"),
            Error::Ids(detail) => write!(f, "an id differs from the fact's: {detail}"),
            Error::Differs => f.write_str("the record is not the encoding of its fact"),
        }
    }
}

impl std::error::Error for Error {}

/// The export request of `fact`, written by `origin`.
pub fn encode(fact: &Fact, origin: &Origin) -> Result<Export, Error> {
    fact::encode(fact, origin)
}

/// The fact `export` is the encoding of; the resource is not read.
pub fn decode(export: &Export) -> Result<Fact, Error> {
    fact::decode(export)
}

/// The journal line of `fact`, written by `origin`, without a trailing `\n`.
pub fn encode_line(fact: &Fact, origin: &Origin) -> Result<String, Error> {
    Ok(json::to_json(&encode(fact, origin)?))
}

/// The fact a journal line holds.
pub fn decode_line(line: &str) -> Result<Fact, Error> {
    decode(&json::from_json(line)?)
}
