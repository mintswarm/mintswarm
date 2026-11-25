//! JSON as Python's `json` module accepts it, for the `questions` array and transcript lines.
//!
//! Unlike [`crate::json::read`], this decoder accepts what `save-output.py` accepts, so that
//! capture reaches the same decision on the same input:
//! - `NaN`, `Infinity` and `-Infinity` are numbers;
//! - a `\u` escape of a lone surrogate is kept: the string's [`PyStr::has_surrogate`] holds, and
//!   its code points are kept for messages;
//! - [`raw_decode`] keeps the first value of a repeated key and lists the repeats, as
//!   `save-output.py`'s `JsonObject` hook does; [`loads`] keeps the last, as plain `json.loads`;
//! - an error carries only where decoding stopped, in characters, from which the caller decides
//!   whether the text was cut short. It carries no message: callers give their own.
//!
//! Numbers keep no value: only their type matters to the callers. Nesting deeper than
//! [`MAX_DEPTH`] is an error at the opening bracket.

/// The deepest nesting of arrays and objects decoded.
pub const MAX_DEPTH: usize = 512;

/// A decoded string, which may hold lone surrogates.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PyStr {
    /// The text, with each lone surrogate replaced by U+FFFD.
    text: String,
    /// Every code point, surrogates included; only set when the string holds a lone surrogate.
    code_points: Option<Vec<u32>>,
}

impl PyStr {
    fn from_code_points(points: Vec<u32>) -> PyStr {
        let mut text = String::with_capacity(points.len());
        let mut lone = false;
        for &p in &points {
            match char::from_u32(p) {
                Some(c) => text.push(c),
                None => {
                    lone = true;
                    text.push(char::REPLACEMENT_CHARACTER);
                }
            }
        }
        PyStr {
            text,
            code_points: lone.then_some(points),
        }
    }

    /// Whether the string holds a lone surrogate, which a Rust string cannot.
    pub fn has_surrogate(&self) -> bool {
        self.code_points.is_some()
    }

    /// The text, each lone surrogate replaced by U+FFFD.
    pub fn lossy(&self) -> &str {
        &self.text
    }

    /// The text, when it holds no lone surrogate.
    pub fn as_str(&self) -> Option<&str> {
        (!self.has_surrogate()).then_some(self.text.as_str())
    }

    /// Whether the string is exactly `s`.
    pub fn is(&self, s: &str) -> bool {
        self.as_str() == Some(s)
    }

    /// The string as Rust's `{:?}` writes it, a lone surrogate as `\u{d800}`.
    pub fn debug(&self) -> String {
        let Some(points) = &self.code_points else {
            return format!("{:?}", self.text);
        };
        let mut out = String::from('"');
        let mut run = String::new();
        let flush = |out: &mut String, run: &mut String| {
            let quoted = format!("{run:?}");
            out.push_str(&quoted[1..quoted.len() - 1]);
            run.clear();
        };
        for &p in points {
            match char::from_u32(p) {
                Some(c) => run.push(c),
                None => {
                    flush(&mut out, &mut run);
                    out.push_str(&format!("\\u{{{p:x}}}"));
                }
            }
        }
        flush(&mut out, &mut run);
        out.push('"');
        out
    }
}

/// A decoded JSON object: its keys in order of first appearance, each with one value, and the
/// keys that appeared more than once.
#[derive(Clone, Debug, PartialEq)]
pub struct Object {
    pub entries: Vec<(PyStr, Value)>,
    /// Each repeated key once, in order of its first repeat; empty for [`loads`].
    pub duplicates: Vec<PyStr>,
}

impl Object {
    /// The value of `key`, when present.
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.entries.iter().find(|(k, _)| k.is(key)).map(|(_, v)| v)
    }
}

/// A decoded JSON value.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    /// Any number, `NaN` and the infinities included.
    Number,
    String(PyStr),
    Array(Vec<Value>),
    Object(Object),
}

impl Value {
    /// The value's type as `save-output.py`'s `json_type` names it.
    pub fn type_name(&self) -> &'static str {
        match self {
            Value::Null => "null",
            Value::Bool(_) => "a boolean",
            Value::Number => "a number",
            Value::String(_) => "a string",
            Value::Array(_) => "an array",
            Value::Object(_) => "an object",
        }
    }

    /// The string, when the value is one.
    pub fn as_str(&self) -> Option<&PyStr> {
        match self {
            Value::String(s) => Some(s),
            _ => None,
        }
    }
}

