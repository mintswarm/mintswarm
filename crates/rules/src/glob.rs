//! Path globs with the plan floor's semantics: `glob_to_regex`, `glob_ci` and `matching` in
//! `.mintswarm/dogfood/plan-approval/floor.py`.
//!
//! A pattern is a run of tokens:
//! - `**/` matches an optional run of characters that ends in `/`;
//! - `**` matches any run of characters;
//! - `*` matches any run of characters without `/`;
//! - `?` matches one character other than `/`;
//! - every other character matches itself, so `[ab]`, `.` and `+` are literal.
//!
//! A match is a full match of the whole path, and a newline is an ordinary character, as Python's
//! `fullmatch` with `re.DOTALL` gives.
//!
//! Deliberate divergence: the case-insensitive mode compares characters by their simple Unicode
//! lowercase, where Python's `re.IGNORECASE` also folds a few extra pairs such as `ſ` and `s`.
//! ASCII paths match exactly as Python matches them.

/// One token of a pattern.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Token {
    /// `**/`: an optional run of characters ending in `/`.
    Dirs,
    /// `**`: any run of characters.
    Any,
    /// `*`: any run of characters without `/`.
    Star,
    /// `?`: one character other than `/`.
    One,
    /// A character that matches itself.
    Literal(char),
}

/// A compiled path glob.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Glob {
    tokens: Vec<Token>,
}

impl Glob {
    /// The glob `pattern`, tokenised as `glob_to_regex` reads it. Every pattern is valid.
    pub fn new(pattern: &str) -> Self {
        let chars: Vec<char> = pattern.chars().collect();
        let mut tokens = Vec::with_capacity(chars.len());
        let mut i = 0;
        while i < chars.len() {
            let rest = &chars[i..];
            if rest.starts_with(&['*', '*', '/']) {
                tokens.push(Token::Dirs);
                i += 3;
            } else if rest.starts_with(&['*', '*']) {
                tokens.push(Token::Any);
                i += 2;
            } else {
                tokens.push(match rest[0] {
                    '*' => Token::Star,
                    '?' => Token::One,
                    c => Token::Literal(c),
                });
                i += 1;
            }
        }
        Glob { tokens }
    }

    /// Whether the glob matches the whole of `path`, case-sensitively.
    pub fn matches(&self, path: &str) -> bool {
        self.full_match(path, false)
    }

    /// Whether the glob matches the whole of `path`, comparing characters by simple lowercase.
    pub fn matches_ci(&self, path: &str) -> bool {
        self.full_match(path, true)
    }

    /// A memoised full match: `rows[t][s]` holds whether `tokens[t..]` matches `path[s..]`,
    /// filled from the end of both.
    fn full_match(&self, path: &str, ci: bool) -> bool {
        let chars: Vec<char> = if ci {
            path.chars().map(lower).collect()
        } else {
            path.chars().collect()
        };
        let len = chars.len();
        let mut next = vec![false; len + 1];
        next[len] = true;
        for token in self.tokens.iter().rev() {
            let mut row = vec![false; len + 1];
            match *token {
                Token::Literal(c) => {
                    let c = if ci { lower(c) } else { c };
                    for s in 0..len {
                        row[s] = chars[s] == c && next[s + 1];
                    }
                }
                Token::One => {
                    for s in 0..len {
                        row[s] = chars[s] != '/' && next[s + 1];
                    }
                }
                Token::Star => {
                    row[len] = next[len];
                    for s in (0..len).rev() {
                        row[s] = next[s] || (chars[s] != '/' && row[s + 1]);
                    }
                }
                Token::Any => {
                    row[len] = next[len];
                    for s in (0..len).rev() {
                        row[s] = next[s] || row[s + 1];
                    }
                }
                Token::Dirs => {
                    // `slash` holds whether some `/` at or after s ends a run the rest matches.
                    let mut slash = false;
                    row[len] = next[len];
                    for s in (0..len).rev() {
                        slash = slash || (chars[s] == '/' && next[s + 1]);
                        row[s] = next[s] || slash;
                    }
                }
            }
            next = row;
        }
        next[0]
    }
}

/// The character's simple lowercase: its lowercase when that is one character. U+0130, the only
/// character whose full lowercase is two characters, has `i` as its simple lowercase.
fn lower(c: char) -> char {
    if c == '\u{130}' {
        return 'i';
    }
    let mut lowered = c.to_lowercase();
    match (lowered.next(), lowered.next()) {
        (Some(only), None) => only,
        _ => c,
    }
}

/// The globs that match `path`, in their given order; case-insensitively when `ci` is set.
pub fn matching<'g, S: AsRef<str>>(globs: &'g [S], path: &str, ci: bool) -> Vec<&'g str> {
    globs
        .iter()
        .map(AsRef::as_ref)
        .filter(|glob| {
            let glob = Glob::new(glob);
            if ci {
                glob.matches_ci(path)
            } else {
                glob.matches(path)
            }
        })
        .collect()
}
