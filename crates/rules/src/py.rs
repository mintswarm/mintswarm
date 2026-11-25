//! Python's string semantics, where a ported tool's messages quote what Python prints.
//!
//! - [`repr_str`] and [`repr`]: `repr()` of a `str` and of a value `json.loads` returns;
//! - [`flat`]: `floor.py`'s `flat`, text on one line cut to [`REASON_LIMIT`] characters;
//! - [`lower`], `str.lower()`.
//!
//! Deliberate divergence: Python's `str.isprintable`, which `repr` follows, rests on the full
//! Unicode category table. [`is_printable`] covers the controls, the format, separator and
//! private-use characters and the noncharacters, but not the unassigned code points, which
//! Python's `repr` escapes and this one writes as they are.

use mintswarm_domain::records::Json;

use crate::json::float_repr;
use crate::markdown::is_py_space;

/// The characters `flat` keeps: `floor.py`'s `REASON_LIMIT`.
pub const REASON_LIMIT: usize = 200;

/// The ranges of characters in the Unicode categories Cf (format), Zs other than the space, Zl,
/// Zp and Co (private use), which Python's `str.isprintable` rejects.
const NOT_PRINTABLE: &[(u32, u32)] = &[
    (0x00a0, 0x00a0),
    (0x00ad, 0x00ad),
    (0x0600, 0x0605),
    (0x061c, 0x061c),
    (0x06dd, 0x06dd),
    (0x070f, 0x070f),
    (0x0890, 0x0891),
    (0x08e2, 0x08e2),
    (0x1680, 0x1680),
    (0x180e, 0x180e),
    (0x2000, 0x200f),
    (0x2028, 0x202f),
    (0x205f, 0x2064),
    (0x2066, 0x206f),
    (0x3000, 0x3000),
    (0xe000, 0xf8ff),
    (0xfdd0, 0xfdef),
    (0xfeff, 0xfeff),
    (0xfff9, 0xfffb),
    (0x110bd, 0x110bd),
    (0x110cd, 0x110cd),
    (0x13430, 0x1343f),
    (0x1bca0, 0x1bca3),
    (0x1d173, 0x1d17a),
    (0xe0001, 0xe0001),
    (0xe0020, 0xe007f),
    (0xf0000, 0x10ffff),
];

/// Whether Python's `str.isprintable` holds for `c`, within the divergence the module names.
pub fn is_printable(c: char) -> bool {
    let code = u32::from(c);
    if c.is_control() || (code & 0xfffe) == 0xfffe {
        return false;
    }
    !NOT_PRINTABLE
        .iter()
        .any(|(low, high)| (*low..=*high).contains(&code))
}

/// `repr(s)` for a Python `str`: single quotes unless `s` holds a single quote and no double one,
/// and escapes for the backslash, the quote, `\t`, `\n`, `\r` and every character that is not
/// printable.
pub fn repr_str(s: &str) -> String {
    let quote = if s.contains('\'') && !s.contains('"') {
        '"'
    } else {
        '\''
    };
    let mut out = String::with_capacity(s.len() + 2);
    out.push(quote);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            _ if c == quote => {
                out.push('\\');
                out.push(c);
            }
            _ if is_printable(c) => out.push(c),
            _ => {
                let code = u32::from(c);
                if code <= 0xff {
                    out.push_str(&format!("\\x{code:02x}"));
                } else if code <= 0xffff {
                    out.push_str(&format!("\\u{code:04x}"));
                } else {
                    out.push_str(&format!("\\U{code:08x}"));
                }
            }
        }
    }
    out.push(quote);
    out
}

/// `repr(value)` for what Python's `json.loads` returns: `None`, `True` and `False`, integers,
/// floats as `float.__repr__` writes them, strings by [`repr_str`], and lists and dicts with
/// `", "` and `": "`.
pub fn repr(value: &Json) -> String {
    match value {
        Json::Null => "None".into(),
        Json::Bool(true) => "True".into(),
        Json::Bool(false) => "False".into(),
        Json::Int(i) => i.to_string(),
        Json::UInt(u) => u.to_string(),
        Json::Float(f) if f.is_nan() => "nan".into(),
        Json::Float(f) if f.is_infinite() => if *f > 0.0 { "inf" } else { "-inf" }.into(),
        Json::Float(f) => float_repr(*f),
        Json::String(s) => repr_str(s),
        Json::Array(items) => {
            let items: Vec<String> = items.iter().map(repr).collect();
            format!("[{}]", items.join(", "))
        }
        Json::Object(entries) => {
            let entries: Vec<String> = entries
                .iter()
                .map(|(key, item)| format!("{}: {}", repr_str(key), repr(item)))
                .collect();
            format!("{{{}}}", entries.join(", "))
        }
    }
}

/// `" ".join(text.split())[:REASON_LIMIT]`: the words of `text`, split on Python's whitespace,
/// joined by single spaces and cut to [`REASON_LIMIT`] characters.
pub fn flat(text: &str) -> String {
    let joined = text
        .split(is_py_space)
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    match joined.char_indices().nth(REASON_LIMIT) {
        Some((cut, _)) => joined[..cut].to_owned(),
        None => joined,
    }
}

/// `s.lower()`: the full Unicode lowercase, with the final sigma rule, as Python and Rust both
/// apply it.
pub fn lower(s: &str) -> String {
    s.to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repr_str_chooses_quotes_and_escapes_as_python() {
        assert_eq!(repr_str("a.md"), "'a.md'");
        assert_eq!(repr_str("it's"), "\"it's\"");
        assert_eq!(repr_str("it's \"x\""), "'it\\'s \"x\"'");
        assert_eq!(repr_str("docs\\a.md"), "'docs\\\\a.md'");
        assert_eq!(repr_str("a\tb\nc\rd"), "'a\\tb\\nc\\rd'");
        assert_eq!(
            repr_str("\u{1}\u{7f}\u{85}\u{a0}é"),
            "'\\x01\\x7f\\x85\\xa0é'"
        );
        assert_eq!(repr_str("\u{200b}\u{10ffff}"), "'\\u200b\\U0010ffff'");
    }

    #[test]
    fn repr_writes_json_values_as_python() {
        let value = Json::Object(vec![
            ("a".into(), Json::Array(vec![Json::Int(1), Json::Null])),
            ("b".into(), Json::Bool(true)),
            ("c".into(), Json::Float(1.5)),
        ]);
        assert_eq!(repr(&value), "{'a': [1, None], 'b': True, 'c': 1.5}");
        assert_eq!(repr(&Json::Object(vec![])), "{}");
        assert_eq!(repr(&Json::Float(1e16)), "1e+16");
    }

    #[test]
    fn flat_joins_and_cuts() {
        assert_eq!(flat("  a\n\tb\u{1c} c  "), "a b c");
        let long = "é".repeat(250);
        assert_eq!(flat(&long).chars().count(), REASON_LIMIT);
    }
}
