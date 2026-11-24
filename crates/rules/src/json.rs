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
//!   [`ReadErrorKind::IntegerOutOfRange`];
//! - a `\u` escape of a lone surrogate, which a Rust string cannot hold, is
//!   [`ReadErrorKind::LoneSurrogate`];
//! - nesting deeper than [`MAX_DEPTH`] containers (Python raises `RecursionError` far deeper) is
//!   [`ReadErrorKind::TooDeep`].
//!
//! Python's error wording is not reproduced: a [`ReadError`] is structured, and each tool phrases
//! its own message from it.

use std::fmt;

use mintswarm_domain::records::Json;

/// The deepest nesting of arrays and objects [`read`] accepts.
pub const MAX_DEPTH: usize = 512;

/// Why a text is not strict JSON.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReadErrorKind {
    /// The text is not JSON at all: Python's `JSONDecodeError` for anything not listed below.
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
    /// An unescaped control character (below U+0020) inside a string.
    ControlCharacter,
    /// The text starts with a byte order mark.
    Bom,
    /// More text follows the value.
    ExtraData,
    /// Arrays and objects nest deeper than [`MAX_DEPTH`].
    TooDeep,
}

impl fmt::Display for ReadErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ReadErrorKind::Syntax => f.write_str("not valid JSON"),
            ReadErrorKind::DuplicateKey(key) => write!(f, "duplicate key {key:?}"),
            ReadErrorKind::Constant(name) => write!(f, "{name} is not allowed"),
            ReadErrorKind::NonFinite => f.write_str("a number is not finite"),
            ReadErrorKind::LoneSurrogate => f.write_str("a lone surrogate escape"),
            ReadErrorKind::IntegerOutOfRange => f.write_str("an integer is out of range"),
            ReadErrorKind::ControlCharacter => f.write_str("a control character in a string"),
            ReadErrorKind::Bom => f.write_str("a byte order mark"),
            ReadErrorKind::ExtraData => f.write_str("extra data"),
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

impl fmt::Display for ReadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}: line {} column {}",
            self.kind, self.line, self.column
        )
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
        return Err(reader.error(ReadErrorKind::Bom, 0));
    }
    reader.skip_whitespace();
    let value = reader.value(0)?;
    reader.skip_whitespace();
    if reader.at < reader.bytes.len() {
        return Err(reader.error(ReadErrorKind::ExtraData, reader.at));
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
                Some(_) => return Err(self.error(ReadErrorKind::ControlCharacter, self.at)),
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
                Some(b',') => {
                    self.at += 1;
                    self.skip_whitespace();
                }
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
                Some(b',') => {
                    self.at += 1;
                    self.skip_whitespace();
                }
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
