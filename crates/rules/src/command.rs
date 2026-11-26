//! The commands a comment carries and the approval package a comment is.
//!
//! - [`read`]: the command a comment gives, with its note, or a hint for a near miss;
//! - [`package`]: the kind and bound digest of an approval package.
//!
//! [`Command`] and [`PackageKind`] are the only copy of the command words and package markers:
//! the readers here and [`crate::render::commands`] take them from these enums.
//!
//! The reader reports what a comment says, never whether its author counts: who may approve is
//! the caller's check.

use std::fmt;

use crate::markdown::{is_py_space, py_strip, scan_fences};

/// A command a comment may give, standing alone on its own line.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Command {
    /// Approve what the latest approval package binds.
    Approve,
    /// Reject what the latest approval package binds.
    Reject,
    /// Ask for the commands valid on the issue now.
    Help,
}

impl Command {
    /// Every command, in the order the Commands section lists them.
    pub const ALL: [Command; 3] = [Command::Approve, Command::Reject, Command::Help];

    /// The word that gives the command, in lower case.
    pub const fn word(self) -> &'static str {
        match self {
            Command::Approve => "approve",
            Command::Reject => "reject",
            Command::Help => "help",
        }
    }
}

impl fmt::Display for Command {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.word())
    }
}

/// What an approval package asks the maintainer to approve.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PackageKind {
    /// A plan.
    Plan,
    /// A design.
    Design,
    /// A decomposition.
    Decomposition,
}

impl PackageKind {
    /// Every kind.
    pub const ALL: [PackageKind; 3] = [
        PackageKind::Plan,
        PackageKind::Design,
        PackageKind::Decomposition,
    ];

    /// The kind as a noun in running text: `plan`, `design` or `decomposition`.
    pub const fn noun(self) -> &'static str {
        match self {
            PackageKind::Plan => "plan",
            PackageKind::Design => "design",
            PackageKind::Decomposition => "decomposition",
        }
    }

    /// The kind at the start of the digest line: `Plan`, `Design` or `Decomposition`.
    pub const fn label(self) -> &'static str {
        match self {
            PackageKind::Plan => "Plan",
            PackageKind::Design => "Design",
            PackageKind::Decomposition => "Decomposition",
        }
    }

    /// The first line of a package of this kind.
    pub const fn marker(self) -> &'static str {
        match self {
            PackageKind::Plan => "<!-- mintswarm: plan approval package -->",
            PackageKind::Design => "<!-- mintswarm: design approval package -->",
            PackageKind::Decomposition => "<!-- mintswarm: decomposition approval package -->",
        }
    }

    /// The second line of a package of this kind, binding `sha256`: `<Label> sha256: <hex>`.
    pub fn digest_line(self, sha256: &str) -> String {
        format!("{}{DIGEST_INFIX}{sha256}", self.label())
    }
}

/// What lies between the label and the digest on a package's second line.
const DIGEST_INFIX: &str = " sha256: ";

/// The hexadecimal characters of a sha256 digest.
const DIGEST_LEN: usize = 64;

/// The most edits a word may be from a command word to get a hint.
const HINT_DISTANCE: usize = 2;

/// What a near miss is taken for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Hint {
    /// A single `/` token, which is no command: the token as written.
    NotACommand(String),
    /// A single word close to a command word: the nearest command.
    DidYouMean(Command),
}

impl fmt::Display for Hint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Hint::NotACommand(token) => write!(f, "`{token}` is not a command"),
            Hint::DidYouMean(command) => write!(f, "did you mean `{command}`?"),
        }
    }
}

/// What [`read`] finds in a comment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reading {
    /// A command line.
    Command {
        /// The command the line gives.
        command: Command,
        /// Every other line of the comment, joined by LF, without leading and trailing
        /// whitespace-only lines; the rest verbatim.
        note: String,
    },
    /// No command line, but a near miss.
    Hint(Hint),
}

/// The command `body` gives, with its note, or the hint for its first near miss, or nothing.
///
/// CRLF becomes LF. Lines in fenced blocks, `>` quote lines, their lazy continuations (non-blank
/// lines after a quote line with text, up to a blank line or a fence) and lines indented by four
/// spaces or a tab do not count. A remaining line that is, stripped, exactly one command word in
/// any case is a command line, and the first one wins.
///
/// Only when there is no command line, the first remaining line that is a single `/` token
/// (`/[A-Za-z][A-Za-z0-9-]*`) or a single word of ASCII letters within two edits of a command
/// word gives a hint; the nearest command wins, ties going to the earlier one in
/// [`Command::ALL`].
pub fn read(body: &str) -> Option<Reading> {
    let text = body.replace("\r\n", "\n");
    let lines: Vec<&str> = text.split('\n').collect();
    let mut quoted = false;
    let mut previous: Option<usize> = None;
    let mut hint = None;
    for (index, line) in scan_fences(&lines).outside {
        if previous.is_some_and(|p| index != p + 1) {
            quoted = false;
        }
        previous = Some(index);
        let stripped = py_strip(line);
        if stripped.is_empty() {
            quoted = false;
            continue;
        }
        if let Some(rest) = line.trim_start_matches(is_py_space).strip_prefix('>') {
            quoted = !py_strip(rest.trim_start_matches('>')).is_empty();
            continue;
        }
        if quoted || line.starts_with("    ") || line.starts_with('\t') {
            continue;
        }
        if let Some(command) = command_of(stripped) {
            return Some(Reading::Command {
                command,
                note: note(&lines, index),
            });
        }
        if hint.is_none() {
            hint = hint_of(stripped);
        }
    }
    hint.map(Reading::Hint)
}

