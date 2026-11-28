//! Strict JSON as the Python tools read and write it.
//!
//! [`read`] follows Python's `json.loads` with the tools' `object_pairs_hook` that rejects
//! duplicate keys and their `parse_constant` that rejects `NaN`, `Infinity` and `-Infinity`
//! (`strict_json` in `.mintswarm/dogfood/plan-approval/floor.py`).
//!
//! Deliberate divergences from Python, where it would accept the text:
//! - a number too large for a finite `f64` (Python reads `1e400` as infinity) is
//!   [`ReadErrorKind::NonFinite`];
//! - an integer outside `i64` and `u64` (Python's integers are unbounded) is
//!   [`ReadErrorKind::IntegerOutOfRange`], which `floor.py` checks for the footprint with a
//!   `parse_int` hook;
//! - a `\u` escape of a lone surrogate, which a Rust string cannot hold, is
//!   [`ReadErrorKind::LoneSurrogate`];
//! - nesting deeper than [`MAX_DEPTH`] containers is [`ReadErrorKind::TooDeep`], at the first
//!   container past it, where reading stops. For the footprint, `floor.py` scans for that
//!   container and decodes only the text before it, so an earlier error comes first and nothing
//!   after the container is read, and the first error is the one reported on both sides.
//!
//! The kinds are only those Python can tell apart without reading its decoder's message: every
//! `JSONDecodeError` is [`ReadErrorKind::Syntax`], at the position Python gives it. A comma
//! followed only by whitespace and a closing `]` or `}` is reported at the comma, as Python 3.13
//! and later report it.
//!
//! Python's error wording is not reproduced: a [`ReadError`] is structured, and each tool phrases
//! its own message from it.

use std::fmt;

use mintswarm_domain::records::Json;

use crate::py::repr_str;

/// The deepest nesting of arrays and objects [`read`] accepts.
pub const MAX_DEPTH: usize = 512;

/// Why a text is not strict JSON.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReadErrorKind {
    /// The text is not JSON: Python's `JSONDecodeError`, among them an unescaped control
    /// character in a string, a leading byte order mark and text after the value.
    Syntax,
    /// An object holds the key twice, found when the object closes as `object_pairs_hook` finds it.
    DuplicateKey(String),
    /// One of the constants `NaN`, `Infinity` or `-Infinity`, as written.
    Constant(String),
    /// A number too large to be a finite `f64`.
    NonFinite,
    /// A `\u` escape of a surrogate that is not half of a pair.
    LoneSurrogate,
    /// An integer outside both `i64` and `u64`.
    IntegerOutOfRange,
    /// Arrays and objects nest deeper than [`MAX_DEPTH`].
    TooDeep,
}

impl fmt::Display for ReadErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ReadErrorKind::Syntax => f.write_str("not valid JSON"),
            ReadErrorKind::DuplicateKey(key) => write!(f, "duplicate key {}", repr_str(key)),
            ReadErrorKind::Constant(name) => write!(f, "{name} is not allowed"),
            ReadErrorKind::NonFinite => f.write_str("a number is not finite"),
            ReadErrorKind::LoneSurrogate => f.write_str("a lone surrogate escape"),
            ReadErrorKind::IntegerOutOfRange => f.write_str("an integer is out of range"),
            ReadErrorKind::TooDeep => write!(f, "nested deeper than {MAX_DEPTH}"),
        }
    }
}

/// A text that is not strict JSON, and where.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReadError {
    /// Why it is not.
    pub kind: ReadErrorKind,
    /// The 1-based line of the offending character.
    pub line: usize,
    /// The 1-based column of the offending character, counted in characters as Python counts them.
    pub column: usize,
}

/// The kind alone for the kinds a Python hook detects, which carry no position there: a
/// duplicate key, a constant and an integer out of range. The kind and its line and column for
/// every other.
impl fmt::Display for ReadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.kind {
            ReadErrorKind::DuplicateKey(_)
            | ReadErrorKind::Constant(_)
            | ReadErrorKind::IntegerOutOfRange => {
                write!(f, "{}", self.kind)
            }
            _ => write!(
                f,
                "{}: line {} column {}",
                self.kind, self.line, self.column
            ),
        }
    }
}

impl std::error::Error for ReadError {}