/// Text that does not decode: where decoding stopped, a character index.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DecodeError {
    pub pos: usize,
}

/// How an object treats a repeated key.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Repeats {
    /// Keep the first value and list the repeats.
    KeepFirst,
    /// Keep the last value, at the key's first position, as a plain `dict` does.
    KeepLast,
}

/// Decodes the JSON value at the start of `text`, as `json.JSONDecoder(object_pairs_hook=...)
/// .raw_decode(text)` does with `save-output.py`'s `JsonObject`; returns the value and the index
/// just past it. Leading whitespace is not skipped.
pub fn raw_decode(text: &[char]) -> Result<(Value, usize), DecodeError> {
    Decoder {
        s: text,
        repeats: Repeats::KeepFirst,
    }
    .scan(0, 0)
}

/// Decodes `text` as `json.loads` does: one value with only whitespace around it, a repeated key
/// keeping its last value. A leading byte order mark is an error, as in Python.
pub fn loads(text: &str) -> Result<Value, DecodeError> {
    let chars: Vec<char> = text.chars().collect();
    if chars.first() == Some(&'\u{feff}') {
        return Err(DecodeError { pos: 0 });
    }
    let decoder = Decoder {
        s: &chars,
        repeats: Repeats::KeepLast,
    };
    let start = decoder.skip_whitespace(0);
    let (value, end) = decoder.scan(start, 0)?;
    let end = decoder.skip_whitespace(end);
    if end != chars.len() {
        return Err(DecodeError { pos: end });
    }
    Ok(value)
}

struct Decoder<'s> {
    s: &'s [char],
    repeats: Repeats,
}

fn error(pos: usize) -> DecodeError {
    DecodeError { pos }
}