/// The command `word` is, in any case.
fn command_of(word: &str) -> Option<Command> {
    Command::ALL
        .into_iter()
        .find(|command| word.eq_ignore_ascii_case(command.word()))
}

/// Every line but `skip`, joined by LF, without leading and trailing whitespace-only lines.
fn note(lines: &[&str], skip: usize) -> String {
    let rest: Vec<&str> = lines
        .iter()
        .enumerate()
        .filter(|(index, _)| *index != skip)
        .map(|(_, line)| *line)
        .collect();
    let blank = |line: &&str| py_strip(line).is_empty();
    let start = rest.iter().position(|line| !blank(line));
    let end = rest.iter().rposition(|line| !blank(line));
    match (start, end) {
        (Some(start), Some(end)) => rest[start..=end].join("\n"),
        _ => String::new(),
    }
}

/// The hint for a stripped line, when it is a `/` token or a word close to a command word.
fn hint_of(word: &str) -> Option<Hint> {
    if is_slash_token(word) {
        return Some(Hint::NotACommand(word.to_owned()));
    }
    if word.is_empty() || !word.chars().all(|c| c.is_ascii_alphabetic()) {
        return None;
    }
    let lower = word.to_ascii_lowercase();
    Command::ALL
        .into_iter()
        .map(|command| (levenshtein(&lower, command.word()), command))
        .filter(|(distance, _)| (1..=HINT_DISTANCE).contains(distance))
        .min_by_key(|(distance, _)| *distance)
        .map(|(_, command)| Hint::DidYouMean(command))
}

/// Whether `word` is `/`, an ASCII letter, then ASCII letters, digits and `-`.
fn is_slash_token(word: &str) -> bool {
    let Some(rest) = word.strip_prefix('/') else {
        return false;
    };
    let mut chars = rest.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '-')
}

/// The edit distance between two ASCII words: insertions, deletions and substitutions.
fn levenshtein(a: &str, b: &str) -> usize {
    let b = b.as_bytes();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.bytes().enumerate() {
        let mut diagonal = row[0];
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let above = row[j + 1];
            row[j + 1] = if ca == *cb {
                diagonal
            } else {
                1 + diagonal.min(above).min(row[j])
            };
            diagonal = above;
        }
    }
    row[b.len()]
}

/// What [`package`] finds in an approval package.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PackageRead {
    /// A package with a well-formed digest line.
    Current {
        /// What the package asks to approve.
        kind: PackageKind,
        /// The digest it binds, as 64 lowercase hexadecimal characters.
        sha256: String,
    },
    /// A package whose second line is missing or no well-formed digest line of its kind.
    NotCurrent(PackageKind),
}

/// The kind and bound digest of `body` when it is an approval package, or nothing.
///
/// CRLF becomes LF and each line is stripped. The first line must be a [`PackageKind::marker`];
/// the package is current when its second line is that kind's label, ` sha256: ` and 64
/// lowercase hexadecimal characters, and not current otherwise.
pub fn package(body: &str) -> Option<PackageRead> {
    let text = body.replace("\r\n", "\n");
    let mut lines = text.split('\n').map(py_strip);
    let first = lines.next()?;
    let kind = PackageKind::ALL
        .into_iter()
        .find(|kind| kind.marker() == first)?;
    let sha256 = lines
        .next()
        .and_then(|line| line.strip_prefix(kind.label()))
        .and_then(|line| line.strip_prefix(DIGEST_INFIX))
        .filter(|hex| {
            hex.len() == DIGEST_LEN
                && hex
                    .bytes()
                    .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
        });
    Some(match sha256 {
        Some(sha256) => PackageRead::Current {
            kind,
            sha256: sha256.to_owned(),
        },
        None => PackageRead::NotCurrent(kind),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levenshtein_counts_edits() {
        assert_eq!(levenshtein("approve", "approve"), 0);
        assert_eq!(levenshtein("approved", "approve"), 1);
        assert_eq!(levenshtein("rejct", "reject"), 1);
        assert_eq!(levenshtein("", "help"), 4);
        assert_eq!(levenshtein("yelp", "help"), 1);
        assert_eq!(levenshtein("kitten", "sitting"), 3);
    }
}
