//! Shell normalisation: the program a shell command runs, and its leading arguments.
//!
//! [`normalise`] follows five steps:
//!
//! 1. Split the command into POSIX words, honouring single quotes, double quotes and backslash
//!    escapes. A backslash before a newline continues the line, and a `#` that starts a word
//!    starts a comment, up to the end of the line.
//! 2. Split commands on `&&`, `||`, `;`, `|`, a lone `&` and an unquoted newline. Redirections
//!    (`>`, `>>`, `<`, `>|`, `<>`, `>&`, `<&`, `&>` and `&>>`, with an optional file descriptor
//!    number) and their targets are dropped.
//! 3. Drop leading `cd` commands, and a command's leading unquoted `!` (pipeline negation) and
//!    `NAME=value` words. When only `cd` commands are left, the first of them stays.
//! 4. Unwrap `env`, `command` and `time` with their options, and `env`'s `NAME=value` words, as
//!    many as stand in a row. `command -v` and `command -V` look a program up rather than run
//!    it, so they stay the program. Then unwrap `bash`, `sh` or `zsh` given `-c STRING` (or a
//!    short option cluster ending in `c`, such as `-lc`) once, by normalising STRING again. A
//!    wrapper with nothing after it is the program itself.
//! 5. The program is the basename of the first word; the arguments are the words after it.
//!
//! A command it cannot read is [`ShellUnreadable`]: an unquoted `(` or `)`, a `$(` or backtick
//! outside single quotes, an unquoted `<<`, unbalanced quotes, a trailing backslash, a
//! redirection without a target, or no program at all.

use std::fmt;

/// A shell command's program and its arguments, after normalisation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Normalised {
    /// The basename of the program.
    pub program: String,
    /// The words after the program, in order.
    pub args: Vec<String>,
}

/// A shell command the normaliser cannot read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShellUnreadable;

impl fmt::Display for ShellUnreadable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("the shell command cannot be read")
    }
}

impl std::error::Error for ShellUnreadable {}

/// The program `command` runs and its arguments, after the five steps of the module docs.
pub fn normalise(command: &str) -> Result<Normalised, ShellUnreadable> {
    normalise_at(command, true)
}

/// [`normalise`], unwrapping `bash`, `sh` and `zsh -c` only when `unwrap_shell` is set.
fn normalise_at(command: &str, unwrap_shell: bool) -> Result<Normalised, ShellUnreadable> {
    let commands = split(command)?;
    let words = first_command(commands)?;
    let words = unwrap_wrappers(words);
    if unwrap_shell && let Some(script) = shell_script(&words)? {
        return normalise_at(&script, false);
    }
    let mut words = words.into_iter();
    let program = words.next().ok_or(ShellUnreadable)?;
    let program = basename(&program.text);
    if program.is_empty() {
        return Err(ShellUnreadable);
    }
    Ok(Normalised {
        program: program.to_owned(),
        args: words.map(|word| word.text).collect(),
    })
}

/// One word of a command.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Word {
    /// The word after quote removal.
    text: String,
    /// Whether the word is a `NAME=value` assignment, its name and `=` unquoted.
    assignment: bool,
    /// Whether any part of the word was quoted or escaped.
    quoted: bool,
}

/// The tokeniser's state while it reads one word.
#[derive(Default)]
struct Current {
    text: String,
    /// Whether a word has started, which an empty quoted string does.
    started: bool,
    /// The length of the text before the first quote or escape, once one is seen.
    literal: Option<usize>,
}

impl Current {
    fn quoted(&mut self) {
        self.started = true;
        self.literal.get_or_insert(self.text.len());
    }

    fn push(&mut self, c: char) {
        self.started = true;
        self.text.push(c);
    }

    /// The finished word, if one started, leaving the state empty.
    fn take(&mut self) -> Option<Word> {
        let current = std::mem::take(self);
        if !current.started {
            return None;
        }
        let literal = current.literal.unwrap_or(current.text.len());
        let assignment = current.text[..literal]
            .find('=')
            .is_some_and(|at| is_name(&current.text[..at]));
        Some(Word {
            text: current.text,
            assignment,
            quoted: current.literal.is_some(),
        })
    }

    /// Whether the word so far is an unquoted file descriptor number, as in `2>`.
    fn is_fd(&self) -> bool {
        self.started
            && self.literal.is_none()
            && !self.text.is_empty()
            && self.text.chars().all(|c| c.is_ascii_digit())
    }
}

