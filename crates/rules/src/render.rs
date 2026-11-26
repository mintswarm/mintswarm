//! The comment bodies the factory posts, and the confinement of model, session and user text in
//! them.
//!
//! - [`inert`] and [`span`]: `decision.py`'s confinement of model and session text, one line
//!   without control or format characters or backticks, set in a bounded code span;
//! - [`comment`]: the auto-approval comment and the shadow record `decision.py` prints, with their
//!   Confidence, Hard triggers, Doubt and Package rows;
//! - [`approval`]: the TL;DR check and comment of step 1 of `plan-approval/README.md`, and the
//!   approval package the session posts;
//! - [`commands`]: the Commands section of a plan, design or decomposition approval, the only copy
//!   of it, listing every [`Command`];
//! - [`decision`]: the decision records, answers, set-backs and merge records of design #548 §7,
//!   the readers of their markers, and the shadow record's maintainer decision row.
//!
//! Deliberate divergence: [`inert`] removes the characters of the Unicode categories Cc, Cf, Zl and
//! Zp from a table written for Unicode 14.0, the version of the Python that captured the parity
//! fixture; a Python on another Unicode version may remove other code points.

pub mod approval;
pub mod comment;
pub mod decision;

use mintswarm_domain::records::Json;

use crate::command::{Command, PackageKind};
use crate::markdown::is_py_space;
use crate::py::repr;

/// The hard trigger identifiers an assessment may list, in `decision.py`'s order.
pub const HARD_TRIGGERS: [&str; 6] = [
    "criterion-not-met",
    "outside-scope",
    "not-revertible",
    "credentials",
    "weakens-safeguard",
    "undeclared-high",
];

/// The hard triggers that only route the plan to a person: listed in any band, never forcing
/// `blocked`; in `decision.py`'s order.
pub const AUTHORITY_TRIGGERS: [&str; 4] = [
    "not-revertible",
    "credentials",
    "weakens-safeguard",
    "undeclared-high",
];

/// The characters [`span`] keeps before it cuts: `decision.py`'s `SPAN_LIMIT`.
pub const SPAN_LIMIT: usize = 600;

/// What [`span`] appends to a cut text: `decision.py`'s `SPAN_MARK`.
pub const SPAN_MARK: &str = " … (truncated)";

/// The code points of the Unicode categories Cc (control), Cf (format), Zl (line separator) and
/// Zp (paragraph separator), as Unicode 14.0 assigns them.
const INERT_REMOVED: &[(u32, u32)] = &[
    (0x0000, 0x001f),
    (0x007f, 0x009f),
    (0x00ad, 0x00ad),
    (0x0600, 0x0605),
    (0x061c, 0x061c),
    (0x06dd, 0x06dd),
    (0x070f, 0x070f),
    (0x0890, 0x0891),
    (0x08e2, 0x08e2),
    (0x180e, 0x180e),
    (0x200b, 0x200f),
    (0x2028, 0x202e),
    (0x2060, 0x2064),
    (0x2066, 0x206f),
    (0xfeff, 0xfeff),
    (0xfff9, 0xfffb),
    (0x110bd, 0x110bd),
    (0x110cd, 0x110cd),
    (0x13430, 0x13438),
    (0x1bca0, 0x1bca3),
    (0x1d173, 0x1d17a),
    (0xe0001, 0xe0001),
    (0xe0020, 0xe007f),
];

/// Whether [`inert`] removes `c`: whether it is in the category Cc, Cf, Zl or Zp.
pub fn is_removed(c: char) -> bool {
    let code = u32::from(c);
    INERT_REMOVED
        .iter()
        .any(|(low, high)| (*low..=*high).contains(&code))
}

fn words(text: &str) -> impl Iterator<Item = &str> {
    text.split(is_py_space).filter(|word| !word.is_empty())
}