/// Reads `text` as one strict JSON value, as Python's `json.loads` with the tools' hooks does.
///
/// Whitespace is space, tab, newline and carriage return. An integer is a [`Json::Int`] when it
/// fits in `i64` and a [`Json::UInt`] only above `i64::MAX`; a number with a fraction or an
/// exponent is a [`Json::Float`].
pub fn read(text: &str) -> Result<Json, ReadError> {
    let mut reader = Reader {
        text,
        bytes: text.as_bytes(),
        at: 0,
    };
    if text.starts_with('\u{feff}') {
        return Err(reader.syntax(0));
    }
    reader.skip_whitespace();
    let value = reader.value(0)?;
    reader.skip_whitespace();
    if reader.at < reader.bytes.len() {
        return Err(reader.syntax(reader.at));
    }
    Ok(value)
}

struct Reader<'t> {
    text: &'t str,
    bytes: &'t [u8],
    at: usize,
}

impl Reader<'_> {
    /// The error `kind` at byte offset `at`, with Python's line and character column.
    fn error(&self, kind: ReadErrorKind, at: usize) -> ReadError {
        let before = &self.text[..at];
        let line_start = before.rfind('\n').map_or(0, |i| i + 1);
        ReadError {
            kind,
            line: before.matches('\n').count() + 1,
            column: before[line_start..].chars().count() + 1,
        }
    }

    fn syntax(&self, at: usize) -> ReadError {
        self.error(ReadErrorKind::Syntax, at)
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.at).copied()
    }

    fn skip_whitespace(&mut self) {
        while let Some(b' ' | b'\t' | b'\n' | b'\r') = self.peek() {
            self.at += 1;
        }
    }

    /// Past the comma at `comma` and the whitespace after it; a closing `]` or `}` next is a
    /// trailing comma, reported at the comma.
    fn after_comma(&mut self, comma: usize) -> Result<(), ReadError> {
        self.at = comma + 1;
        self.skip_whitespace();
        match self.peek() {
            Some(b']' | b'}') => Err(self.syntax(comma)),
            _ => Ok(()),
        }
    }

    fn rest_starts_with(&self, word: &str) -> bool {
        self.bytes[self.at..].starts_with(word.as_bytes())
    }

    fn value(&mut self, depth: usize) -> Result<Json, ReadError> {
        let start = self.at;
        match self.peek() {
            Some(b'"') => self.string().map(Json::String),
            Some(b'{') => self.object(depth + 1),
            Some(b'[') => self.array(depth + 1),
            Some(b'n') if self.rest_starts_with("null") => {
                self.at += 4;
                Ok(Json::Null)
            }
            Some(b't') if self.rest_starts_with("true") => {
                self.at += 4;
                Ok(Json::Bool(true))
            }
            Some(b'f') if self.rest_starts_with("false") => {
                self.at += 5;
                Ok(Json::Bool(false))
            }
            _ => {
                for name in ["NaN", "Infinity", "-Infinity"] {
                    if self.rest_starts_with(name) {
                        return Err(self.error(ReadErrorKind::Constant(name.into()), start));
                    }
                }
                self.number()
            }
        }
    }

    fn digits(&mut self) -> usize {
        let start = self.at;
        while let Some(b'0'..=b'9') = self.peek() {
            self.at += 1;
        }
        self.at - start
    }

    /// A number by Python's grammar: `-?(0|[1-9][0-9]*)(\.[0-9]+)?([eE][-+]?[0-9]+)?`, where a
    /// fraction or exponent without digits is left unread.
    fn number(&mut self) -> Result<Json, ReadError> {
        let start = self.at;
        if self.peek() == Some(b'-') {
            self.at += 1;
        }
        match self.peek() {
            Some(b'0') => self.at += 1,
            Some(b'1'..=b'9') => {
                self.digits();
            }
            _ => return Err(self.syntax(start)),
        }
        let mut float = false;
        if self.peek() == Some(b'.') && matches!(self.bytes.get(self.at + 1), Some(b'0'..=b'9')) {
            self.at += 1;
            self.digits();
            float = true;
        }
        if let Some(b'e' | b'E') = self.peek() {
            let mark = self.at;
            self.at += 1;
            if let Some(b'+' | b'-') = self.peek() {
                self.at += 1;
            }
            if self.digits() == 0 {
                self.at = mark;
            } else {
                float = true;
            }
        }
        let text = &self.text[start..self.at];
        if float {
            match text.parse::<f64>() {
                Ok(f) if f.is_finite() => Ok(Json::Float(f)),
                _ => Err(self.error(ReadErrorKind::NonFinite, start)),
            }
        } else if let Ok(i) = text.parse::<i64>() {
            Ok(Json::Int(i))
        } else if let Ok(u) = text.parse::<u64>() {
            Ok(Json::UInt(u))
        } else {
            Err(self.error(ReadErrorKind::IntegerOutOfRange, start))
        }
    }

    fn hex4(&mut self) -> Result<u32, ReadError> {
        let digits = self
            .bytes
            .get(self.at..self.at + 4)
            .filter(|d| d.iter().all(u8::is_ascii_hexdigit))
            .ok_or_else(|| self.syntax(self.at))?;
        self.at += 4;
        let mut value = 0;
        for digit in digits {
            value = value * 16 + char::from(*digit).to_digit(16).unwrap_or(0);
        }
        Ok(value)
    }

    /// A string; the reader is on its opening quote.
    fn string(&mut self) -> Result<String, ReadError> {
        let open = self.at;
        self.at += 1;
        let mut out = String::new();
        loop {
            let run = self.at;
            while let Some(b) = self.peek() {
                if b == b'"' || b == b'\\' || b < 0x20 {
                    break;
                }
                self.at += 1;
            }
            out.push_str(&self.text[run..self.at]);
            match self.peek() {
                None => return Err(self.syntax(open)),
                Some(b'"') => {
                    self.at += 1;
                    return Ok(out);
                }
                Some(b'\\') => {
                    let escape = self.at;
                    self.at += 1;
                    let c = match self.peek() {
                        Some(b'"') => '"',
                        Some(b'\\') => '\\',
                        Some(b'/') => '/',
                        Some(b'b') => '\u{8}',
                        Some(b'f') => '\u{c}',
                        Some(b'n') => '\n',
                        Some(b'r') => '\r',
                        Some(b't') => '\t',
                        Some(b'u') => {
                            self.at += 1;
                            out.push(self.unicode_escape(escape)?);
                            continue;
                        }
                        _ => return Err(self.syntax(escape)),
                    };
                    self.at += 1;
                    out.push(c);
                }
                Some(_) => return Err(self.syntax(self.at)),
            }
        }
    }

    /// The character of a `\u` escape starting at `escape`, the reader past its `u`; a high
    /// surrogate followed by a `\u` low surrogate combines into one character.
    fn unicode_escape(&mut self, escape: usize) -> Result<char, ReadError> {
        let first = self.hex4()?;
        let code = match first {
            0xd800..=0xdbff if self.rest_starts_with("\\u") => {
                let mark = self.at;
                self.at += 2;
                let second = self.hex4()?;
                if (0xdc00..=0xdfff).contains(&second) {
                    0x10000 + ((first - 0xd800) << 10) + (second - 0xdc00)
                } else {
                    self.at = mark;
                    first
                }
            }
            _ => first,
        };
        char::from_u32(code).ok_or_else(|| self.error(ReadErrorKind::LoneSurrogate, escape))
    }

    fn enter(&self, depth: usize) -> Result<(), ReadError> {
        if depth > MAX_DEPTH {
            Err(self.error(ReadErrorKind::TooDeep, self.at))
        } else {
            Ok(())
        }
    }

    /// An array; the reader is on its `[`.
    fn array(&mut self, depth: usize) -> Result<Json, ReadError> {
        self.enter(depth)?;
        self.at += 1;
        let mut items = Vec::new();
        self.skip_whitespace();
        if self.peek() == Some(b']') {
            self.at += 1;
            return Ok(Json::Array(items));
        }
        loop {
            items.push(self.value(depth)?);
            self.skip_whitespace();
            match self.peek() {
                Some(b']') => {
                    self.at += 1;
                    return Ok(Json::Array(items));
                }
                Some(b',') => self.after_comma(self.at)?,
                _ => return Err(self.syntax(self.at)),
            }
        }
    }

    /// An object; the reader is on its `{`. Duplicate keys are checked once it closes.
    fn object(&mut self, depth: usize) -> Result<Json, ReadError> {
        self.enter(depth)?;
        self.at += 1;
        let mut entries: Vec<(String, Json)> = Vec::new();
        let mut key_offsets = Vec::new();
        self.skip_whitespace();
        if self.peek() == Some(b'}') {
            self.at += 1;
            return Ok(Json::Object(entries));
        }
        loop {
            if self.peek() != Some(b'"') {
                return Err(self.syntax(self.at));
            }
            key_offsets.push(self.at);
            let key = self.string()?;
            self.skip_whitespace();
            if self.peek() != Some(b':') {
                return Err(self.syntax(self.at));
            }
            self.at += 1;
            self.skip_whitespace();
            let value = self.value(depth)?;
            entries.push((key, value));
            self.skip_whitespace();
            match self.peek() {
                Some(b'}') => {
                    self.at += 1;
                    break;
                }
                Some(b',') => self.after_comma(self.at)?,
                _ => return Err(self.syntax(self.at)),
            }
        }
        for (index, (key, _)) in entries.iter().enumerate() {
            if entries[..index].iter().any(|(seen, _)| seen == key) {
                return Err(
                    self.error(ReadErrorKind::DuplicateKey(key.clone()), key_offsets[index])
                );
            }
        }
        Ok(Json::Object(entries))
    }
}