/// Steps 1 and 2: the commands of `command`, each a list of words, redirections dropped.
fn split(command: &str) -> Result<Vec<Vec<Word>>, ShellUnreadable> {
    let chars: Vec<char> = command.chars().collect();
    let mut commands = vec![Vec::new()];
    let mut current = Current::default();
    // Whether the next word is a redirection's target, to be dropped.
    let mut target = false;
    let mut i = 0;

    while i < chars.len() {
        let c = chars[i];
        let next = chars.get(i + 1).copied();
        match c {
            ' ' | '\t' => {
                end_word(&mut current, &mut commands, &mut target);
                i += 1;
            }
            '\n' | ';' => {
                end_word(&mut current, &mut commands, &mut target);
                separate(&mut commands, target)?;
                i += 1;
            }
            '#' if !current.started => {
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
            }
            '\'' => {
                current.quoted();
                let end = chars[i + 1..]
                    .iter()
                    .position(|&c| c == '\'')
                    .ok_or(ShellUnreadable)?;
                current.text.extend(&chars[i + 1..i + 1 + end]);
                i += end + 2;
            }
            '"' => {
                current.quoted();
                i = double_quoted(&chars, i + 1, &mut current.text)?;
            }
            '\\' => {
                match next {
                    None => return Err(ShellUnreadable),
                    // a line continuation: both characters go, and no word starts
                    Some('\n') => {}
                    Some(escaped) => {
                        current.quoted();
                        current.text.push(escaped);
                    }
                }
                i += 2;
            }
            '`' | '(' | ')' => return Err(ShellUnreadable),
            '$' if next == Some('(') => return Err(ShellUnreadable),
            '&' if next == Some('&') => {
                end_word(&mut current, &mut commands, &mut target);
                separate(&mut commands, target)?;
                i += 2;
            }
            '&' if next == Some('>') => {
                end_word(&mut current, &mut commands, &mut target);
                if target {
                    return Err(ShellUnreadable);
                }
                target = true;
                i += if chars.get(i + 2) == Some(&'>') { 3 } else { 2 };
            }
            '&' => {
                end_word(&mut current, &mut commands, &mut target);
                separate(&mut commands, target)?;
                i += 1;
            }
            '|' => {
                end_word(&mut current, &mut commands, &mut target);
                separate(&mut commands, target)?;
                i += if matches!(next, Some('|' | '&')) {
                    2
                } else {
                    1
                };
            }
            '<' if next == Some('<') => return Err(ShellUnreadable),
            '>' | '<' => {
                if current.is_fd() {
                    current = Current::default();
                } else {
                    end_word(&mut current, &mut commands, &mut target);
                }
                if target {
                    return Err(ShellUnreadable);
                }
                target = true;
                let doubled = matches!(
                    (c, next),
                    ('>', Some('>' | '|' | '&')) | ('<', Some('>' | '&'))
                );
                i += if doubled { 2 } else { 1 };
            }
            c => {
                current.push(c);
                i += 1;
            }
        }
    }
    end_word(&mut current, &mut commands, &mut target);
    if target {
        return Err(ShellUnreadable);
    }
    commands.retain(|command| !command.is_empty());
    Ok(commands)
}

/// Ends the word in progress, dropping it when it is a redirection's target.
fn end_word(current: &mut Current, commands: &mut [Vec<Word>], target: &mut bool) {
    if let Some(word) = current.take() {
        if *target {
            *target = false;
        } else if let Some(last) = commands.last_mut() {
            last.push(word);
        }
    }
}

/// Starts a new command, refusing a redirection left without its target.
fn separate(commands: &mut Vec<Vec<Word>>, target: bool) -> Result<(), ShellUnreadable> {
    if target {
        return Err(ShellUnreadable);
    }
    commands.push(Vec::new());
    Ok(())
}

/// Reads a double-quoted string from `start`, just after its opening quote, into `text`, and
/// returns the index after its closing quote.
fn double_quoted(
    chars: &[char],
    start: usize,
    text: &mut String,
) -> Result<usize, ShellUnreadable> {
    let mut i = start;
    while let Some(&c) = chars.get(i) {
        match c {
            '"' => return Ok(i + 1),
            '`' => return Err(ShellUnreadable),
            '$' if chars.get(i + 1) == Some(&'(') => return Err(ShellUnreadable),
            '\\' => match chars.get(i + 1) {
                None => return Err(ShellUnreadable),
                Some('\n') => i += 2,
                Some(&escaped @ ('"' | '\\' | '$' | '`')) => {
                    text.push(escaped);
                    i += 2;
                }
                Some(_) => {
                    text.push('\\');
                    i += 1;
                }
            },
            c => {
                text.push(c);
                i += 1;
            }
        }
    }
    Err(ShellUnreadable)
}

