//! YAML to JSON, resolved as yq v4.45.1 resolves it.
//!
//! yq reads YAML with yaml.v3, which tags each plain scalar by its form, and then marshals each
//! tagged scalar to JSON by its own rules. Both steps are reproduced here, so a document yq
//! converts gives the same JSON value. Constructs whose yq result is surprising (merge keys,
//! complex keys, custom tags, several documents) are rejected instead, as are documents with
//! duplicate keys, which yq emits twice.

use std::collections::HashMap;

use serde_json::{Map, Number, Value};
use yaml_rust2::parser::{Event, MarkedEventReceiver, Parser, Tag};
use yaml_rust2::scanner::{Marker, TScalarStyle};

use crate::{Error, Path};

/// The handle yaml-rust2 resolves `!!` to.
const CORE_HANDLE: &str = "tag:yaml.org,2002:";

/// The most nodes a document may hold once its aliases are expanded. A config holds a few
/// hundred; the bound stops a document whose nested aliases grow exponentially ("billion laughs")
/// before it exhausts memory, as yaml.v3's "excessive aliasing" check does.
const MAX_NODES: usize = 100_000;

/// Parse one YAML document into the JSON value `yq -p=yaml -o=json .` gives for it.
///
/// Rejected, with the key path where there is one: YAML syntax errors, zero or several documents,
/// duplicate keys, `<<` merge keys, keys that are not scalars, tags other than `!!str`, `!!map`
/// and `!!seq`, scalars yq cannot marshal (`.inf`, `.nan`, `0b…`, integers beyond i64), and
/// documents of more than [`MAX_NODES`] nodes once their aliases are expanded.
pub fn parse_yaml(text: &str) -> Result<Value, Error> {
    let mut builder = Builder::default();
    let mut parser = Parser::new_from_str(text);
    parser
        .load(&mut builder, true)
        .map_err(|error| Error::yaml(&Path::root(), error.to_string()))?;
    if let Some(error) = builder.error {
        return Err(error);
    }
    let mut documents = builder.documents.into_iter();
    let (Some(document), None) = (documents.next(), documents.next()) else {
        let message = if builder.count == 0 {
            "the input has no YAML document"
        } else {
            "the input has more than one YAML document"
        };
        return Err(Error::yaml(&Path::root(), message));
    };
    to_value(&document, &Path::root())
}

/// A YAML node, with the place it starts.
#[derive(Debug, Clone)]
enum Node {
    Scalar {
        value: String,
        style: TScalarStyle,
        tag: Option<Tag>,
        mark: Marker,
    },
    Sequence {
        items: Vec<Node>,
        tag: Option<Tag>,
        mark: Marker,
    },
    Mapping {
        entries: Vec<(Node, Node)>,
        tag: Option<Tag>,
        mark: Marker,
    },
}

impl Node {
    fn mark(&self) -> Marker {
        match self {
            Node::Scalar { mark, .. }
            | Node::Sequence { mark, .. }
            | Node::Mapping { mark, .. } => *mark,
        }
    }

    /// The number of nodes in this subtree, this one included.
    fn size(&self) -> usize {
        match self {
            Node::Scalar { .. } => 1,
            Node::Sequence { items, .. } => 1 + items.iter().map(Node::size).sum::<usize>(),
            Node::Mapping { entries, .. } => {
                1 + entries
                    .iter()
                    .map(|(key, value)| key.size() + value.size())
                    .sum::<usize>()
            }
        }
    }
}

/// A collection being built, with its anchor.
enum Open {
    Sequence(usize, Vec<Node>, Option<Tag>, Marker),
    Mapping(usize, Vec<Node>, Option<Tag>, Marker),
}

/// Builds nodes from parser events.
#[derive(Default)]
struct Builder {
    stack: Vec<Open>,
    anchors: HashMap<usize, Node>,
    documents: Vec<Node>,
    count: usize,
    /// The nodes built so far, each alias counting as the nodes it expands to.
    nodes: usize,
    error: Option<Error>,
}

impl Builder {
    /// Counts `added` nodes, and fails once the document holds more than [`MAX_NODES`].
    fn grow(&mut self, added: usize, mark: Marker) -> bool {
        self.nodes = self.nodes.saturating_add(added);
        if self.nodes > MAX_NODES {
            let message = format!("the document expands to more than {MAX_NODES} nodes");
            self.error = Some(Error::yaml(&Path::root(), at(&message, mark)));
            return false;
        }
        true
    }

    fn finish(&mut self, anchor: usize, node: Node) {
        if anchor != 0 {
            self.anchors.insert(anchor, node.clone());
        }
        match self.stack.last_mut() {
            Some(Open::Sequence(_, items, _, _) | Open::Mapping(_, items, _, _)) => {
                items.push(node)
            }
            None => self.documents.push(node),
        }
    }
}

