//! The floor's view of the repository config: what `validate_config` in
//! `.mintswarm/dogfood/plan-approval/floor.py` returns, built on `mintswarm_config`'s loader,
//! which validates what `validate_config` validates.

use std::collections::BTreeMap;

use mintswarm_config::{Error as ConfigError, Mode as ConfigMode, load_repo_config};
use mintswarm_domain::records::{ExpectedJudge, Mode};
use serde_json::Value;

use crate::exit::Unusable;
use crate::py::repr_str;

/// The limit names, in the floor's order.
pub const LIMITS: [&str; 3] = ["files", "lines", "new_files"];

/// The floor's view of the config at the base, for one component.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct View {
    /// `components.<name>.paths` of every component, empty when absent.
    pub paths: BTreeMap<String, Vec<String>>,
    /// `shared`, empty when absent.
    pub shared: Vec<String>,
    /// `pipeline.plan_approval.mode`.
    pub mode: Mode,
    /// `pipeline.plan_approval.min_confidence`, 90 when absent.
    pub min_confidence: u64,
    /// The assess harness and model the config names: `agents.stages.assess` over
    /// `agents.default`, as `decision.py`'s `expected_judge` reads them.
    pub expected_judge: ExpectedJudge,
    /// `require_human.components`.
    pub rh_components: Vec<String>,
    /// `require_human.paths`.
    pub rh_paths: Vec<String>,
    /// `limits` with the component's `limits_by_component` applied, in [`LIMITS`] order.
    pub limits: [u64; 3],
    /// For each limit an override decides, `limits_by_component.<component>`.
    pub limit_sources: [Option<String>; 3],
    /// `non_production`, the floor's built-in globs when absent.
    pub non_production: Vec<String>,
    /// `gates.tests`.
    pub tests: Option<String>,
    /// `(gate name, word)` for every word of every `gates.run` command, and each word joined to
    /// the directories of the command's earlier `cd`s.
    pub gate_words: Vec<(String, String)>,
}

impl View {
    /// The limit `key`, one of [`LIMITS`], and the override that decides it, if any.
    pub fn limit(&self, key: &str) -> (u64, Option<&str>) {
        let index = LIMITS.iter().position(|k| *k == key).unwrap_or(0);
        (self.limits[index], self.limit_sources[index].as_deref())
    }
}

fn strings(value: Option<&Value>) -> Vec<String> {
    match value {
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|item| item.as_str().map(str::to_owned))
            .collect(),
        _ => Vec::new(),
    }
}

/// `agents.stages.assess` over `agents.default`: for each of the harness and the model, the
/// stage's key when it has one, else the default's; `None` for a missing key or a value that is not
/// a string. A section that is not a mapping counts as an empty one.
fn expected_judge(value: &Value) -> ExpectedJudge {
    let agents = value.get("agents").filter(|v| v.is_object());
    let default = agents
        .and_then(|a| a.get("default"))
        .filter(|v| v.is_object());
    let assess = agents
        .and_then(|a| a.get("stages"))
        .filter(|v| v.is_object())
        .and_then(|s| s.get("assess"))
        .filter(|v| v.is_object());
    let pick = |key: &str| {
        let value = match assess.and_then(|a| a.get(key)) {
            Some(value) => Some(value),
            None => default.and_then(|d| d.get(key)),
        };
        value.and_then(Value::as_str).map(str::to_owned)
    };
    ExpectedJudge {
        harness: pick("harness"),
        model: pick("model"),
    }
}

/// The unusable-input message for a config that does not load. A YAML failure is worded here
/// from its class and key path alone, since its message may carry the YAML parser's own text; a
/// schema failure keeps `mintswarm_config`'s own wording.
fn load_message(error: &ConfigError) -> String {
    match error {
        ConfigError::Yaml { path, .. } if path.is_empty() => {
            "config: .mintswarm/config.yaml at the base is not valid YAML".to_owned()
        }
        ConfigError::Yaml { path, .. } => {
            format!("config: .mintswarm/config.yaml at the base is not valid YAML at {path}")
        }
        ConfigError::Schema { .. } => format!("config: {error}"),
    }
}

