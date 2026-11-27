//! The one lexical reading of migration SQL: statements split from a file's text, and each
//! statement's kind.
//!
//! The lexer removes comments, blanks string literals and leaves dollar-quoted bodies unread,
//! so nothing inside them is ever seen as SQL. It splits only on a `;` outside all of these, and
//! fails closed on text it cannot split safely. This file uses `std` only.

use std::error::Error;
use std::fmt;

/// One token of a statement.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Token {
    /// An unquoted keyword, identifier or number, folded to ASCII lower case.
    Word(String),
    /// A quoted identifier, `"…"` or `U&"…"`, as written between the quotes, with each doubled
    /// `""` read as one `"`.
    Quoted(String),
    /// A string literal of any form, blanked.
    Literal,
    /// A dollar-quoted body, `$tag$…$tag$`, unread.
    DollarBody,
    /// A positional parameter such as `$1`, with its digits.
    Param(String),
    /// Any other character, such as `(`, `,` or `.`.
    Symbol(char),
}

/// One statement of a file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Statement {
    /// The line its first token is on, counted from 1.
    pub line: usize,
    /// Its source with comments removed and each whitespace run collapsed to one space, without
    /// the closing `;`.
    pub text: String,
    /// Its tokens, never empty.
    pub tokens: Vec<Token>,
}

/// Why a file's text cannot be split safely. Each error carries the line, counted from 1, where
/// the offending text starts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LexError {
    /// A `/*` comment is not closed.
    UnterminatedComment {
        /// The line of the opening `/*`.
        line: usize,
    },
    /// A string literal is not closed.
    UnterminatedLiteral {
        /// The line of the opening quote.
        line: usize,
    },
    /// A quoted identifier is not closed.
    UnterminatedQuotedIdentifier {
        /// The line of the opening quote.
        line: usize,
    },
    /// A dollar quote has no closing tag.
    UnterminatedDollarQuote {
        /// The line of the opening tag.
        line: usize,
    },
    /// A `BEGIN ATOMIC` body, whose `;` the lexer cannot tell from a statement's end.
    AtomicBody {
        /// The line of `atomic`.
        line: usize,
    },
}

impl LexError {
    /// The line where the offending text starts, counted from 1.
    pub fn line(&self) -> usize {
        match self {
            Self::UnterminatedComment { line }
            | Self::UnterminatedLiteral { line }
            | Self::UnterminatedQuotedIdentifier { line }
            | Self::UnterminatedDollarQuote { line }
            | Self::AtomicBody { line } => *line,
        }
    }
}

impl fmt::Display for LexError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnterminatedComment { line } => {
                write!(f, "line {line}: the block comment is not closed")
            }
            Self::UnterminatedLiteral { line } => {
                write!(f, "line {line}: the string literal is not closed")
            }
            Self::UnterminatedQuotedIdentifier { line } => {
                write!(f, "line {line}: the quoted identifier is not closed")
            }
            Self::UnterminatedDollarQuote { line } => {
                write!(f, "line {line}: the dollar quote has no closing tag")
            }
            Self::AtomicBody { line } => write!(
                f,
                "line {line}: a BEGIN ATOMIC body cannot be split lexically; use a dollar-quoted body"
            ),
        }
    }
}

impl Error for LexError {}

/// What a statement does, by its leading keyword.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Anything that is neither data nor a grant: schema changes and unknown statements.
    Ddl,
    /// `INSERT`, `UPDATE`, `DELETE`, `MERGE`, `COPY`, `SELECT`, `WITH`, `DO` or `CALL`.
    Data,
    /// `GRANT`, `REVOKE` or `ALTER DEFAULT PRIVILEGES`.
    GrantOrRevoke,
}

/// The leading keywords of a data statement.
const DATA: &[&str] = &[
    "insert", "update", "delete", "merge", "copy", "select", "with", "do", "call",
];

