//! CODEOWNERS: parsing the text of a code owners file and finding the rule that owns a path.
//!
//! The rules follow GitHub's. Each line is a pattern and its owners, and a path takes the
//! last rule that matches it; a rule with no owners leaves its paths unowned. A pattern with a
//! leading `/`, or with a `/` anywhere but at its end, is anchored to the root; otherwise it
//! matches at any depth. `*` matches within one path segment, `?` one character other than
//! `/`, `**/` zero or more directories and a trailing `/**` everything below. A pattern
//! matches a file or anything under a directory of that name, one ending in `/` only what is
//! under a directory, and an anchored one whose last segment ends in a single `*` (`docs/*`,
//! `/docs/*`) only that segment: `docs/*` owns `docs/a.md` but not `docs/b/c.md`.
//!
//! Blank lines and lines starting with `#` are skipped, an owner starting with `#` starts a
//! comment, and `\#` in a pattern is a literal `#`. Negation (`!`), character classes
//! (`[...]`), other `\` escapes, empty patterns and owners that are not `@login`, `@org/team`
//! or an e-mail address make a line invalid: it is left out and reported, and the lines after
//! it still count, as GitHub skips a line with invalid syntax.
//!
//! Paths are compared case-sensitively. This module reads no file: the caller supplies the
//! text.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::tracker::{self, Login};

/// A team, `@org/slug`, stored in lower case.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Team {
    /// The organisation, in lower case.
    pub org: String,
    /// The team's slug, in lower case.
    pub slug: String,
}

impl Team {
    /// The team `org/slug`, lowered.
    pub fn new(org: &str, slug: &str) -> Self {
        Self {
            org: org.to_ascii_lowercase(),
            slug: slug.to_ascii_lowercase(),
        }
    }
}

/// A code owner named by a rule.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Owner {
    /// `@login`, without the `@`, spelt as the file spells it.
    User(Login),
    /// `@org/team`.
    Team(Team),
    /// An e-mail address.
    Email(String),
}

/// Why a line of CODEOWNERS was left out.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LineErrorKind {
    /// The pattern starts with `!`.
    Negation,
    /// The pattern holds `[` or `]`.
    CharacterClass,
    /// The pattern holds a `\` that does not escape a `#`.
    UnsupportedEscape,
    /// The pattern is empty once its slashes are taken off, such as `/`.
    EmptyPattern,
    /// An owner is not `@login`, `@org/team` or an e-mail address.
    InvalidOwner,
}

/// A line of CODEOWNERS that was left out, and why.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct LineError {
    /// The line, counted from 1.
    pub line: usize,
    /// Why it was left out.
    pub kind: LineErrorKind,
}

impl fmt::Display for LineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let why = match self.kind {
            LineErrorKind::Negation => "negation is not supported",
            LineErrorKind::CharacterClass => "character classes are not supported",
            LineErrorKind::UnsupportedEscape => "only \\# may be escaped",
            LineErrorKind::EmptyPattern => "the pattern is empty",
            LineErrorKind::InvalidOwner => "an owner is not @login, @org/team or an e-mail",
        };
        write!(f, "CODEOWNERS line {}: {why}", self.line)
    }
}

impl std::error::Error for LineError {}

/// One element of a compiled pattern.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Token {
    /// One given character.
    Literal(char),
    /// One character other than `/`.
    Any,
    /// Any run of characters other than `/`.
    Star,
    /// Nothing, or anything ending in `/`: leading directories.
    Directories,
    /// Any run of characters at all.
    Rest,
    /// Nothing, or `/` and anything after it: a directory's contents.
    Below,
}

/// A compiled pattern, matched against a whole path.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Pattern(Vec<Token>);

impl Pattern {
    /// Compiles PATTERN, already unescaped; `None` when it is empty.
    fn compile(pattern: &str) -> Option<Self> {
        let leading = pattern.starts_with('/');
        let body = if leading { &pattern[1..] } else { pattern };
        let directory = body.ends_with('/');
        let core = if directory {
            &body[..body.len() - 1]
        } else {
            body
        };
        if core.is_empty() {
            return None;
        }
        let anchored = leading || core.contains('/');
        let mut tokens = Vec::new();
        if !anchored {
            tokens.push(Token::Directories);
        }
        // `**` is special only as a whole segment; elsewhere its `*`s are plain.
        let mut rest = core;
        let mut segment_start = true;
        while let Some(c) = rest.chars().next() {
            if let Some(after) = rest.strip_prefix("**/").filter(|_| segment_start) {
                tokens.push(Token::Directories);
                rest = after;
            } else if rest == "/**" {
                tokens.extend([Token::Literal('/'), Token::Rest]);
                rest = "";
            } else {
                tokens.push(match c {
                    '*' => Token::Star,
                    '?' => Token::Any,
                    other => Token::Literal(other),
                });
                rest = &rest[c.len_utf8()..];
                segment_start = c == '/';
            }
        }
        let trailing_double_star = core == "**" || core.ends_with("/**");
        if directory {
            tokens.extend([Token::Literal('/'), Token::Rest]);
        } else if !(anchored && core.ends_with('*') && !trailing_double_star) {
            tokens.push(Token::Below);
        }
        Some(Self(tokens))
    }