/// How [`dumps`] lays a value out: Python's `json.dumps` arguments.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Style {
    /// `indent`: spaces per nesting level, each item on its own line; `None` writes one line.
    pub indent: Option<usize>,
    /// `ensure_ascii`: every character outside printable ASCII is written as a `\u` escape.
    pub ensure_ascii: bool,
    /// `sort_keys`: object keys are written in code-point order.
    pub sort_keys: bool,
}

impl Style {
    /// A record file as most tools write it: `indent=2, ensure_ascii=False`.
    pub const DISK: Style = Style {
        indent: Some(2),
        ensure_ascii: false,
        sort_keys: false,
    };
    /// A record file as `keep-round.py` writes it: `indent=2` with the default `ensure_ascii`.
    pub const DISK_ASCII: Style = Style {
        indent: Some(2),
        ensure_ascii: true,
        sort_keys: false,
    };
    /// One line: `ensure_ascii=False`.
    pub const LINE: Style = Style {
        indent: None,
        ensure_ascii: false,
        sort_keys: false,
    };
    /// `floor.py`'s `canonical`: `sort_keys=True, ensure_ascii=False`.
    pub const CANONICAL: Style = Style {
        indent: None,
        ensure_ascii: false,
        sort_keys: true,
    };
}

/// `value` as Python's `json.dumps` writes it in `style`, byte for byte.
///
/// The separators are `", "` and `": "` on one line, and `","` and `": "` with an indent. A float
/// is written as Python's `repr` writes it, and a non-finite one as `NaN`, `Infinity` or
/// `-Infinity`, as Python's default `allow_nan` writes it.
pub fn dumps(value: &Json, style: Style) -> String {
    let mut out = String::new();
    write_value(&mut out, value, style, 0);
    out
}