/// Text on one line, without control or format characters or backticks: `decision.py`'s
/// `inert`. The words, split on Python's whitespace and joined by single spaces, lose every
/// character [`is_removed`] names, their backticks become `'`, and the result is split and joined
/// again.
pub fn inert(text: &str) -> String {
    let flat = words(text).collect::<Vec<_>>().join(" ");
    let kept: String = flat
        .chars()
        .filter(|c| !is_removed(*c))
        .map(|c| if c == '`' { '\'' } else { c })
        .collect();
    words(&kept).collect::<Vec<_>>().join(" ")
}

/// Model or session text as one bounded, inert Markdown code span: `decision.py`'s `span`. The
/// [`inert`] text is cut to [`SPAN_LIMIT`] characters, right-stripped and marked with
/// [`SPAN_MARK`] when it is longer; empty text is `(empty)`.
pub fn span(text: &str) -> String {
    let flat = bounded(text);
    if flat.is_empty() {
        "`(empty)`".to_owned()
    } else {
        format!("`{flat}`")
    }
}

/// The [`inert`] text, cut and marked as [`span`] cuts it, without the code span.
pub(crate) fn bounded(text: &str) -> String {
    let flat = inert(text);
    match flat.char_indices().nth(SPAN_LIMIT) {
        Some((cut, _)) => format!("{}{SPAN_MARK}", flat[..cut].trim_end_matches(is_py_space)),
        None => flat,
    }
}

/// What a command does, as its row in the Commands section of a `kind` approval says.
fn command_text(command: Command, kind: PackageKind) -> String {
    let noun = kind.noun();
    match command {
        Command::Approve => format!(
            "approve the {noun} in the latest approval package; the comment's other lines are a note"
        ),
        Command::Reject => format!(
            "reject the {noun} in the latest approval package; the comment's other lines are a note"
        ),
        Command::Help => "reply with the commands valid on this issue now".to_owned(),
    }
}

/// The Commands section of a `kind` approval, each line ending in LF: a `<details>` block with
/// one row per [`Command::ALL`], in order, and the rule on where a command counts.
pub fn commands(kind: PackageKind) -> String {
    let mut out = String::from("<details><summary>Commands</summary>\n\n");
    for command in Command::ALL {
        out.push_str(&format!(
            "- `{}`: {}.\n",
            command.word(),
            command_text(command, kind)
        ));
    }
    out.push_str(
        "\nA command counts only when it stands alone on its own line, outside quotes and code; \
         only the first one in a comment counts.\n\n</details>\n",
    );
    out
}

/// `str(value)` in Python for a value `json.loads` returns: a string as it is, anything else as
/// its `repr`.
pub(crate) fn py_str(value: &Json) -> String {
    match value {
        Json::String(s) => s.clone(),
        other => repr(other),
    }
}

/// [`span`] of `str(value)`.
pub(crate) fn span_of(value: &Json) -> String {
    span(&py_str(value))
}

/// The items `names` as code spans, joined by `", "`.
pub(crate) fn code_list<'a>(names: impl IntoIterator<Item = &'a str>) -> String {
    names
        .into_iter()
        .map(|name| format!("`{name}`"))
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inert_flattens_and_strips() {
        assert_eq!(inert("  a\n\tb  `c`\u{200b}d\u{2028}e "), "a b 'c'd e");
        assert_eq!(inert("x\u{1c}y"), "x y");
        assert_eq!(inert("\u{200b} \u{2060}"), "");
    }

    #[test]
    fn span_cuts_by_characters() {
        let long = "é".repeat(SPAN_LIMIT + 1);
        let cut = span(&long);
        assert_eq!(cut, format!("`{}{SPAN_MARK}`", "é".repeat(SPAN_LIMIT)));
        assert_eq!(span(""), "`(empty)`");
    }

    #[test]
    fn py_str_writes_values_as_python() {
        assert_eq!(py_str(&Json::String("a".into())), "a");
        assert_eq!(py_str(&Json::Int(5)), "5");
        assert_eq!(
            py_str(&Json::Array(vec![Json::String("a".into())])),
            "['a']"
        );
    }
}
