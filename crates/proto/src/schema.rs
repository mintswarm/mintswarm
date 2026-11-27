//! JSON Schemas generated from the compiled descriptors (design §12.4,
//! "Generated, never maintained by hand"): today the stage result block's,
//! [`stage_result_schema`], committed at [`STAGE_RESULT_SCHEMA`].
//!
//! The walker reads every property and enum value from the descriptors, so
//! nothing here names a field. It writes the proto3 JSON mapping's output
//! shape and only a small keyword subset: `type`, `enum`, `properties`,
//! `additionalProperties` (always `false`) and `items`. Keys are the
//! lowerCamel JSON names, sorted, enums list their value names without the
//! zero value, a proto3 `optional` field is its plain type, and no message
//! has a `required` list. Field kinds the subset cannot say exactly, such as
//! maps, real oneofs, well-known types, integers of any width, floats and
//! bytes, enums with negative values, and recursive messages are refused with
//! an [`Error`], so a proto change that needs more keywords fails generation
//! instead of drifting.

use std::collections::BTreeMap;

use prost::Name as _;
use prost_reflect::{FieldDescriptor, Kind, MessageDescriptor};
use serde::ser::{Serialize, SerializeMap as _, SerializeSeq as _, Serializer};
use serde_json::{Map, Value, json};

use crate::runner::v1::StageResult;

/// Where the `StageResult` schema is committed, relative to the workspace root.
pub const STAGE_RESULT_SCHEMA: &str = "api/schemas/runner/v1/stage_result.schema.json";

/// The package of the well-known types, which the subset cannot describe.
const WELL_KNOWN_PACKAGE: &str = "google.protobuf";

/// Why a schema could not be generated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// The descriptor set does not decode, or lacks the message.
    DescriptorsMissing(String),
    /// A field has a kind the keyword subset does not map.
    UnsupportedKind {
        /// The field's full name.
        field: String,
        /// The kind, for example `int64` or `map`.
        kind: &'static str,
    },
    /// A message contains itself, so it cannot be inlined.
    Recursion {
        /// The full name of the message met again.
        message: String,
    },
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DescriptorsMissing(detail) => write!(f, "descriptors missing: {detail}"),
            Self::UnsupportedKind { field, kind } => {
                write!(
                    f,
                    "{field} has kind {kind}, which the schema subset does not map"
                )
            }
            Self::Recursion { message } => {
                write!(f, "{message} contains itself and cannot be inlined")
            }
        }
    }
}

impl std::error::Error for Error {}

/// The schema of `mintswarm.runner.v1.StageResult`, pretty-printed with sorted
/// keys and a trailing newline: the committed file's exact text.
pub fn stage_result_schema() -> Result<String, Error> {
    let pool = crate::json::pool().map_err(Error::DescriptorsMissing)?;
    let name = StageResult::full_name();
    let message = pool
        .get_message_by_name(&name)
        .ok_or_else(|| Error::DescriptorsMissing(format!("no descriptor for {name}")))?;
    let schema = message_schema(&message)?;
    let mut text = serde_json::to_string_pretty(&Sorted(&schema))
        .map_err(|error| Error::DescriptorsMissing(error.to_string()))?;
    text.push('\n');
    Ok(text)
}

/// Serialises a [`Value`] with every object's keys sorted, whether or not
/// serde_json's `preserve_order` feature is on somewhere in the build.
struct Sorted<'a>(&'a Value);

impl Serialize for Sorted<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self.0 {
            Value::Object(object) => {
                let sorted: BTreeMap<&String, &Value> = object.iter().collect();
                let mut map = serializer.serialize_map(Some(sorted.len()))?;
                for (key, value) in sorted {
                    map.serialize_entry(key, &Sorted(value))?;
                }
                map.end()
            }
            Value::Array(items) => {
                let mut seq = serializer.serialize_seq(Some(items.len()))?;
                for item in items {
                    seq.serialize_element(&Sorted(item))?;
                }
                seq.end()
            }
            other => other.serialize(serializer),
        }
    }
}

/// The schema of `message`'s proto3 JSON form, with nested messages inlined.
pub fn message_schema(message: &MessageDescriptor) -> Result<Value, Error> {
    walk_message(message, &mut Vec::new())
}

/// The schema of `message`; `stack` holds the messages being walked.
fn walk_message(message: &MessageDescriptor, stack: &mut Vec<String>) -> Result<Value, Error> {
    let name = message.full_name().to_owned();
    if stack.contains(&name) {
        return Err(Error::Recursion { message: name });
    }
    stack.push(name);
    let mut properties = Map::new();
    for field in message.fields() {
        properties.insert(field.json_name().to_owned(), field_schema(&field, stack)?);
    }
    stack.pop();
    Ok(json!({
        "type": "object",
        "additionalProperties": false,
        "properties": properties,
    }))
}

/// The schema of `field`'s JSON value.
fn field_schema(field: &FieldDescriptor, stack: &mut Vec<String>) -> Result<Value, Error> {
    let unsupported = |kind| Error::UnsupportedKind {
        field: field.full_name().to_owned(),
        kind,
    };
    if field.is_map() {
        return Err(unsupported("map"));
    }
    if field
        .containing_oneof()
        .is_some_and(|oneof| !oneof.is_synthetic())
    {
        return Err(unsupported("oneof"));
    }
    let single = match field.kind() {
        Kind::String => json!({ "type": "string" }),
        Kind::Bool => json!({ "type": "boolean" }),
        Kind::Enum(descriptor) => {
            if descriptor.values().any(|value| value.number() < 0) {
                return Err(unsupported("negative enum value"));
            }
            let names: Vec<String> = descriptor
                .values()
                .filter(|value| value.number() > 0)
                .map(|value| value.name().to_owned())
                .collect();
            json!({ "type": "string", "enum": names })
        }
        Kind::Message(descriptor) => {
            if descriptor.package_name() == WELL_KNOWN_PACKAGE {
                return Err(unsupported("well-known type"));
            }
            walk_message(&descriptor, stack)?
        }
        Kind::Int32 | Kind::Sint32 | Kind::Sfixed32 => return Err(unsupported("int32")),
        Kind::Uint32 | Kind::Fixed32 => return Err(unsupported("uint32")),
        Kind::Int64 | Kind::Sint64 | Kind::Sfixed64 => return Err(unsupported("int64")),
        Kind::Uint64 | Kind::Fixed64 => return Err(unsupported("uint64")),
        Kind::Float => return Err(unsupported("float")),
        Kind::Double => return Err(unsupported("double")),
        Kind::Bytes => return Err(unsupported("bytes")),
    };
    Ok(if field.is_list() {
        json!({ "type": "array", "items": single })
    } else {
        single
    })
}