/// Classifies a statement by its leading keyword. Data and grants are closed lists, so anything
/// else, an unknown keyword included, is DDL.
pub fn classify(statement: &Statement) -> Kind {
    let word = |at: usize| match statement.tokens.get(at) {
        Some(Token::Word(word)) => Some(word.as_str()),
        _ => None,
    };
    match word(0) {
        Some(first) if DATA.contains(&first) => Kind::Data,
        Some("grant" | "revoke") => Kind::GrantOrRevoke,
        Some("alter") if word(1) == Some("default") && word(2) == Some("privileges") => {
            Kind::GrantOrRevoke
        }
        _ => Kind::Ddl,
    }
}

/// Splits a file's text into its statements, in order. Empty statements are dropped, and text
/// after the last `;` is a statement.
pub fn split(text: &str) -> Result<Vec<Statement>, LexError> {
    let mut lexer = Lexer {
        chars: text.chars().collect(),
        at: 0,
        line: 1,
    };
    let mut statements = Vec::new();
    let mut current = Building::default();
    loop {
        let gap = lexer.skip_gap()?;
        let Some(next) = lexer.peek(0) else {
            break;
        };
        if next == ';' {
            lexer.bump();
            current.finish(&mut statements);
            continue;
        }
        let line = lexer.line;
        let start = lexer.at;
        let token = lexer.token()?;
        let raw: String = lexer.chars[start..lexer.at].iter().collect();
        current.push(token, &raw, line, gap)?;
    }
    current.finish(&mut statements);
    Ok(statements)
}

/// The statement being read.
#[derive(Default)]
struct Building {
    line: usize,
    text: String,
    tokens: Vec<Token>,
}

impl Building {
    /// Adds a token, refusing `begin` directly followed by `atomic`.
    fn push(&mut self, token: Token, raw: &str, line: usize, gap: bool) -> Result<(), LexError> {
        if matches!(&token, Token::Word(word) if word == "atomic")
            && matches!(self.tokens.last(), Some(Token::Word(word)) if word == "begin")
        {
            return Err(LexError::AtomicBody { line });
        }
        if self.tokens.is_empty() {
            self.line = line;
        } else if gap {
            self.text.push(' ');
        }
        self.text.push_str(raw);
        self.tokens.push(token);
        Ok(())
    }

    /// Ends the statement, keeping it when it has a token.
    fn finish(&mut self, statements: &mut Vec<Statement>) {
        let building = std::mem::take(self);
        if !building.tokens.is_empty() {
            statements.push(Statement {
                line: building.line,
                text: building
                    .text
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" "),
                tokens: building.tokens,
            });
        }
    }
}

/// A cursor over a file's characters that counts lines.
struct Lexer {
    chars: Vec<char>,
    at: usize,
    line: usize,
}

impl Lexer {
    fn peek(&self, ahead: usize) -> Option<char> {
        self.chars.get(self.at + ahead).copied()
    }

    /// Moves past one character, counting a newline.
    fn bump(&mut self) -> Option<char> {
        let next = self.peek(0)?;
        if next == '\n' {
            self.line += 1;
        }
        self.at += 1;
        Some(next)
    }

    fn bump_by(&mut self, count: usize) {
        for _ in 0..count {
            self.bump();
        }
    }

    /// Skips whitespace and comments, and says whether there were any.
    fn skip_gap(&mut self) -> Result<bool, LexError> {
        let mut gap = false;
        loop {
            match (self.peek(0), self.peek(1)) {
                (Some(next), _) if next.is_whitespace() => {
                    self.bump();
                }
                (Some('-'), Some('-')) => {
                    while self.peek(0).is_some_and(|next| next != '\n') {
                        self.bump();
                    }
                }
                (Some('/'), Some('*')) => self.block_comment()?,
                _ => return Ok(gap),
            }
            gap = true;
        }
    }

    /// Skips a `/* */` comment, which nests.
    fn block_comment(&mut self) -> Result<(), LexError> {
        let line = self.line;
        self.bump_by(2);
        let mut depth = 1usize;
        while depth > 0 {
            match (self.peek(0), self.peek(1)) {
                (None, _) => return Err(LexError::UnterminatedComment { line }),
                (Some('/'), Some('*')) => {
                    self.bump_by(2);
                    depth += 1;
                }
                (Some('*'), Some('/')) => {
                    self.bump_by(2);
                    depth -= 1;
                }
                _ => {
                    self.bump();
                }
            }
        }
        Ok(())
    }

