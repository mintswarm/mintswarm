//! The OTLP/JSON form of an [`Export`]: `prost-reflect`'s proto3 JSON mapping with integer
//! enums, skipped defaults and stringified 64-bit integers, ids rewritten between base64 and
//! hex, and an emitter that sorts object keys.

use prost::{Message, Name};
use prost_reflect::{DeserializeOptions, DynamicMessage, MessageDescriptor, SerializeOptions};
use serde_json::{Map, Value};

use super::{Error, Export};
use crate::opentelemetry::proto::collector::logs::v1::ExportLogsServiceRequest;
use crate::opentelemetry::proto::collector::trace::v1::ExportTraceServiceRequest;

/// The top-level key of a traces request.
const RESOURCE_SPANS: &str = "resourceSpans";
/// The top-level key of a logs request.
const RESOURCE_LOGS: &str = "resourceLogs";

/// The descriptor of the generated message `M`.
fn descriptor<M: Name>() -> Result<MessageDescriptor, Error> {
    let pool = crate::json::pool().map_err(Error::Json)?;
    let name = M::full_name();
    pool.get_message_by_name(&name)
        .ok_or_else(|| Error::Json(format!("no descriptor for {name}")))
}

/// `message` as a JSON tree, ids still in base64.
fn tree<M: Message + Name>(message: &M) -> Result<Value, Error> {
    let desc = descriptor::<M>()?;
    let dynamic = DynamicMessage::decode(desc, message.encode_to_vec().as_slice())
        .map_err(|e| Error::Json(e.to_string()))?;
    let options = SerializeOptions::new()
        .use_enum_numbers(true)
        .skip_default_fields(true)
        .stringify_64_bit_integers(true);
    dynamic
        .serialize_with_options(serde_json::value::Serializer, &options)
        .map_err(|e| Error::Json(e.to_string()))
}

/// The OTLP/JSON line of `export`, without a trailing `\n`.
///
/// # Panics
///
/// When the compiled descriptor set does not hold the export requests, which only a broken
/// build can cause.
pub fn to_json(export: &Export) -> String {
    let tree = match export {
        Export::Traces(request) => tree(request),
        Export::Logs(request) => tree(request),
    };
    let mut tree = tree.expect("the descriptor set holds the OTLP export requests");
    // Only bytes fields hold base64, and every one of them is an id the walk rewrites.
    rewrite_ids(&mut tree, &hex_of_base64).expect("prost-reflect writes ids as base64");
    let mut out = String::new();
    emit(&tree, &mut out);
    out
}

/// Reads an export request from its OTLP/JSON line.
pub fn from_json(text: &str) -> Result<Export, Error> {
    let mut tree: Value = serde_json::from_str(text).map_err(|e| Error::Json(e.to_string()))?;
    let key = match tree.as_object() {
        Some(object) if object.len() == 1 => object.keys().next().cloned().unwrap_or_default(),
        _ => {
            return Err(Error::Json(format!(
                "an object with exactly one of `{RESOURCE_SPANS}` and `{RESOURCE_LOGS}`"
            )));
        }
    };
    rewrite_ids(&mut tree, &base64_of_hex)?;
    let options = DeserializeOptions::new().deny_unknown_fields(true);
    let read = |desc: MessageDescriptor| {
        DynamicMessage::deserialize_with_options(desc, tree.clone(), &options)
            .map_err(|e| Error::Json(e.to_string()))
    };
    match key.as_str() {
        RESOURCE_SPANS => read(descriptor::<ExportTraceServiceRequest>()?)?
            .transcode_to()
            .map(Export::Traces)
            .map_err(|e| Error::Json(e.to_string())),
        RESOURCE_LOGS => read(descriptor::<ExportLogsServiceRequest>()?)?
            .transcode_to()
            .map(Export::Logs)
            .map_err(|e| Error::Json(e.to_string())),
        other => Err(Error::Json(format!("unknown top-level key `{other}`"))),
    }
}

/// The objects of the array at `object[key]`, if any.
fn objects<'a>(object: &'a mut Map<String, Value>, key: &str) -> Vec<&'a mut Map<String, Value>> {
    match object.get_mut(key) {
        Some(Value::Array(items)) => items.iter_mut().filter_map(Value::as_object_mut).collect(),
        _ => Vec::new(),
    }
}