impl MarkedEventReceiver for Builder {
    fn on_event(&mut self, event: Event, mark: Marker) {
        if self.error.is_some() {
            return;
        }
        match event {
            Event::DocumentStart => self.count += 1,
            Event::Scalar(value, style, anchor, tag) => {
                if !self.grow(1, mark) {
                    return;
                }
                let node = Node::Scalar {
                    value,
                    style,
                    tag,
                    mark,
                };
                self.finish(anchor, node);
            }
            Event::Alias(anchor) => match self.anchors.get(&anchor).map(Node::size) {
                Some(size) => {
                    // Counted before the clone, so an oversized expansion is never built.
                    if self.grow(size, mark) {
                        let node = self.anchors[&anchor].clone();
                        self.finish(0, node);
                    }
                }
                None => self.error = Some(Error::yaml(&Path::root(), at("unknown alias", mark))),
            },
            Event::SequenceStart(anchor, tag) => {
                if !self.grow(1, mark) {
                    return;
                }
                self.stack
                    .push(Open::Sequence(anchor, Vec::new(), tag, mark));
            }
            Event::MappingStart(anchor, tag) => {
                if !self.grow(1, mark) {
                    return;
                }
                self.stack
                    .push(Open::Mapping(anchor, Vec::new(), tag, mark));
            }
            Event::SequenceEnd | Event::MappingEnd => match self.stack.pop() {
                Some(Open::Sequence(anchor, items, tag, mark)) => {
                    self.finish(anchor, Node::Sequence { items, tag, mark });
                }
                Some(Open::Mapping(anchor, items, tag, mark)) => {
                    let mut entries = Vec::with_capacity(items.len() / 2);
                    let mut items = items.into_iter();
                    while let (Some(key), Some(value)) = (items.next(), items.next()) {
                        entries.push((key, value));
                    }
                    self.finish(anchor, Node::Mapping { entries, tag, mark });
                }
                None => {
                    self.error = Some(Error::yaml(&Path::root(), at("unbalanced events", mark)));
                }
            },
            Event::Nothing | Event::StreamStart | Event::StreamEnd | Event::DocumentEnd => {}
        }
    }
}

/// `message (line L, column C)`.
fn at(message: &str, mark: Marker) -> String {
    format!(
        "{message} (line {}, column {})",
        mark.line(),
        mark.col() + 1
    )
}

/// Whether `tag` is absent or is the core tag `!!<suffix>`.
fn tag_allows(tag: &Option<Tag>, suffix: &str) -> bool {
    match tag {
        None => true,
        Some(tag) => tag.handle == CORE_HANDLE && tag.suffix == suffix,
    }
}

fn tag_text(tag: &Option<Tag>) -> String {
    match tag {
        Some(tag) if tag.handle == CORE_HANDLE => format!("!!{}", tag.suffix),
        Some(tag) => format!("{}{}", tag.handle, tag.suffix),
        None => String::new(),
    }
}

fn to_value(node: &Node, path: &Path) -> Result<Value, Error> {
    match node {
        Node::Scalar {
            value,
            style,
            tag,
            mark,
        } => {
            if !tag_allows(tag, "str") {
                let message = format!("the tag {} is not supported", tag_text(tag));
                return Err(Error::yaml(path, at(&message, *mark)));
            }
            if tag.is_some() || *style != TScalarStyle::Plain {
                return Ok(Value::String(value.clone()));
            }
            resolve_plain(value).map_err(|message| Error::yaml(path, at(&message, *mark)))
        }
        Node::Sequence { items, tag, mark } => {
            if !tag_allows(tag, "seq") {
                let message = format!("the tag {} is not supported on a sequence", tag_text(tag));
                return Err(Error::yaml(path, at(&message, *mark)));
            }
            let values = items
                .iter()
                .enumerate()
                .map(|(index, item)| to_value(item, &path.index(index)))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(Value::Array(values))
        }
        Node::Mapping { entries, tag, mark } => {
            if !tag_allows(tag, "map") {
                let message = format!("the tag {} is not supported on a mapping", tag_text(tag));
                return Err(Error::yaml(path, at(&message, *mark)));
            }
            let mut map = Map::new();
            for (key, value) in entries {
                let name = key_text(key, path)?;
                let key_path = path.key(&name);
                if map.contains_key(&name) {
                    return Err(Error::yaml(&key_path, at("duplicate key", key.mark())));
                }
                let value = to_value(value, &key_path)?;
                map.insert(name, value);
            }
            Ok(Value::Object(map))
        }
    }
}