impl Decoder<'_> {
    fn at(&self, i: usize) -> Option<char> {
        self.s.get(i).copied()
    }

    fn starts_with(&self, i: usize, word: &str) -> bool {
        let mut k = i;
        for c in word.chars() {
            if self.at(k) != Some(c) {
                return false;
            }
            k += 1;
        }
        true
    }

    fn skip_whitespace(&self, mut i: usize) -> usize {
        while let Some(' ' | '\t' | '\n' | '\r') = self.at(i) {
            i += 1;
        }
        i
    }

    fn is_digit(&self, i: usize) -> bool {
        matches!(self.at(i), Some('0'..='9'))
    }

    /// One value at `i`; a value that cannot start there is an error at `i`.
    fn scan(&self, i: usize, depth: usize) -> Result<(Value, usize), DecodeError> {
        let expecting = || error(i);
        match self.at(i) {
            None => Err(expecting()),
            Some('"') => {
                let (s, end) = self.string(i + 1)?;
                Ok((Value::String(s), end))
            }
            Some('{') if depth < MAX_DEPTH => self.object(i + 1, depth + 1),
            Some('[') if depth < MAX_DEPTH => self.array(i + 1, depth + 1),
            Some('{' | '[') => Err(expecting()),
            Some('n') if self.starts_with(i, "null") => Ok((Value::Null, i + 4)),
            Some('t') if self.starts_with(i, "true") => Ok((Value::Bool(true), i + 4)),
            Some('f') if self.starts_with(i, "false") => Ok((Value::Bool(false), i + 5)),
            Some('N') if self.starts_with(i, "NaN") => Ok((Value::Number, i + 3)),
            Some('I') if self.starts_with(i, "Infinity") => Ok((Value::Number, i + 8)),
            Some('-') if self.starts_with(i, "-Infinity") => Ok((Value::Number, i + 9)),
            _ => self.number(i).ok_or_else(expecting),
        }
    }

    /// A number by the C scanner's grammar, `-?(0|[1-9][0-9]*)(\.[0-9]+)?([eE][-+]?[0-9]+)?`,
    /// ASCII digits only.
    fn number(&self, start: usize) -> Option<(Value, usize)> {
        let mut i = start;
        if self.at(i) == Some('-') {
            i += 1;
        }
        match self.at(i) {
            Some('0') => i += 1,
            Some('1'..='9') => {
                while self.is_digit(i) {
                    i += 1;
                }
            }
            _ => return None,
        }
        if self.at(i) == Some('.') && self.is_digit(i + 1) {
            i += 1;
            while self.is_digit(i) {
                i += 1;
            }
        }
        if let Some('e' | 'E') = self.at(i) {
            let mut k = i + 1;
            if let Some('+' | '-') = self.at(k) {
                k += 1;
            }
            if self.is_digit(k) {
                while self.is_digit(k) {
                    k += 1;
                }
                i = k;
            }
        }
        Some((Value::Number, i))
    }

    /// Four hex digits after the `u` at `u`; the C scanner also wants a character after them.
    fn hex4(&self, u: usize) -> Result<u32, DecodeError> {
        let bad = || error(u);
        if u + 5 >= self.s.len() {
            return Err(bad());
        }
        let mut code = 0;
        for k in u + 1..u + 5 {
            code = code * 16 + self.s[k].to_digit(16).ok_or_else(bad)?;
        }
        Ok(code)
    }

    /// A string whose opening quote is just before `begin`; returns it and the index past its
    /// closing quote.
    fn string(&self, begin: usize) -> Result<(PyStr, usize), DecodeError> {
        let unterminated = || error(begin - 1);
        let mut points: Vec<u32> = Vec::new();
        let mut i = begin;
        loop {
            match self.at(i) {
                None => return Err(unterminated()),
                Some('"') => return Ok((PyStr::from_code_points(points), i + 1)),
                Some('\\') => {
                    let escape = i;
                    i += 1;
                    let code = match self.at(i) {
                        None => return Err(unterminated()),
                        Some('"') => u32::from('"'),
                        Some('\\') => u32::from('\\'),
                        Some('/') => u32::from('/'),
                        Some('b') => 0x8,
                        Some('f') => 0xc,
                        Some('n') => u32::from('\n'),
                        Some('r') => u32::from('\r'),
                        Some('t') => u32::from('\t'),
                        Some('u') => {
                            let first = self.hex4(i)?;
                            i += 5;
                            if (0xd800..=0xdbff).contains(&first)
                                && self.at(i) == Some('\\')
                                && self.at(i + 1) == Some('u')
                            {
                                let second = self.hex4(i + 1)?;
                                if (0xdc00..=0xdfff).contains(&second) {
                                    i += 6;
                                    points.push(
                                        0x10000 + ((first - 0xd800) << 10) + (second - 0xdc00),
                                    );
                                    continue;
                                }
                            }
                            points.push(first);
                            continue;
                        }
                        Some(_) => return Err(error(escape)),
                    };
                    points.push(code);
                    i += 1;
                }
                Some(c) if u32::from(c) < 0x20 => {
                    return Err(error(i));
                }
                Some(c) => {
                    points.push(u32::from(c));
                    i += 1;
                }
            }
        }
    }

    /// An array whose `[` is just before `i`.
    fn array(&self, i: usize, depth: usize) -> Result<(Value, usize), DecodeError> {
        let mut items = Vec::new();
        let mut i = self.skip_whitespace(i);
        if self.at(i) == Some(']') {
            return Ok((Value::Array(items), i + 1));
        }
        loop {
            let (item, end) = self.scan(i, depth)?;
            items.push(item);
            i = self.skip_whitespace(end);
            match self.at(i) {
                Some(']') => return Ok((Value::Array(items), i + 1)),
                Some(',') => i = self.skip_whitespace(i + 1),
                _ => return Err(error(i)),
            }
        }
    }

    /// An object whose `{` is just before `i`.
    fn object(&self, i: usize, depth: usize) -> Result<(Value, usize), DecodeError> {
        let mut object = Object {
            entries: Vec::new(),
            duplicates: Vec::new(),
        };
        let mut i = self.skip_whitespace(i);
        if self.at(i) == Some('}') {
            return Ok((Value::Object(object), i + 1));
        }
        loop {
            if self.at(i) != Some('"') {
                return Err(error(i));
            }
            let (key, end) = self.string(i + 1)?;
            i = self.skip_whitespace(end);
            if self.at(i) != Some(':') {
                return Err(error(i));
            }
            i = self.skip_whitespace(i + 1);
            let (value, end) = self.scan(i, depth)?;
            match object.entries.iter_mut().find(|(k, _)| *k == key) {
                None => object.entries.push((key, value)),
                Some(entry) => match self.repeats {
                    Repeats::KeepLast => entry.1 = value,
                    Repeats::KeepFirst => {
                        if !object.duplicates.contains(&key) {
                            object.duplicates.push(key);
                        }
                    }
                },
            }
            i = self.skip_whitespace(end);
            match self.at(i) {
                Some('}') => return Ok((Value::Object(object), i + 1)),
                Some(',') => i = self.skip_whitespace(i + 1),
                _ => return Err(error(i)),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode(text: &str) -> Result<(Value, usize), usize> {
        let chars: Vec<char> = text.chars().collect();
        raw_decode(&chars).map_err(|e| e.pos)
    }

    /// Where decoding stops, which decides whether the questions array was cut short: only
    /// whitespace from there on.
    #[test]
    fn errors_stop_where_the_text_breaks() {
        let cases: &[(&str, usize)] = &[
            ("[", 1),
            ("[ ", 2),
            ("[1", 2),
            ("[1,", 3),
            ("[1,]", 3),
            ("[1 2]", 3),
            ("[{", 2),
            ("[{\"a\"", 5),
            ("[{\"a\":", 6),
            ("[{\"a\":1", 7),
            ("[{\"a\":1,}", 8),
            ("[{1}", 2),
            ("[\"a", 1),
            ("[\"a\\", 1),
            ("[\"a\\\"]", 1),
            ("[\"\\x\"]", 2),
            ("[\"\\u12\"]", 3),
            ("[\"\\u1234", 3),
            ("[\"\\u1234\"", 9),
            ("[\"\\ud800\\u12\"]", 9),
            ("[\"\\ud800\\u\"]", 9),
            ("[\"\\ud800\\udc00", 9),
            ("[\"\\ud800\\udc00\n", 14),
            ("[\"\t\"]", 2),
            ("[-]", 1),
            ("[n]", 1),
            ("[tru", 1),
            ("[1.]", 2),
            ("[01]", 2),
            ("[1e", 2),
            ("[1\n,\n2", 6),
            ("[{\"a\":1}, ]", 10),
            ("[\u{661}]", 1),
            ("[NaN, Infinity, -Infinity, 1e400, 1.e5]", 35),
        ];
        for &(text, pos) in cases {
            assert_eq!(decode(text), Err(pos), "{text:?}");
        }
    }

    #[test]
    fn values_and_ends_match_python() {
        let (value, end) = decode("[]x").unwrap();
        assert_eq!((value, end), (Value::Array(vec![]), 2));
        assert_eq!(decode("[ \n ]").unwrap().1, 5);
        let (value, end) = decode("[\"\\ud800\\udc00x\"]").unwrap();
        assert_eq!(end, 17);
        let Value::Array(items) = value else { panic!() };
        assert_eq!(items[0].as_str().unwrap().as_str(), Some("\u{10000}x"));
        let (value, _) = decode("[\"q\\ud800\", \"\\ud800\\ud800\"]").unwrap();
        let Value::Array(items) = value else { panic!() };
        let lone = items[0].as_str().unwrap();
        assert!(lone.has_surrogate());
        assert_eq!(lone.debug(), "\"q\\u{d800}\"");
        assert_eq!(items[1].as_str().unwrap().debug(), "\"\\u{d800}\\u{d800}\"");
        let (value, _) = decode("[NaN, -Infinity, 1.5e3, true, null]").unwrap();
        let Value::Array(items) = value else { panic!() };
        let names: Vec<_> = items.iter().map(Value::type_name).collect();
        assert_eq!(
            names,
            ["a number", "a number", "a number", "a boolean", "null"]
        );
    }

    #[test]
    fn repeated_keys() {
        let (value, _) = decode("{\"a\": 1, \"b\": 2, \"a\": \"x\", \"a\": 3, \"b\": 4}").unwrap();
        let Value::Object(object) = value else {
            panic!()
        };
        assert_eq!(object.get("a"), Some(&Value::Number));
        let duplicates: Vec<_> = object.duplicates.iter().map(PyStr::lossy).collect();
        assert_eq!(duplicates, ["a", "b"]);
        let Value::Object(object) = loads(" {\"a\": 1, \"a\": \"x\"} \n").unwrap() else {
            panic!()
        };
        assert!(object.get("a").unwrap().as_str().unwrap().is("x"));
        assert!(object.duplicates.is_empty());
    }

    #[test]
    fn loads_rules() {
        assert_eq!(loads("NaN"), Ok(Value::Number));
        assert_eq!(loads("[1] x"), Err(DecodeError { pos: 4 }));
        assert!(loads("\u{feff}[1]").is_err());
        assert!(loads("").is_err());
    }
}