/// The floor's view of the config `bytes` at the base, for `component`; unusable when the bytes
/// are not UTF-8, the config does not load, a gate command does not split or `component` is not
/// a defined component.
pub fn view(bytes: &[u8], component: &str) -> Result<View, Unusable> {
    let text = std::str::from_utf8(bytes)
        .map_err(|_| Unusable::new("config: .mintswarm/config.yaml at the base is not UTF-8"))?;
    let config = load_repo_config(text).map_err(|error| Unusable::new(load_message(&error)))?;
    let value = config.value();
    let mut paths = BTreeMap::new();
    if let Some(Value::Object(components)) = value.get("components") {
        for (name, spec) in components {
            paths.insert(name.clone(), strings(spec.get("paths")));
        }
    }
    let shared = strings(value.get("shared"));
    let gates = value.get("gates");
    let tests = gates
        .and_then(|gates| gates.get("tests"))
        .and_then(Value::as_str)
        .map(str::to_owned);
    let mut gate_words = Vec::new();
    if let Some(Value::Array(run)) = gates.and_then(|gates| gates.get("run")) {
        for (index, gate) in run.iter().enumerate() {
            let command = gate.get("command").and_then(Value::as_str).unwrap_or("");
            let name = gate
                .get("name")
                .and_then(Value::as_str)
                .map_or_else(|| format!("gates.run[{index}]"), str::to_owned);
            let words = shlex_split(command).map_err(|error| {
                Unusable::new(format!(
                    "config: gate {name}: command does not split: {error}"
                ))
            })?;
            gate_words.extend(command_words(&words).into_iter().map(|w| (name.clone(), w)));
        }
    }
    if !paths.contains_key(component) {
        return Err(Unusable::new(format!(
            "--component {} is not a component in the config",
            repr_str(component)
        )));
    }
    let approval = config.plan_approval();
    let overrides = approval.limits_by_component.get(component);
    let flat = [
        approval.limits.files,
        approval.limits.lines,
        approval.limits.new_files,
    ];
    let over = overrides.map_or([None; 3], |o| [o.files, o.lines, o.new_files]);
    let source = format!("limits_by_component.{component}");
    Ok(View {
        paths,
        shared,
        mode: match approval.mode {
            ConfigMode::Always => Mode::Always,
            ConfigMode::Shadow => Mode::Shadow,
            ConfigMode::Judged => Mode::Judged,
            ConfigMode::Never => Mode::Never,
        },
        min_confidence: approval.min_confidence,
        expected_judge: expected_judge(value),
        rh_components: approval.require_human.components.clone(),
        rh_paths: approval.require_human.paths.clone(),
        limits: [0, 1, 2].map(|i| over[i].unwrap_or(flat[i])),
        limit_sources: [0, 1, 2].map(|i| over[i].map(|_| source.clone())),
        non_production: approval.non_production.clone(),
        tests,
        gate_words,
    })
}

/// The words `validate_config` derives from one command's split `words`: each word without its
/// leading `./`s, and, after a `cd ARG`, also joined to the directories of every earlier `cd`.
pub fn command_words(words: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    let mut directory: Vec<String> = Vec::new();
    for (index, original) in words.iter().enumerate() {
        let mut word = original.as_str();
        while let Some(rest) = word.strip_prefix("./") {
            word = rest;
        }
        out.push(word.to_owned());
        if !directory.is_empty() {
            out.push(format!("{}/{word}", directory.join("/")));
        }
        if index > 0 && words[index - 1] == "cd" {
            directory.extend(
                word.split('/')
                    .filter(|segment| !matches!(*segment, "" | "."))
                    .map(str::to_owned),
            );
        }
    }
    out
}

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
    fn a_yaml_failure_is_worded_by_its_class_and_path() {
        let unbalanced = view(b"a: [1\n", "core").map(|_| ()).unwrap_err();
        assert_eq!(
            unbalanced.message(),
            "config: .mintswarm/config.yaml at the base is not valid YAML"
        );
        let duplicate = view(b"a: 1\na: 2\n", "core").map(|_| ()).unwrap_err();
        assert_eq!(
            duplicate.message(),
            "config: .mintswarm/config.yaml at the base is not valid YAML at a"
        );
    }

    #[test]
    fn cd_directories_join_later_words() {
        let words = split("cd ./web && cd ./apps && bash y.sh");
        assert_eq!(
            command_words(&words),
            [
                "cd",
                "web",
                "&&",
                "web/&&",
                "cd",
                "web/cd",
                "apps",
                "web/apps",
                "&&",
                "web/apps/&&",
                "bash",
                "web/apps/bash",
                "y.sh",
                "web/apps/y.sh",
            ]
        );
    }
}
