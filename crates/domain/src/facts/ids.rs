//! Item keys and the ids derived from them, design §4.1.
//!
//! `H(tag, parts…)` is SHA-256 over the tag and then each part, each followed by `\n`. Ids are
//! the first bytes of the digest; an id that comes out all zero has its last byte set to 1.

use std::borrow::Cow;
use std::fmt;
use std::str::FromStr;

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use sha2::{Digest, Sha256};

/// The tag of trace ids.
pub const TRACE_TAG: &str = "mintswarm.trace.v1";
/// The tag of root span ids.
pub const ROOT_TAG: &str = "mintswarm.root.v1";
/// The tag of span ids.
pub const SPAN_TAG: &str = "mintswarm.span.v1";
/// The tag of fact ids.
pub const FACT_TAG: &str = "mintswarm.fact.v1";

/// The pattern an item key's text matches; host, owner and repository are lowercased on reading.
pub const ITEM_KEY_PATTERN: &str = r"^[^/#\s]+/[^/#\s]+/[^/#\s]+#[1-9][0-9]*$";

/// An item's identity: `<tracker host>/<owner>/<repo>#<number>`, with host, owner and
/// repository lowercased, for example `github.com/mintswarm/mintswarm#412`.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ItemKey {
    host: String,
    owner: String,
    repository: String,
    number: u64,
}

/// Why a text is not an item key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ItemKeyError {
    /// The text does not have the shape `<host>/<owner>/<repo>#<number>`.
    Shape(String),
    /// A host, owner or repository segment is empty or holds `/`, `#` or white space.
    Segment(String),
    /// The number is not a positive decimal integer without leading zeros.
    Number(String),
}

impl fmt::Display for ItemKeyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ItemKeyError::Shape(text) => {
                write!(
                    f,
                    "`{text}` is not of the form <host>/<owner>/<repo>#<number>"
                )
            }
            ItemKeyError::Segment(text) => write!(f, "`{text}` is not a valid key segment"),
            ItemKeyError::Number(text) => write!(f, "`{text}` is not a valid item number"),
        }
    }
}

impl std::error::Error for ItemKeyError {}

fn segment(text: &str) -> Result<String, ItemKeyError> {
    if text.is_empty()
        || text
            .chars()
            .any(|c| c == '/' || c == '#' || c.is_whitespace())
    {
        return Err(ItemKeyError::Segment(text.into()));
    }
    Ok(text.to_lowercase())
}

impl ItemKey {
    /// The key of item `number` in `owner/repository` on `host`, lowercasing the text parts.
    pub fn new(
        host: &str,
        owner: &str,
        repository: &str,
        number: u64,
    ) -> Result<Self, ItemKeyError> {
        if number == 0 {
            return Err(ItemKeyError::Number(number.to_string()));
        }
        Ok(ItemKey {
            host: segment(host)?,
            owner: segment(owner)?,
            repository: segment(repository)?,
            number,
        })
    }

    /// The tracker host, for example `github.com`.
    pub fn host(&self) -> &str {
        &self.host
    }

    /// The repository's owner.
    pub fn owner(&self) -> &str {
        &self.owner
    }

    /// The repository's name, without its owner.
    pub fn repository(&self) -> &str {
        &self.repository
    }

    /// The item's number on the tracker.
    pub fn number(&self) -> u64 {
        self.number
    }
}

impl fmt::Display for ItemKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}/{}/{}#{}",
            self.host, self.owner, self.repository, self.number
        )
    }
}

impl FromStr for ItemKey {
    type Err = ItemKeyError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let shape = || ItemKeyError::Shape(text.into());
        let (path, number) = text.split_once('#').ok_or_else(shape)?;
        let mut segments = path.split('/');
        let (Some(host), Some(owner), Some(repository), None) = (
            segments.next(),
            segments.next(),
            segments.next(),
            segments.next(),
        ) else {
            return Err(shape());
        };
        let bad_number = || ItemKeyError::Number(number.into());
        if number.starts_with('0') || !number.bytes().all(|b| b.is_ascii_digit()) {
            return Err(bad_number());
        }
        let number = number.parse::<u64>().map_err(|_| bad_number())?;
        ItemKey::new(host, owner, repository, number)
    }
}

impl Serialize for ItemKey {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for ItemKey {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        text.parse().map_err(serde::de::Error::custom)
    }
}