/// The JSON key yq gives a mapping key: the scalar's text as written.
fn key_text(key: &Node, path: &Path) -> Result<String, Error> {
    match key {
        Node::Scalar {
            value,
            style,
            tag,
            mark,
        } => {
            if !tag_allows(tag, "str") {
                let message = format!("the tag {} is not supported on a key", tag_text(tag));
                return Err(Error::yaml(&path.key(value), at(&message, *mark)));
            }
            if *style == TScalarStyle::Plain && tag.is_none() && value == "<<" {
                return Err(Error::yaml(
                    &path.key(value),
                    at("merge keys are not supported", *mark),
                ));
            }
            Ok(value.clone())
        }
        Node::Sequence { mark, .. } | Node::Mapping { mark, .. } => Err(Error::yaml(
            path,
            at("a mapping key must be a scalar", *mark),
        )),
    }
}

/// The JSON value of a plain, untagged scalar, or why yq cannot marshal it.
///
/// This is yaml.v3's `resolve` for the tag, followed by yq's marshalling of that tag.
fn resolve_plain(text: &str) -> Result<Value, String> {
    match text {
        "" | "~" | "null" | "Null" | "NULL" => return Ok(Value::Null),
        "true" | "True" | "TRUE" => return Ok(Value::Bool(true)),
        "false" | "False" | "FALSE" => return Ok(Value::Bool(false)),
        ".nan" | ".NaN" | ".NAN" | ".inf" | ".Inf" | ".INF" | "+.inf" | "+.Inf" | "+.INF"
        | "-.inf" | "-.Inf" | "-.INF" => {
            return Err(format!(
                "{text} is not a finite number, which JSON cannot hold"
            ));
        }
        _ => {}
    }
    match text.as_bytes()[0] {
        b'.' => {
            // yaml.v3 tries Go's ParseFloat on the text as written.
            let plain = text.replace('_', "");
            if underscores_ok(text)
                && float_form(&plain)
                && let Some(float) = finite(&plain)
            {
                return Ok(float_value(float));
            }
            Ok(Value::String(text.to_owned()))
        }
        b'0'..=b'9' | b'+' | b'-' => {
            let plain = text.replace('_', "");
            if go_int_base0(&plain) {
                return marshal_int(&plain).ok_or_else(|| {
                    format!("{text} does not convert: yq reads it as an integer it cannot marshal")
                });
            }
            if float_form(&plain)
                && let Some(float) = finite(&plain)
            {
                // yq parses the text as written, where Go allows `_` only between digits.
                if !underscores_ok(text) {
                    return Err(format!(
                        "{text} does not convert: yq cannot parse the float"
                    ));
                }
                return Ok(float_value(float));
            }
            Ok(Value::String(text.to_owned()))
        }
        _ => Ok(Value::String(text.to_owned())),
    }
}

/// Whether Go's `strconv.ParseInt(s, 0, 64)` or `ParseUint(s, 0, 64)` accepts `s`: an optional
/// sign (signed only), then `0x`, `0o` or `0b` digits, a leading-zero octal, or decimal digits.
fn go_int_base0(s: &str) -> bool {
    let (negative, unsigned) = match s.as_bytes().first() {
        Some(b'-') => (true, &s[1..]),
        Some(b'+') => (false, &s[1..]),
        _ => (false, s),
    };
    let lower = unsigned.to_ascii_lowercase();
    let (digits, radix) = if let Some(rest) = lower.strip_prefix("0x") {
        (rest, 16)
    } else if let Some(rest) = lower.strip_prefix("0o") {
        (rest, 8)
    } else if let Some(rest) = lower.strip_prefix("0b") {
        (rest, 2)
    } else if lower.len() > 1 && lower.starts_with('0') {
        (&lower[1..], 8)
    } else {
        (lower.as_str(), 10)
    };
    if digits.is_empty() || !digits.chars().all(|c| c.is_digit(radix)) {
        return false;
    }
    let Ok(magnitude) = u64::from_str_radix(digits, radix) else {
        return false;
    };
    if negative {
        magnitude <= i64::MIN.unsigned_abs()
    } else {
        // ParseInt takes up to i64::MAX; ParseUint, without a sign, up to u64::MAX.
        s.as_bytes()[0] != b'+' || magnitude <= i64::MAX as u64
    }
}

/// yq's marshalling of an `!!int`: `0x` hexadecimal, `0o` octal, and anything else decimal, each
/// within i64. `s` has its underscores removed.
fn marshal_int(s: &str) -> Option<Value> {
    let number = if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        i64::from_str_radix(hex, 16).ok()?
    } else if let Some(octal) = s.strip_prefix("0o") {
        i64::from_str_radix(octal, 8).ok()?
    } else {
        s.parse::<i64>().ok()?
    };
    Some(Value::from(number))
}