    /// Reads the token at the cursor, which is not whitespace, a comment or `;`.
    fn token(&mut self) -> Result<Token, LexError> {
        let line = self.line;
        let first = self.peek(0).unwrap_or(' ');
        let second = self.peek(1);
        let third = self.peek(2);
        match (first, second, third) {
            ('\'', _, _) => {
                self.bump();
                self.literal(line, false)
            }
            ('e' | 'E', Some('\''), _) => {
                self.bump_by(2);
                self.literal(line, true)
            }
            ('b' | 'B' | 'x' | 'X' | 'n' | 'N', Some('\''), _) => {
                self.bump_by(2);
                self.literal(line, false)
            }
            ('u' | 'U', Some('&'), Some('\'')) => {
                self.bump_by(3);
                self.literal(line, false)
            }
            ('u' | 'U', Some('&'), Some('"')) => {
                self.bump_by(3);
                self.quoted(line)
            }
            ('"', _, _) => {
                self.bump();
                self.quoted(line)
            }
            ('$', _, _) => self.dollar(line),
            (first, _, _) if is_word_start(first) => {
                let mut word = String::new();
                while let Some(next) = self.peek(0).filter(|next| is_word_char(*next)) {
                    word.push(next.to_ascii_lowercase());
                    self.bump();
                }
                Ok(Token::Word(word))
            }
            (first, _, _) => {
                self.bump();
                Ok(Token::Symbol(first))
            }
        }
    }

    /// Reads a string literal after its opening quote. A doubled `''` is a quote, and with
    /// `escapes` a backslash escapes the next character.
    fn literal(&mut self, line: usize, escapes: bool) -> Result<Token, LexError> {
        loop {
            match self.bump() {
                None => return Err(LexError::UnterminatedLiteral { line }),
                Some('\\') if escapes => {
                    if self.bump().is_none() {
                        return Err(LexError::UnterminatedLiteral { line });
                    }
                }
                Some('\'') if self.peek(0) == Some('\'') => {
                    self.bump();
                }
                Some('\'') => return Ok(Token::Literal),
                Some(_) => {}
            }
        }
    }

    /// Reads a quoted identifier after its opening quote. A doubled `""` is a quote.
    fn quoted(&mut self, line: usize) -> Result<Token, LexError> {
        let mut name = String::new();
        loop {
            match self.bump() {
                None => return Err(LexError::UnterminatedQuotedIdentifier { line }),
                Some('"') if self.peek(0) == Some('"') => {
                    self.bump();
                    name.push('"');
                }
                Some('"') => return Ok(Token::Quoted(name)),
                Some(next) => name.push(next),
            }
        }
    }

    /// Reads a token starting with `$`: a parameter, a dollar-quoted body, or a lone symbol.
    fn dollar(&mut self, line: usize) -> Result<Token, LexError> {
        if self.peek(1).is_some_and(|next| next.is_ascii_digit()) {
            self.bump();
            let mut digits = String::new();
            while let Some(next) = self.peek(0).filter(char::is_ascii_digit) {
                digits.push(next);
                self.bump();
            }
            return Ok(Token::Param(digits));
        }
        let mut end = self.at + 1;
        while self
            .chars
            .get(end)
            .is_some_and(|next| next.is_alphanumeric() || *next == '_')
        {
            end += 1;
        }
        if self.chars.get(end) != Some(&'$') {
            self.bump();
            return Ok(Token::Symbol('$'));
        }
        let tag: Vec<char> = self.chars[self.at..=end].to_vec();
        self.bump_by(tag.len());
        loop {
            if self.chars[self.at..].starts_with(&tag) {
                self.bump_by(tag.len());
                return Ok(Token::DollarBody);
            }
            if self.bump().is_none() {
                return Err(LexError::UnterminatedDollarQuote { line });
            }
        }
    }
}

/// Whether a character starts an unquoted word: a letter, a digit or `_`.
fn is_word_start(next: char) -> bool {
    next.is_alphanumeric() || next == '_'
}

/// Whether a character continues an unquoted word: a letter, a digit, `_` or `$`.
fn is_word_char(next: char) -> bool {
    next.is_alphanumeric() || next == '_' || next == '$'
}