impl JsonSchema for ItemKey {
    fn inline_schema() -> bool {
        true
    }

    fn schema_name() -> Cow<'static, str> {
        "ItemKey".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({ "type": "string", "pattern": ITEM_KEY_PATTERN })
    }
}

/// The kind of a span below an item's root.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SpanKind {
    StageRun,
    Turn,
    ToolCall,
    Phase,
    Wait,
}

impl SpanKind {
    /// Every span kind, in design §4.1's order.
    pub const ALL: [SpanKind; 5] = [
        SpanKind::StageRun,
        SpanKind::Turn,
        SpanKind::ToolCall,
        SpanKind::Phase,
        SpanKind::Wait,
    ];

    /// The kind as it is spelt in facts and in span id derivations.
    pub fn as_str(self) -> &'static str {
        match self {
            SpanKind::StageRun => "stage_run",
            SpanKind::Turn => "turn",
            SpanKind::ToolCall => "tool_call",
            SpanKind::Phase => "phase",
            SpanKind::Wait => "wait",
        }
    }
}

/// Bytes as lowercase hexadecimal.
pub fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(char::from(DIGITS[usize::from(byte >> 4)]));
        out.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    out
}

/// Why a text is not an id of the expected width.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IdError(pub String);

impl fmt::Display for IdError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "`{}` is not an id in lowercase hex of the expected width",
            self.0
        )
    }
}

impl std::error::Error for IdError {}

fn unhex<const N: usize>(text: &str) -> Result<[u8; N], IdError> {
    let error = || IdError(text.into());
    let digit = |b: u8| match b {
        b'0'..=b'9' => Ok(b - b'0'),
        b'a'..=b'f' => Ok(b - b'a' + 10),
        _ => Err(error()),
    };
    let bytes = text.as_bytes();
    if bytes.len() != N * 2 {
        return Err(error());
    }
    let mut out = [0_u8; N];
    for (byte, pair) in out.iter_mut().zip(bytes.chunks_exact(2)) {
        *byte = (digit(pair[0])? << 4) | digit(pair[1])?;
    }
    Ok(out)
}

/// `H(tag, parts…)`: SHA-256 over `tag` and then each part, each followed by `\n`.
pub fn h<P: AsRef<str>>(tag: &str, parts: &[P]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(tag.as_bytes());
    hasher.update(b"\n");
    for part in parts {
        hasher.update(part.as_ref().as_bytes());
        hasher.update(b"\n");
    }
    hasher.finalize().into()
}

/// The first `N` bytes of a digest, with the last byte set to 1 when they are all zero.
fn truncate<const N: usize>(digest: &[u8; 32]) -> [u8; N] {
    let mut out = [0_u8; N];
    out.copy_from_slice(&digest[..N]);
    if out.iter().all(|b| *b == 0) {
        out[N - 1] = 1;
    }
    out
}

macro_rules! hex_id {
    ($(#[$doc:meta])* $name:ident, $width:literal, $pattern:literal) => {
        $(#[$doc])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub struct $name(pub [u8; $width]);

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&hex(&self.0))
            }
        }

        impl FromStr for $name {
            type Err = IdError;

            fn from_str(text: &str) -> Result<Self, Self::Err> {
                unhex(text).map($name)
            }
        }

        impl Serialize for $name {
            fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.collect_str(self)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                let text = String::deserialize(deserializer)?;
                text.parse().map_err(serde::de::Error::custom)
            }
        }

        impl JsonSchema for $name {
            fn inline_schema() -> bool {
                true
            }

            fn schema_name() -> Cow<'static, str> {
                stringify!($name).into()
            }

            fn json_schema(_: &mut SchemaGenerator) -> Schema {
                json_schema!({ "type": "string", "pattern": $pattern })
            }
        }
    };
}

hex_id!(
    /// An OTLP trace id: one per item.
    TraceId,
    16,
    "^[0-9a-f]{32}$"
);
hex_id!(
    /// An OTLP span id.
    SpanId,
    8,
    "^[0-9a-f]{16}$"
);
hex_id!(
    /// A fact's id, carried in `mintswarm.fact.id`: the same fact written twice has the same id.
    FactId,
    16,
    "^[0-9a-f]{32}$"
);

/// The item's trace id: `H("mintswarm.trace.v1", key)`, first 16 bytes.
pub fn trace_id(key: &ItemKey) -> TraceId {
    TraceId(truncate(&h(TRACE_TAG, &[key.to_string()])))
}