/// yaml.v3's float form: `[-+]?(\.[0-9]+|[0-9]+(\.[0-9]*)?)([eE][-+]?[0-9]+)?`.
fn float_form(s: &str) -> bool {
    let bytes = s.as_bytes();
    let mut i = 0;
    let digits = |i: &mut usize| {
        let start = *i;
        while *i < bytes.len() && bytes[*i].is_ascii_digit() {
            *i += 1;
        }
        *i > start
    };
    if matches!(bytes.first(), Some(b'-' | b'+')) {
        i += 1;
    }
    if bytes.get(i) == Some(&b'.') {
        i += 1;
        if !digits(&mut i) {
            return false;
        }
    } else {
        if !digits(&mut i) {
            return false;
        }
        if bytes.get(i) == Some(&b'.') {
            i += 1;
            digits(&mut i);
        }
    }
    if matches!(bytes.get(i), Some(b'e' | b'E')) {
        i += 1;
        if matches!(bytes.get(i), Some(b'-' | b'+')) {
            i += 1;
        }
        if !digits(&mut i) {
            return false;
        }
    }
    i == bytes.len()
}

/// Go's `underscoreOK` for a decimal number: each `_` sits between two digits.
fn underscores_ok(s: &str) -> bool {
    let bytes = s.as_bytes();
    let start = usize::from(matches!(bytes.first(), Some(b'-' | b'+')));
    let mut previous = b'^';
    for &byte in &bytes[start..] {
        let class = if byte.is_ascii_digit() {
            b'0'
        } else if byte == b'_' {
            if previous != b'0' {
                return false;
            }
            b'_'
        } else if previous == b'_' {
            return false;
        } else {
            b'!'
        };
        previous = class;
    }
    previous != b'_'
}

/// The float `s` reads as, when it is within f64's range (Go's ParseFloat fails beyond it).
fn finite(s: &str) -> Option<f64> {
    s.parse::<f64>().ok().filter(|float| float.is_finite())
}

/// The JSON value of a float as yq writes it and a JSON reader reads it back: Go writes a whole
/// float below 1e21 without a fraction or exponent, so it reads back as an integer (`-0` as 0).
fn float_value(float: f64) -> Value {
    if float.fract() == 0.0 && float.abs() < 1e21 {
        let whole = float + 0.0;
        if let Ok(value) = serde_json::from_str::<Value>(&format!("{whole}")) {
            return value;
        }
    }
    Number::from_f64(float).map_or(Value::Null, Value::Number)
}

#[cfg(test)]
mod tests {
    use super::{MAX_NODES, float_form, go_int_base0, parse_yaml, resolve_plain, underscores_ok};
    use serde_json::{Value, json};

    #[test]
    fn plain_scalars_resolve_as_yq() {
        let cases = [
            ("~", json!(null)),
            ("off", json!("off")),
            ("014", json!(14)),
            ("0o14", json!(12)),
            ("0x1F", json!(31)),
            ("1_000", json!(1000)),
            ("1e3", json!(1000)),
            ("1.0", json!(1)),
            ("-0", json!(0)),
            ("08", json!(8)),
            ("-1.5", json!(-1.5)),
            ("1e400", json!("1e400")),
            ("0o9", json!("0o9")),
            (".5_0", json!(0.5)),
            ("._5", json!("._5")),
        ];
        for (text, expected) in cases {
            assert_eq!(resolve_plain(text), Ok(expected), "{text}");
        }
    }

    #[test]
    fn unmarshallable_scalars_fail() {
        for text in [
            ".inf",
            ".nan",
            "0b11",
            "9223372036854775808",
            "-0x1F",
            "1__0.5",
        ] {
            assert!(resolve_plain(text).is_err(), "{text}");
        }
        assert!(matches!(
            resolve_plain("-9223372036854775809"),
            Ok(Value::Number(_))
        ));
    }

    #[test]
    fn alias_expansion_is_bounded() {
        // Nine levels of nine aliases each: 9^9 scalars once expanded.
        let mut text = String::from("l0: &l0 x\n");
        for level in 1..=9 {
            let aliases = vec![format!("*l{}", level - 1); 9].join(", ");
            text.push_str(&format!("l{level}: &l{level} [{aliases}]\n"));
        }
        let error = parse_yaml(&text).expect_err("an exponential expansion");
        assert!(
            error
                .to_string()
                .contains(&format!("more than {MAX_NODES} nodes")),
            "{error}"
        );
        assert_eq!(
            parse_yaml("a: &a [1, 2]\nb: [*a, *a]\n"),
            Ok(json!({"a": [1, 2], "b": [[1, 2], [1, 2]]}))
        );
    }

    #[test]
    fn helpers_follow_go() {
        assert!(go_int_base0("-9223372036854775808"));
        assert!(!go_int_base0("-9223372036854775809"));
        assert!(go_int_base0("18446744073709551615"));
        assert!(!go_int_base0("+9223372036854775808"));
        assert!(float_form("5.") && float_form(".5e3") && !float_form("1e") && !float_form("."));
        assert!(underscores_ok("1_0.5") && !underscores_ok("1_.5") && !underscores_ok("1_"));
    }
}
