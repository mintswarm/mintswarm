//! A port of Python's `shlex.split`, the one splitter of gate commands: the config schema
//! checks that every `gates.run` and formatter command splits with it, and the plan floor
//! splits gate commands into words with it. Its errors are worded here, never in Python's text.

/// Why a command does not split as Python's `shlex.split` splits it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SplitError {
    /// A quote is not closed.
    NoClosingQuotation,
    /// A backslash ends the text.
    NoEscapedCharacter,
}

impl std::fmt::Display for SplitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            SplitError::NoClosingQuotation => "a quote is not closed",
            SplitError::NoEscapedCharacter => "a backslash ends the command",
        })
    }
}

/// Python's `shlex.split(text)`: POSIX mode, comments off, whitespace splitting.
///
/// Whitespace is space, tab, carriage return and newline. Outside quotes a backslash takes the
/// next character as it is; inside double quotes it does so only before `"` or `\`, and is kept
/// otherwise; inside single quotes it is an ordinary character. An empty quoted string is a word.
pub fn shlex_split(text: &str) -> Result<Vec<String>, SplitError> {
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum State {
        Between,
        Word,
        Quote(char),
        Escape(Option<char>),
    }
    let mut words = Vec::new();
    let mut token = String::new();
    let mut quoted = false;
    let mut state = State::Between;
    for c in text.chars() {
        let space = matches!(c, ' ' | '\t' | '\r' | '\n');
        match state {
            State::Between | State::Word => {
                if space {
                    if state == State::Word {
                        words.push(std::mem::take(&mut token));
                        quoted = false;
                    }
                    state = State::Between;
                } else if c == '\\' {
                    state = State::Escape(None);
                } else if c == '\'' || c == '"' {
                    quoted = true;
                    state = State::Quote(c);
                } else {
                    token.push(c);
                    state = State::Word;
                }
            }
            State::Quote(quote) => {
                if c == quote {
                    state = State::Word;
                } else if c == '\\' && quote == '"' {
                    state = State::Escape(Some(quote));
                } else {
                    token.push(c);
                }
            }
            State::Escape(within) => {
                if let Some(quote) = within
                    && c != quote
                    && c != '\\'
                {
                    token.push('\\');
                }
                token.push(c);
                state = within.map_or(State::Word, State::Quote);
            }
        }
    }
    match state {
        State::Quote(_) => Err(SplitError::NoClosingQuotation),
        State::Escape(_) => Err(SplitError::NoEscapedCharacter),
        State::Word => {
            words.push(token);
            Ok(words)
        }
        State::Between => {
            if quoted || !token.is_empty() {
                words.push(token);
            }
            Ok(words)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn split(text: &str) -> Vec<String> {
        shlex_split(text).unwrap_or_default()
    }

    #[test]
    fn splits_as_posix_shlex() {
        assert_eq!(split("a  b\tc"), ["a", "b", "c"]);
        assert_eq!(split("a'b c'd"), ["ab cd"]);
        assert_eq!(split(r#""a\"b\\c\d""#), [r#"a"b\c\d"#]);
        assert_eq!(split(r"a\ b \x"), ["a b", "x"]);
        assert_eq!(split(r"'a\b'"), [r"a\b"]);
        assert_eq!(split("'' x"), ["", "x"]);
        assert_eq!(split("a # b"), ["a", "#", "b"]);
        assert_eq!(split("x && y"), ["x", "&&", "y"]);
        assert_eq!(shlex_split("'a"), Err(SplitError::NoClosingQuotation));
        assert_eq!(shlex_split("a\\"), Err(SplitError::NoEscapedCharacter));
    }

    #[test]
    fn finds_unclosed_quotes_and_escapes() {
        assert!(shlex_split("cd ./web && bash x.sh").is_ok());
        assert!(shlex_split(r#"echo "a \" b" 'c\'"#).is_ok());
        assert_eq!(shlex_split("echo 'a"), Err(SplitError::NoClosingQuotation));
        assert_eq!(shlex_split("echo \"a"), Err(SplitError::NoClosingQuotation));
        assert_eq!(shlex_split("echo a\\"), Err(SplitError::NoEscapedCharacter));
        assert_eq!(
            shlex_split("echo \"a\\"),
            Err(SplitError::NoEscapedCharacter)
        );
    }

    #[test]
    fn errors_display_in_the_ports_words() {
        assert_eq!(
            SplitError::NoClosingQuotation.to_string(),
            "a quote is not closed"
        );
        assert_eq!(
            SplitError::NoEscapedCharacter.to_string(),
            "a backslash ends the command"
        );
    }
}