    /// Whether the pattern matches the whole of PATH.
    fn matches(&self, path: &str) -> bool {
        let text: Vec<char> = path.chars().collect();
        let n = text.len();
        // The positions in `text` that the tokens so far can end at.
        let mut reach = vec![false; n + 1];
        reach[0] = true;
        for token in &self.0 {
            let mut next = vec![false; n + 1];
            for start in (0..=n).filter(|&p| reach[p]) {
                match *token {
                    Token::Literal(c) => {
                        if text.get(start) == Some(&c) {
                            next[start + 1] = true;
                        }
                    }
                    Token::Any => {
                        if text.get(start).is_some_and(|&c| c != '/') {
                            next[start + 1] = true;
                        }
                    }
                    Token::Star => {
                        next[start] = true;
                        for end in start..n {
                            if text[end] == '/' {
                                break;
                            }
                            next[end + 1] = true;
                        }
                    }
                    Token::Directories => {
                        next[start] = true;
                        for end in start..n {
                            if text[end] == '/' {
                                next[end + 1] = true;
                            }
                        }
                    }
                    Token::Rest => next[start..].fill(true),
                    Token::Below => {
                        next[start] = true;
                        if text.get(start) == Some(&'/') {
                            next[start + 1..].fill(true);
                        }
                    }
                }
            }
            reach = next;
        }
        reach[n]
    }
}

/// A valid line of CODEOWNERS.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rule {
    /// The line, counted from 1.
    pub line: usize,
    /// The pattern as the file writes it, escapes included.
    pub pattern: String,
    /// The owners, in the file's order; empty for a rule that leaves its paths unowned.
    pub owners: Vec<Owner>,
    compiled: Pattern,
}

impl Rule {
    /// Whether the rule's pattern matches PATH.
    pub fn matches(&self, path: &str) -> bool {
        self.compiled.matches(path)
    }
}

/// The valid rules of a CODEOWNERS file, in the file's order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CodeOwners {
    rules: Vec<Rule>,
}

impl CodeOwners {
    /// The rules, in the file's order.
    pub fn rules(&self) -> &[Rule] {
        &self.rules
    }

    /// The last rule that matches PATH, which decides its owners, or `None` when no rule does.
    pub fn matching_rule(&self, path: &str) -> Option<&Rule> {
        self.rules.iter().rev().find(|rule| rule.matches(path))
    }
}

/// A parsed CODEOWNERS file: the valid rules and the lines left out.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Parsed {
    /// The valid rules.
    pub rules: CodeOwners,
    /// The lines left out, in the file's order.
    pub errors: Vec<LineError>,
}

/// Parses the text of a CODEOWNERS file.
pub fn parse(text: &str) -> Parsed {
    let mut parsed = Parsed::default();
    for (index, raw) in text.lines().enumerate() {
        let line = index + 1;
        let stripped = raw.trim();
        if stripped.is_empty() || stripped.starts_with('#') {
            continue;
        }
        match parse_line(line, stripped) {
            Ok(rule) => parsed.rules.rules.push(rule),
            Err(kind) => parsed.errors.push(LineError { line, kind }),
        }
    }
    parsed
}

fn parse_line(line: usize, text: &str) -> Result<Rule, LineErrorKind> {
    let mut tokens = text.split_whitespace();
    let pattern = tokens.next().unwrap_or_default();
    let owners = tokens
        .take_while(|token| !token.starts_with('#'))
        .map(owner)
        .collect::<Option<Vec<_>>>()
        .ok_or(LineErrorKind::InvalidOwner)?;
    if pattern.starts_with('!') {
        return Err(LineErrorKind::Negation);
    }
    if pattern.contains(['[', ']']) {
        return Err(LineErrorKind::CharacterClass);
    }
    let unescaped = unescape(pattern).ok_or(LineErrorKind::UnsupportedEscape)?;
    let compiled = Pattern::compile(&unescaped).ok_or(LineErrorKind::EmptyPattern)?;
    Ok(Rule {
        line,
        pattern: pattern.to_owned(),
        owners,
        compiled,
    })
}

/// PATTERN with each `\#` made `#`; `None` when a `\` escapes anything else.
fn unescape(pattern: &str) -> Option<String> {
    let mut out = String::with_capacity(pattern.len());
    let mut chars = pattern.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            if chars.next() != Some('#') {
                return None;
            }
            out.push('#');
        } else {
            out.push(c);
        }
    }
    Some(out)
}

fn is_login(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

fn is_bot_login(s: &str) -> bool {
    s.len() > 5
        && s.is_char_boundary(s.len() - 5)
        && tracker::is_bot(s, None)
        && is_login(&s[..s.len() - 5])
}

fn is_slug(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

/// The owner TOKEN names, or `None` when it is not one.
fn owner(token: &str) -> Option<Owner> {
    if let Some(name) = token.strip_prefix('@') {
        if let Some((org, slug)) = name.split_once('/') {
            return (is_login(org) && is_slug(slug)).then(|| Owner::Team(Team::new(org, slug)));
        }
        return (is_login(name) || is_bot_login(name)).then(|| Owner::User(Login(name.to_owned())));
    }
    let (local, domain) = token.split_once('@')?;
    let plain = |s: &str| !s.is_empty() && !s.contains('@');
    (plain(local) && plain(domain)).then(|| Owner::Email(token.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::{Pattern, unescape};

    #[test]
    fn star_stays_within_a_segment() {
        let pattern = Pattern::compile("/a*b").unwrap();
        assert!(pattern.matches("axxb"));
        assert!(!pattern.matches("ax/xb"));
    }

    #[test]
    fn unescape_takes_only_hash() {
        assert_eq!(unescape(r"\#a").as_deref(), Some("#a"));
        assert_eq!(unescape(r"a\b"), None);
        assert_eq!(unescape("a\\"), None);
    }
}