/// Rewrites, with `convert`, the `traceId`, `spanId` and `parentSpanId` of every span, link
/// and log record in `tree`; `convert` gets the value and its width in bytes.
fn rewrite_ids(
    tree: &mut Value,
    convert: &dyn Fn(&str, usize) -> Result<String, Error>,
) -> Result<(), Error> {
    let Some(root) = tree.as_object_mut() else {
        return Ok(());
    };
    let ids = |record: &mut Map<String, Value>| -> Result<(), Error> {
        for (key, width) in [("traceId", 16), ("spanId", 8), ("parentSpanId", 8)] {
            if let Some(value) = record.get_mut(key) {
                let Value::String(text) = value else {
                    return Err(Error::Json(format!("`{key}` is not a string")));
                };
                *text = convert(text, width)
                    .map_err(|_| Error::Json(format!("`{key}` is not an id of {width} bytes")))?;
            }
        }
        Ok(())
    };
    for resource in objects(root, "resourceSpans") {
        for scope in objects(resource, "scopeSpans") {
            for span in objects(scope, "spans") {
                ids(span)?;
                for link in objects(span, "links") {
                    ids(link)?;
                }
            }
        }
    }
    for resource in objects(root, "resourceLogs") {
        for scope in objects(resource, "scopeLogs") {
            for record in objects(scope, "logRecords") {
                ids(record)?;
            }
        }
    }
    Ok(())
}

/// Writes `value` as compact JSON with object keys sorted.
fn emit(value: &Value, out: &mut String) {
    match value {
        Value::Object(object) => {
            let mut entries: Vec<(&String, &Value)> = object.iter().collect();
            entries.sort_by(|a, b| a.0.cmp(b.0));
            out.push('{');
            for (i, (key, value)) in entries.into_iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&Value::String(key.clone()).to_string());
                out.push(':');
                emit(value, out);
            }
            out.push('}');
        }
        Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                emit(item, out);
            }
            out.push(']');
        }
        scalar => out.push_str(&scalar.to_string()),
    }
}

const BASE64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Standard padded base64 of `bytes`.
fn base64(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for (i, shift) in [18, 12, 6, 0].into_iter().enumerate() {
            if i <= chunk.len() {
                out.push(char::from(BASE64[((n >> shift) & 0x3f) as usize]));
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// The bytes of standard base64 text, padded or not.
fn unbase64(text: &str) -> Option<Vec<u8>> {
    let digits = text.trim_end_matches('=').as_bytes();
    let mut bits = 0_u32;
    let mut count = 0_u32;
    let mut out = Vec::with_capacity(digits.len() * 3 / 4);
    for digit in digits {
        let value = BASE64.iter().position(|b| b == digit)?;
        bits = (bits << 6) | u32::try_from(value).ok()?;
        count += 6;
        if count >= 8 {
            count -= 8;
            out.push(u8::try_from((bits >> count) & 0xff).ok()?);
        }
    }
    Some(out)
}

/// Lowercase hex of the base64 id `text`; the width is checked on reading only.
fn hex_of_base64(text: &str, _width: usize) -> Result<String, Error> {
    unbase64(text)
        .map(|bytes| mintswarm_domain::facts::ids::hex(&bytes))
        .ok_or_else(|| Error::Json(format!("`{text}` is not base64")))
}

/// Base64 of the lowercase hex id `text` of `width` bytes.
fn base64_of_hex(text: &str, width: usize) -> Result<String, Error> {
    let error = || Error::Json(format!("`{text}` is not a lowercase hex id"));
    if text.len() != width * 2 {
        return Err(error());
    }
    let digit = |b: u8| match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        _ => None,
    };
    let bytes: Option<Vec<u8>> = text
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| Some((digit(pair[0])? << 4) | digit(pair[1])?))
        .collect();
    Ok(base64(&bytes.ok_or_else(error)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_round_trips() {
        for bytes in [
            vec![],
            vec![0],
            vec![0xff, 0x10],
            vec![1, 2, 3],
            (0..16).collect::<Vec<u8>>(),
            (0..8).collect::<Vec<u8>>(),
        ] {
            assert_eq!(unbase64(&base64(&bytes)), Some(bytes.clone()));
        }
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(unbase64("Zm8"), Some(b"fo".to_vec()));
        assert_eq!(unbase64("Z!=="), None);
    }

    #[test]
    fn hex_ids_convert_both_ways() {
        let hex = "00ff10a5c3e7b2d4";
        let b64 = base64_of_hex(hex, 8).unwrap();
        assert_eq!(hex_of_base64(&b64, 8).unwrap(), hex);
        assert!(base64_of_hex("00FF10A5C3E7B2D4", 8).is_err());
        assert!(base64_of_hex(hex, 16).is_err());
    }

    #[test]
    fn emitter_sorts_keys_and_keeps_arrays() {
        let value: Value = serde_json::from_str(r#"{"b":[3,1],"a":{"d":"é","c":1.5}}"#).unwrap();
        let mut out = String::new();
        emit(&value, &mut out);
        assert_eq!(out, r#"{"a":{"c":1.5,"d":"é"},"b":[3,1]}"#);
    }
}