/// [`dumps`] followed by the final newline every record file ends with.
pub fn to_file_text(value: &Json, style: Style) -> String {
    let mut out = dumps(value, style);
    out.push('\n');
    out
}

/// Why a value could not be turned into a [`Json`] tree.
#[derive(Debug)]
pub enum ToJsonError {
    /// `serde_json` could not serialise the value.
    Serialise(serde_json::Error),
    /// The serialised text is not strict JSON.
    Read(ReadError),
}

impl fmt::Display for ToJsonError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ToJsonError::Serialise(error) => write!(f, "cannot serialise: {error}"),
            ToJsonError::Read(error) => write!(f, "cannot read back: {error}"),
        }
    }
}

impl std::error::Error for ToJsonError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ToJsonError::Serialise(error) => Some(error),
            ToJsonError::Read(error) => Some(error),
        }
    }
}

/// `value` as a [`Json`] tree in its serialised key order, through `serde_json` and [`read`].
///
/// `serde_json` writes floats in shortest round-trip form, so they read back exactly.
pub fn to_json<T: serde::Serialize>(value: &T) -> Result<Json, ToJsonError> {
    let text = serde_json::to_string(value).map_err(ToJsonError::Serialise)?;
    read(&text).map_err(ToJsonError::Read)
}

fn newline(out: &mut String, style: Style, level: usize) {
    if let Some(indent) = style.indent {
        out.push('\n');
        out.extend(std::iter::repeat_n(' ', indent * level));
    }
}