/// The item's root span id: `H("mintswarm.root.v1", key)`, first 8 bytes.
pub fn root_span_id(key: &ItemKey) -> SpanId {
    SpanId(truncate(&h(ROOT_TAG, &[key.to_string()])))
}

/// A span's id: `H("mintswarm.span.v1", key, kind, local…)`, first 8 bytes.
pub fn span_id<P: AsRef<str>>(key: &ItemKey, kind: SpanKind, local: &[P]) -> SpanId {
    let mut parts = vec![key.to_string(), kind.as_str().to_owned()];
    parts.extend(local.iter().map(|p| p.as_ref().to_owned()));
    SpanId(truncate(&h(SPAN_TAG, &parts)))
}

/// A fact's id: `H("mintswarm.fact.v1", key, event name, natural key…)`, first 16 bytes.
pub fn fact_id<P: AsRef<str>>(key: &ItemKey, event: &str, natural_key: &[P]) -> FactId {
    let mut parts = vec![key.to_string(), event.to_owned()];
    parts.extend(natural_key.iter().map(|p| p.as_ref().to_owned()));
    FactId(truncate(&h(FACT_TAG, &parts)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_zero_digest_ends_in_one() {
        let zero = [0_u8; 32];
        assert_eq!(hex(&truncate::<8>(&zero)), "0000000000000001");
        assert_eq!(
            hex(&truncate::<16>(&zero)),
            "00000000000000000000000000000001"
        );
        let mut digest = [0_u8; 32];
        digest[3] = 7;
        assert_eq!(hex(&truncate::<8>(&digest)), "0000000700000000");
    }

    #[test]
    fn hex_round_trips() {
        assert_eq!(hex(&[0x00, 0x0f, 0xa5, 0xff]), "000fa5ff");
        let id: SpanId = "00ff10a5c3e7b2d4".parse().unwrap();
        assert_eq!(id.0, [0x00, 0xff, 0x10, 0xa5, 0xc3, 0xe7, 0xb2, 0xd4]);
        assert_eq!(id.to_string(), "00ff10a5c3e7b2d4");
        assert!("00FF10A5C3E7B2D4".parse::<SpanId>().is_err());
        assert!("00ff10a5c3e7b2".parse::<SpanId>().is_err());
        assert!("00ff10a5c3e7b2dz".parse::<SpanId>().is_err());
    }

    #[test]
    fn h_hashes_tag_and_parts_with_newlines() {
        let expected: [u8; 32] = Sha256::digest(b"t\na\nb\n").into();
        assert_eq!(h("t", &["a", "b"]), expected);
    }

    #[test]
    fn item_key_lowercases_and_renders() {
        let key: ItemKey = "GitHub.com/MintSwarm/MintSwarm#412".parse().unwrap();
        assert_eq!(key.to_string(), "github.com/mintswarm/mintswarm#412");
        assert_eq!(key.host(), "github.com");
        assert_eq!(key.owner(), "mintswarm");
        assert_eq!(key.repository(), "mintswarm");
        assert_eq!(key.number(), 412);
        let built = ItemKey::new("GITHUB.COM", "Owner", "Repo", 7).unwrap();
        assert_eq!(built.to_string(), "github.com/owner/repo#7");
    }

    #[test]
    fn item_key_rejects_malformed_text() {
        for text in [
            "",
            "github.com/owner/repo",
            "github.com/owner#1",
            "github.com/owner/repo/extra#1",
            "github.com//repo#1",
            "github.com/own er/repo#1",
            "github.com/owner/repo#",
            "github.com/owner/repo#0",
            "github.com/owner/repo#012",
            "github.com/owner/repo#-1",
            "github.com/owner/repo#1#2",
            "github.com/owner/repo#99999999999999999999999",
        ] {
            assert!(text.parse::<ItemKey>().is_err(), "{text}");
        }
        assert!(ItemKey::new("github.com", "owner", "repo", 0).is_err());
    }

    #[test]
    fn item_key_serialises_as_text() {
        let key: ItemKey = serde_json::from_str("\"GitHub.com/a/B#3\"").unwrap();
        assert_eq!(serde_json::to_string(&key).unwrap(), "\"github.com/a/b#3\"");
        assert!(serde_json::from_str::<ItemKey>("\"a/b#3\"").is_err());
    }
}
