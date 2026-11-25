//! A string as Python's `repr()` writes it, as `save-output.py` quotes reply text in its faults.
//!
//! Printability follows Python 3.11 (Unicode 14.0): a character is escaped when its general
//! category is Cc, Cf, Co, Zl, Zp, or Zs other than the space. Python also escapes unassigned code
//! points (Cn), which this table does not know: such a character is written as it is here, a
//! documented divergence. Reply text holds no lone surrogate, so Cs never arises.

/// The Cf (format) characters of Unicode 14.0, as pairs of inclusive bounds.
const FORMAT: &[u32] = &[
    0xad, 0xad, 0x600, 0x605, 0x61c, 0x61c, 0x6dd, 0x6dd, 0x70f, 0x70f, 0x890, 0x891, 0x8e2, 0x8e2,
    0x180e, 0x180e, 0x200b, 0x200f, 0x202a, 0x202e, 0x2060, 0x2064, 0x2066, 0x206f, 0xfeff, 0xfeff,
    0xfff9, 0xfffb, 0x110bd, 0x110bd, 0x110cd, 0x110cd, 0x13430, 0x13438, 0x1bca0, 0x1bca3,
    0x1d173, 0x1d17a, 0xe0001, 0xe0001, 0xe0020, 0xe007f,
];

/// The Zs (space separator) characters other than the space, the Zl and Zp characters, and the
/// Co (private use) ranges, as pairs of inclusive bounds.
const SEPARATORS_AND_PRIVATE: &[u32] = &[
    0xa0, 0xa0, 0x1680, 0x1680, 0x2000, 0x200a, 0x2028, 0x2029, 0x202f, 0x202f, 0x205f, 0x205f,
    0x3000, 0x3000, 0xe000, 0xf8ff, 0xf0000, 0xffffd, 0x100000, 0x10fffd,
];

/// Whether Python's `str.isprintable` holds for `c`, but for unassigned code points.
fn is_printable(c: char) -> bool {
    let code = u32::from(c);
    let within = |bounds: &[u32]| bounds.chunks(2).any(|b| (b[0]..=b[1]).contains(&code));
    !(c.is_control() || within(FORMAT) || within(SEPARATORS_AND_PRIVATE))
}

/// `s` as Python's `repr(s)` writes it.
///
/// The quote is `'`, unless `s` holds a `'` and no `"`. Backslash, the quote, tab, newline and
/// carriage return are escaped with a backslash; other characters that are not printable are
/// written as `\xNN`, `\uNNNN` or `\UNNNNNNNN`.
pub fn py_repr(s: &str) -> String {
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
                if code < 0x100 {
                    out.push_str(&format!("\\x{code:02x}"));
                } else if code < 0x10000 {
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

/// `c` as Python's `repr(c)` writes a one-character string.
pub fn py_repr_char(c: char) -> String {
    py_repr(c.encode_utf8(&mut [0; 4]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_as_python_picks_them() {
        assert_eq!(py_repr("plain text"), "'plain text'");
        assert_eq!(py_repr(""), "''");
        assert_eq!(py_repr("a'b"), "\"a'b\"");
        assert_eq!(py_repr("a\"b"), "'a\"b'");
        assert_eq!(py_repr("a'\"b"), "'a\\'\"b'");
        assert_eq!(py_repr_char('-'), "'-'");
        assert_eq!(py_repr_char('\''), "\"'\"");
    }

    #[test]
    fn escapes_as_python_does() {
        assert_eq!(py_repr("a\\b\tc\nd\re"), "'a\\\\b\\tc\\nd\\re'");
        assert_eq!(
            py_repr("\u{0}\u{7f}\u{85}\u{a0}\u{200b}\u{e0001}\u{e000}\u{2028} é"),
            "'\\x00\\x7f\\x85\\xa0\\u200b\\U000e0001\\ue000\\u2028 é'"
        );
        assert_eq!(py_repr("\u{1c}"), "'\\x1c'");
        assert_eq!(py_repr("— §10.2 ✓ 日本"), "'— §10.2 ✓ 日本'");
    }
}