/// Step 3: the first command after leading `cd` commands, without its leading `!` and
/// assignments.
fn first_command(commands: Vec<Vec<Word>>) -> Result<Vec<Word>, ShellUnreadable> {
    let mut first_cd = None;
    for words in commands {
        let words: Vec<Word> = words
            .into_iter()
            .skip_while(|word| !word.quoted && word.text == "!")
            .skip_while(|word| word.assignment)
            .collect();
        match words.first() {
            None => {}
            Some(word) if word.text == "cd" => {
                first_cd.get_or_insert(words);
            }
            Some(_) => return Ok(words),
        }
    }
    first_cd.ok_or(ShellUnreadable)
}

/// The first part of step 4: `env`, `command` and `time`, with their options and `env`'s
/// assignments, dropped while they lead and something follows them.
fn unwrap_wrappers(mut words: Vec<Word>) -> Vec<Word> {
    loop {
        let Some(first) = words.first() else {
            return words;
        };
        let wrapper = basename(&first.text);
        if !matches!(wrapper, "env" | "command" | "time") {
            return words;
        }
        let mut rest = 1;
        while let Some(word) = words.get(rest) {
            let text = word.text.as_str();
            if text == "--" {
                rest += 1;
                break;
            }
            if wrapper == "env" && word.assignment {
                rest += 1;
            } else if text.starts_with('-') && text.len() > 1 {
                if wrapper == "command" && !text.starts_with("--") && text.contains(['v', 'V']) {
                    // `command -v` and `-V` look a program up rather than run it
                    return words;
                }
                let takes_value = wrapper == "env"
                    && matches!(
                        text,
                        "-u" | "--unset" | "-C" | "--chdir" | "-S" | "--split-string"
                    );
                rest += if takes_value { 2 } else { 1 };
            } else {
                break;
            }
        }
        if rest >= words.len() {
            return words;
        }
        words.drain(..rest);
    }
}

/// The second part of step 4: the STRING of `bash`, `sh` or `zsh -c STRING`, if `words` is one.
fn shell_script(words: &[Word]) -> Result<Option<String>, ShellUnreadable> {
    let Some(first) = words.first() else {
        return Ok(None);
    };
    if !matches!(basename(&first.text), "bash" | "sh" | "zsh") {
        return Ok(None);
    }
    let mut i = 1;
    while let Some(word) = words.get(i) {
        let text = word.text.as_str();
        let Some(flags) = text.strip_prefix('-').or_else(|| text.strip_prefix('+')) else {
            return Ok(None);
        };
        if flags.is_empty() || flags.starts_with('-') {
            return Ok(None);
        }
        if text.starts_with('-') && flags.ends_with('c') && flags.chars().all(char::is_alphabetic) {
            return match words.get(i + 1) {
                Some(script) => Ok(Some(script.text.clone())),
                None => Err(ShellUnreadable),
            };
        }
        i += if flags == "o" { 2 } else { 1 };
    }
    Ok(None)
}

/// The text after the last `/`.
fn basename(word: &str) -> &str {
    word.rsplit('/').next().unwrap_or(word)
}

/// Whether `name` is a shell variable name, `[A-Za-z_][A-Za-z0-9_]*`.
fn is_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

#[cfg(test)]
mod tests {
    use super::{ShellUnreadable, basename, is_name, split};

    fn words(command: &str) -> Vec<Vec<String>> {
        split(command)
            .unwrap()
            .into_iter()
            .map(|words| words.into_iter().map(|word| word.text).collect())
            .collect()
    }

    #[test]
    fn redirections_and_their_targets_are_dropped() {
        assert_eq!(words("cargo test 2>&1 > out.txt"), [["cargo", "test"]]);
        assert_eq!(words("ls &>/dev/null"), [["ls"]]);
        assert_eq!(words("wc -l <in"), [["wc", "-l"]]);
        assert_eq!(split("ls >"), Err(ShellUnreadable));
    }

    #[test]
    fn quoted_digits_before_a_redirection_stay_a_word() {
        assert_eq!(words("echo '2'>x"), [["echo", "2"]]);
    }

    #[test]
    fn assignments_need_an_unquoted_name() {
        let words = split("A=1 \"B\"=2 C=\"x y\"").unwrap();
        let flags: Vec<bool> = words[0].iter().map(|word| word.assignment).collect();
        assert_eq!(flags, [true, false, true]);
    }

    #[test]
    fn helpers() {
        assert_eq!(basename("/usr/bin/git"), "git");
        assert_eq!(basename("git"), "git");
        assert!(is_name("_A1"));
        assert!(!is_name("1A"));
        assert!(!is_name(""));
    }
}