fn write_value(out: &mut String, value: &Json, style: Style, level: usize) {
    let item_separator = if style.indent.is_some() { "," } else { ", " };
    match value {
        Json::Null => out.push_str("null"),
        Json::Bool(true) => out.push_str("true"),
        Json::Bool(false) => out.push_str("false"),
        Json::Int(i) => out.push_str(&i.to_string()),
        Json::UInt(u) => out.push_str(&u.to_string()),
        Json::Float(f) => out.push_str(&float_repr(*f)),
        Json::String(s) => write_string(out, s, style.ensure_ascii),
        Json::Array(items) if items.is_empty() => out.push_str("[]"),
        Json::Array(items) => {
            out.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push_str(item_separator);
                }
                newline(out, style, level + 1);
                write_value(out, item, style, level + 1);
            }
            newline(out, style, level);
            out.push(']');
        }
        Json::Object(entries) if entries.is_empty() => out.push_str("{}"),
        Json::Object(entries) => {
            let mut entries: Vec<&(String, Json)> = entries.iter().collect();
            if style.sort_keys {
                // byte order of UTF-8 is code-point order, as Python sorts str keys
                entries.sort_by(|a, b| a.0.cmp(&b.0));
            }
            out.push('{');
            for (index, (key, item)) in entries.into_iter().enumerate() {
                if index > 0 {
                    out.push_str(item_separator);
                }
                newline(out, style, level + 1);
                write_string(out, key, style.ensure_ascii);
                out.push_str(": ");
                write_value(out, item, style, level + 1);
            }
            newline(out, style, level);
            out.push('}');
        }
    }
}

/// A string as Python's `encode_basestring` or, with `ensure_ascii`, `encode_basestring_ascii`
/// writes it.
fn write_string(out: &mut String, s: &str, ensure_ascii: bool) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let escape = |out: &mut String, unit: u16| {
        out.push_str("\\u");
        for shift in [12, 8, 4, 0] {
            out.push(char::from(HEX[usize::from((unit >> shift) & 0xf)]));
        }
    };
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            '\u{0}'..='\u{1f}' => escape(out, c as u16),
            ' '..='~' => out.push(c),
            _ if !ensure_ascii => out.push(c),
            _ => {
                let mut units = [0u16; 2];
                for unit in c.encode_utf16(&mut units) {
                    escape(out, *unit);
                }
            }
        }
    }
    out.push('"');
}

/// `f` as Python's `float.__repr__` writes it: the shortest digits that read back as `f`, in
/// fixed notation with at least one fractional digit for decimal exponents from -4 to 15, and
/// otherwise as `d.ddde±XX`.
pub(crate) fn float_repr(f: f64) -> String {
    if f.is_nan() {
        return "NaN".into();
    }
    if f.is_infinite() {
        return if f > 0.0 { "Infinity" } else { "-Infinity" }.into();
    }
    // `{:e}` gives the shortest round-trip digits, as `-1.2345e-7` or `0e0`
    let scientific = format!("{:e}", f.abs());
    let (mantissa, exponent) = scientific
        .split_once('e')
        .expect("`{:e}` of a finite float always holds an exponent");
    let exponent: i32 = exponent.parse().expect("`{:e}` writes a decimal exponent");
    let digits: String = mantissa.chars().filter(|c| *c != '.').collect();
    let sign = if f.is_sign_negative() { "-" } else { "" };
    if (-4..16).contains(&exponent) {
        let point = exponent + 1;
        let body = if point <= 0 {
            format!("0.{}{digits}", "0".repeat(point.unsigned_abs() as usize))
        } else {
            let point = point as usize;
            if digits.len() <= point {
                format!("{digits}{}.0", "0".repeat(point - digits.len()))
            } else {
                format!("{}.{}", &digits[..point], &digits[point..])
            }
        };
        format!("{sign}{body}")
    } else {
        let exponent_sign = if exponent < 0 { '-' } else { '+' };
        format!(
            "{sign}{mantissa}e{exponent_sign}{:02}",
            exponent.unsigned_abs()
        )
    }
}
