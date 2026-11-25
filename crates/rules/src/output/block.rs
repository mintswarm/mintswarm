//! The result block's content, in `save-output.py`'s strict subset of YAML, and its checks.
//!
//! Each function ports the Python function of the same name, decision for decision: the same
//! input breaks the same rule. The messages are this module's own. Text quoted from the reply is
//! written by Rust's `{:?}`, never by Python's `repr`, and a questions array that does not decode
//! is reported without the decoder's wording or position. Python's `str.strip()` is [`py_strip`],
//! and Rust's `trim` decides emptiness where the Python uses `rust_blank`. Reply text holds no
//! lone surrogate, so only decoded JSON strings are checked for one.

use mintswarm_domain::question::Question;
use mintswarm_domain::records::Json;
use mintswarm_domain::work::{OutcomeStatus, StageOutcome};

use super::pyjson::{self, Object, PyStr, Value};
use crate::markdown::py_strip;

const STATUSES: &[&str] = &["complete", "needs_input", "failed"];
const KINDS: &[&str] = &["must_decide", "recommended", "assumption"];
const SHAPES: &[&str] = &["one", "several", "yes_no", "open"];
const REFERENCE_KINDS: &[&str] = &["file", "artefact", "ticket", "design", "answer", "url"];
const REQUIRED_KEYS: &[&str] = &["status", "artefact", "outputs", "questions", "reason"];
const OPTIONAL_KEYS: &[&str] = &["commit", "pr_title"];
const CONTINUATION: &str = "    ";
const INDICATORS: &str = "[]{},#&*!|>'\"%@`";

/// What a JSON value in `questions` must be.
#[derive(Clone, Copy)]
enum Spec {
    /// A string, required.
    Str,
    /// A string or null.
    OptStr,
    /// One of these strings, required.
    Enum(&'static [&'static str]),
    /// An array of items, null or absent meaning empty.
    List(&'static Spec),
    /// An object with these fields, required.
    Object(&'static [Field]),
    /// An object with these fields, or null or absent.
    OptObject(&'static [Field]),
}

/// A key and its spec, in serde order.
type Field = (&'static str, Spec);

const OPTION_FIELDS: &[Field] = &[
    ("id", Spec::Str),
    ("label", Spec::Str),
    ("consequence", Spec::Str),
    ("trade_off", Spec::OptStr),
];
const RECOMMENDATION_FIELDS: &[Field] = &[
    ("choice", Spec::List(&Spec::Str)),
    ("answer", Spec::OptStr),
    ("reason", Spec::Str),
];
const REFERENCE_FIELDS: &[Field] = &[
    ("kind", Spec::Enum(REFERENCE_KINDS)),
    ("target", Spec::Str),
    ("lines", Spec::OptStr),
];
const QUESTION_FIELDS: &[Field] = &[
    ("id", Spec::Str),
    ("kind", Spec::Enum(KINDS)),
    ("question", Spec::Str),
    ("why", Spec::Str),
    ("shape", Spec::Enum(SHAPES)),
    ("options", Spec::List(&Spec::Object(OPTION_FIELDS))),
    ("recommendation", Spec::OptObject(RECOMMENDATION_FIELDS)),
    ("references", Spec::List(&Spec::Object(REFERENCE_FIELDS))),
];

/// `KEY_LINE`, `  ([A-Za-z_][A-Za-z0-9_-]*):(?:[ \t]+(.*)|[ \t]*)`: the key and its raw value.
fn key_line(line: &str) -> Option<(&str, &str)> {
    let rest = line.strip_prefix("  ")?;
    let mut chars = rest.char_indices();
    match chars.next() {
        Some((_, c)) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return None,
    }
    let end = rest
        .char_indices()
        .skip(1)
        .find(|&(_, c)| !(c.is_ascii_alphanumeric() || c == '_' || c == '-'))
        .map_or(rest.len(), |(i, _)| i);
    let after = rest[end..].strip_prefix(':')?;
    let value = after.trim_start_matches([' ', '\t']);
    if value.len() == after.len() && !after.is_empty() {
        return None;
    }
    Some((&rest[..end], value))
}

/// Whether `TAIL`, `(?:[ \t]+#.*)?[ \t]*`, matches all of `s`.
fn is_tail(s: &str) -> bool {
    let rest = s.trim_start_matches([' ', '\t']);
    rest.is_empty() || (rest.len() < s.len() && rest.starts_with('#'))
}

/// Where `COMMENT`, `[ \t]#`, first matches in `s`.
fn comment_at(s: &str) -> Option<usize> {
    s.as_bytes()
        .windows(2)
        .position(|w| matches!(w[0], b' ' | b'\t') && w[1] == b'#')
}

fn strip_blanks(s: &str) -> &str {
    s.trim_matches([' ', '\t'])
}

/// Whether `s` is all ASCII digits and not empty.
fn digits(s: &str, radix: u32) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_digit(radix) && c.is_ascii())
}

/// Whether `YAML_NUMBER` matches all of `text`.
fn is_yaml_number(text: &str) -> bool {
    if let Some(octal) = text.strip_prefix("0o")
        && digits(octal, 8)
    {
        return true;
    }
    if let Some(hex) = text.strip_prefix("0x")
        && digits(hex, 16)
    {
        return true;
    }
    if matches!(text, ".nan" | ".NaN" | ".NAN") {
        return true;
    }
    let unsigned = text.strip_prefix(['-', '+']).unwrap_or(text);
    if matches!(unsigned, ".inf" | ".Inf" | ".INF") || digits(unsigned, 10) {
        return true;
    }
    // (?:\.[0-9]+|[0-9]+(?:\.[0-9]*)?)(?:[eE][-+]?[0-9]+)?
    let (mantissa, exponent) = match unsigned.find(['e', 'E']) {
        Some(at) => (&unsigned[..at], Some(&unsigned[at + 1..])),
        None => (unsigned, None),
    };
    let mantissa_ok = match mantissa.split_once('.') {
        Some(("", fraction)) => digits(fraction, 10),
        Some((whole, fraction)) => {
            digits(whole, 10) && (fraction.is_empty() || digits(fraction, 10))
        }
        None => digits(mantissa, 10),
    };
    let exponent_ok = exponent.is_none_or(|e| digits(e.strip_prefix(['-', '+']).unwrap_or(e), 10));
    mantissa_ok && exponent_ok
}

/// Why `text` cannot be a plain scalar here, or `None` when it can.
fn plain_problem(text: &str) -> Option<String> {
    let mut chars = text.chars();
    let first = chars.next()?;
    let second = chars.next();
    if INDICATORS.contains(first)
        || ("-?:".contains(first) && second.is_none_or(|c| c == ' ' || c == '\t'))
    {
        return Some(format!(
            "starts with {first:?}; quote it with double quotes"
        ));
    }
    if text.contains(": ") || text.ends_with(':') || text.contains(":\t") {
        return Some("holds ': '; quote it with double quotes".into());
    }
    None
}

/// A scalar as YAML resolves it.
enum Scalar {
    Null,
    Str(String),
    /// Something other than a string: `a boolean` or `a number`.
    Other(&'static str),
}

/// A plain scalar as YAML resolves it.
fn resolve_plain(text: &str) -> Scalar {
    if matches!(text, "" | "~" | "null" | "Null" | "NULL") {
        Scalar::Null
    } else if matches!(text, "true" | "True" | "TRUE" | "false" | "False" | "FALSE") {
        Scalar::Other("a boolean")
    } else if is_yaml_number(text) {
        Scalar::Other("a number")
    } else {
        Scalar::Str(text.into())
    }
}

/// A scalar value with an optional trailing comment, or why it is not in the subset.
fn read_scalar(raw: &str) -> Result<Scalar, String> {
    let raw = strip_blanks(raw);
    if raw.starts_with('"') {
        let chars: Vec<char> = raw.chars().collect();
        let mut i = 1;
        while i < chars.len() && chars[i] != '"' {
            i += if chars[i] == '\\' { 2 } else { 1 };
        }
        if i >= chars.len() {
            return Err("a double-quoted value is not closed on its line".into());
        }
        let quoted: String = chars[..=i].iter().collect();
        let value = match pyjson::loads(&quoted) {
            Ok(Value::String(value)) => value,
            _ => return Err("a double-quoted value may use JSON escapes only".into()),
        };
        let Some(value) = value.as_str() else {
            return Err("holds an unpaired surrogate".into());
        };
        let after: String = chars[i + 1..].iter().collect();
        if !is_tail(&after) {
            return Err(format!(
                "text after the closing quote: {:?}",
                py_strip(&after)
            ));
        }
        return Ok(Scalar::Str(value.into()));
    }
    if raw.starts_with('\'') {
        return Err("single-quoted values are not supported; use double quotes".into());
    }
    if raw.starts_with(['|', '>']) {
        return Err("block scalars are not supported; use one double-quoted line".into());
    }
    let text = match comment_at(raw) {
        Some(cut) => &raw[..cut],
        None if raw.starts_with('#') => "",
        None => raw,
    };
    let text = text.trim_end_matches([' ', '\t']);
    match plain_problem(text) {
        Some(problem) => Err(problem),
        None => Ok(resolve_plain(text)),
    }
}

/// A one-line flow list of plain scalars, or why it is not.
fn read_outputs(raw: &str) -> Result<Vec<String>, String> {
    let raw = strip_blanks(raw);
    let end = match raw.find(']') {
        Some(end) if raw.starts_with('[') => end,
        _ => {
            return Err(
                "outputs must be a flow list on one line, such as [.mintswarm/job/outputs/plan.md]"
                    .into(),
            );
        }
    };
    if !is_tail(&raw[end + 1..]) {
        return Err(format!(
            "text after outputs' closing ]: {:?}",
            py_strip(&raw[end + 1..])
        ));
    }
    let inner = strip_blanks(&raw[1..end]);
    if inner.is_empty() {
        return Ok(Vec::new());
    }
    let mut paths = Vec::new();
    for item in inner.split(',') {
        let item = strip_blanks(item);
        let problem =
            plain_problem(item).or_else(|| comment_at(item).map(|_| "holds ' #'".to_string()));
        let reason = match (problem, resolve_plain(item)) {
            (None, Scalar::Str(path)) => {
                paths.push(path);
                continue;
            }
            (Some(problem), _) => problem,
            (None, Scalar::Other(what)) => format!("reads as {what}"),
            (None, Scalar::Null) => "is empty or null".into(),
        };
        return Err(format!(
            "outputs item {item:?} is not a plain string: {reason}"
        ));
    }
    Ok(paths)
}

/// Why a line inside the questions array breaks the four-space rule, or `None`.
fn indent_problem(line: &str) -> Option<&'static str> {
    if py_strip(line).is_empty() {
        return Some("a blank line inside the questions array");
    }
    if line.starts_with(CONTINUATION) {
        return None;
    }
    let indent = &line[..line.len() - line.trim_start_matches([' ', '\t']).len()];
    if indent.contains('\t') {
        Some("a continuation line of questions is indented with a tab; use four spaces")
    } else {
        Some("a continuation line of questions is indented by fewer than four spaces")
    }
}

/// The joined texts as characters, and the JSON array decoded from their start.
fn decode_array(texts: &[&str]) -> (Vec<char>, Result<(Value, usize), pyjson::DecodeError>) {
    let text: Vec<char> = texts.join("\n").chars().collect();
    let decoded = pyjson::raw_decode(&text);
    (text, decoded)
}

fn count_newlines(chars: &[char]) -> usize {
    chars.iter().filter(|&&c| c == '\n').count()
}

/// The questions array opened on `block[k]`, whose value is `raw`: the questions when they
/// decode, the index of the next line to read, and the violations.
fn read_questions(
    block: &[&str],
    k: usize,
    raw: &str,
    first: usize,
) -> (Option<Vec<Value>>, usize, Vec<String>) {
    let raw = strip_blanks(raw);
    if !raw.starts_with('[') {
        return (
            None,
            k + 1,
            vec![format!(
                "line {}: questions must be a JSON array opened on its line, such as []",
                first + k
            )],
        );
    }
    let mut end = k + 1;
    while end < block.len() && block[end].starts_with(CONTINUATION) {
        end += 1;
    }
    let texts = |upto: usize| -> Vec<&str> {
        std::iter::once(raw)
            .chain(block[k + 1..upto].iter().copied())
            .collect()
    };
    let mut stop = end;
    let (mut text, mut decoded) = decode_array(&texts(end));
    if decoded.is_err() {
        while stop < block.len() && key_line(block[stop]).is_none() {
            stop += 1;
        }
        if stop > end {
            (text, decoded) = decode_array(&texts(stop));
        }
    }
    let (value, close) = match decoded {
        Ok(decoded) => decoded,
        Err(error) => {
            let rest: String = text[error.pos.min(text.len())..].iter().collect();
            // where decoding stopped decides between the two, as in save-output.py; neither
            // message quotes the decoder
            let violation = if py_strip(&rest).is_empty() {
                format!("line {}: the questions array is not closed by ]", first + k)
            } else {
                format!(
                    "line {}: the questions array is not valid JSON; write it as a JSON array of \
                     question objects",
                    first + k
                )
            };
            return (None, stop, vec![violation]);
        }
    };
    let mut violations = Vec::new();
    let last = count_newlines(&text[..close - 1]);
    for j in 1..=last {
        if let Some(problem) = indent_problem(block[k + j]) {
            violations.push(format!("line {}: {problem}", first + k + j));
        }
    }
    let after: String = text[close..].iter().collect();
    let mut tail = after.split('\n');
    let head = tail.next().unwrap_or("");
    if !is_tail(head) {
        violations.push(format!(
            "line {}: text after the questions array's closing ]: {:?}",
            first + k + last,
            py_strip(head)
        ));
    }
    for (j, line) in tail.enumerate() {
        let stripped = py_strip(line);
        if !stripped.is_empty() && !stripped.starts_with('#') {
            violations.push(format!(
                "line {}: text after the questions array's closing ]: {stripped:?}",
                first + k + last + 1 + j
            ));
        }
    }
    // text starting with `[` decodes to an array, or not at all
    let items = match value {
        Value::Array(items) => Some(items),
        _ => None,
    };
    (items, stop, violations)
}

/// Checks one JSON value against its spec; `value` is `None` when the key is absent. Returns the
/// value with defaults applied when it is sound.
fn check_value(
    value: Option<&Value>,
    spec: Spec,
    label: &str,
    path: &str,
    out: &mut Vec<String>,
) -> Option<Json> {
    let present = value.is_some();
    let null = matches!(value, None | Some(Value::Null));
    let type_name = || value.map_or("null", Value::type_name);
    match spec {
        Spec::Str => {
            match value {
                None => out.push(format!("{label}: {path} is missing")),
                Some(Value::String(s)) => match s.as_str() {
                    Some(s) => return Some(Json::String(s.into())),
                    None => out.push(format!("{label}: {path} holds an unpaired surrogate")),
                },
                Some(other) => out.push(format!(
                    "{label}: {path} must be a string, not {}",
                    other.type_name()
                )),
            }
            None
        }
        Spec::OptStr => {
            match value {
                Some(Value::String(s)) => match s.as_str() {
                    Some(s) => return Some(Json::String(s.into())),
                    None => out.push(format!("{label}: {path} holds an unpaired surrogate")),
                },
                _ if null => return Some(Json::Null),
                _ => out.push(format!(
                    "{label}: {path} must be a string or null, not {}",
                    type_name()
                )),
            }
            None
        }
        Spec::Enum(values) => {
            match value {
                None => out.push(format!("{label}: {path} is missing")),
                Some(Value::String(s)) if values.iter().any(|v| s.is(v)) => {
                    return Some(Json::String(s.lossy().into()));
                }
                Some(other) => {
                    let found = match other {
                        Value::String(s) => s.debug(),
                        _ => other.type_name().into(),
                    };
                    out.push(format!(
                        "{label}: {path} must be one of {}, not {found}",
                        values.join(", ")
                    ));
                }
            }
            None
        }
        Spec::List(item_spec) => {
            if null {
                return Some(Json::Array(Vec::new()));
            }
            let Some(Value::Array(items)) = value else {
                out.push(format!(
                    "{label}: {path} must be an array or null, not {}",
                    type_name()
                ));
                return None;
            };
            let mut sound = true;
            let mut checked = Vec::new();
            for (i, item) in items.iter().enumerate() {
                match check_value(Some(item), *item_spec, label, &format!("{path}[{i}]"), out) {
                    Some(item) => checked.push(item),
                    None => sound = false,
                }
            }
            sound.then_some(Json::Array(checked))
        }
        Spec::Object(fields) | Spec::OptObject(fields) => {
            let optional = matches!(spec, Spec::OptObject(_));
            if null && (optional || !present) {
                if optional {
                    return Some(Json::Null);
                }
                out.push(format!("{label}: {path} is missing"));
                return None;
            }
            let Some(Value::Object(object)) = value else {
                out.push(format!(
                    "{label}: {path} must be an object, not {}",
                    type_name()
                ));
                return None;
            };
            check_fields(object, fields, label, path, out)
        }
    }
}

/// Checks an object's keys; returns the object in serde order, defaults applied, when sound.
fn check_fields(
    object: &Object,
    fields: &[Field],
    label: &str,
    path: &str,
    out: &mut Vec<String>,
) -> Option<Json> {
    let mut sound = true;
    let place = if path.is_empty() {
        String::new()
    } else {
        format!("{path}: ")
    };
    for key in &object.duplicates {
        out.push(format!("{label}: {place}duplicate key {}", key.debug()));
        sound = false;
    }
    for (key, _) in &object.entries {
        if !fields.iter().any(|(name, _)| key.is(name)) {
            out.push(format!("{label}: {place}unknown key {}", key.debug()));
            sound = false;
        }
    }
    let mut checked = Vec::new();
    for &(name, spec) in fields {
        let at = if path.is_empty() {
            name.to_string()
        } else {
            format!("{path}.{name}")
        };
        match check_value(object.get(name), spec, label, &at, out) {
            Some(value) => checked.push((name.to_string(), value)),
            None => sound = false,
        }
    }
    sound.then_some(Json::Object(checked))
}

/// The ids that occur more than once, each once, in order of first use.
fn repeated<'a>(ids: impl IntoIterator<Item = &'a str>) -> Vec<&'a str> {
    let mut seen = Vec::new();
    let mut out = Vec::new();
    for id in ids {
        if !seen.contains(&id) {
            seen.push(id);
        } else if !out.contains(&id) {
            out.push(id);
        }
    }
    out
}

fn status_of(text: &str) -> Option<OutcomeStatus> {
    match text {
        "complete" => Some(OutcomeStatus::Complete),
        "needs_input" => Some(OutcomeStatus::NeedsInput),
        "failed" => Some(OutcomeStatus::Failed),
        _ => None,
    }
}

/// The structural checks, then `Question::violations` on each sound question and, when all are
/// sound, the id rule and, for a valid status, the status rule of `StageOutcome::check`. Returns
/// the violations and the questions when all are sound.
fn check_questions(
    questions: &[Value],
    status: Option<OutcomeStatus>,
) -> (Vec<String>, Option<Vec<Question>>) {
    let mut out = Vec::new();
    let mut typed = Vec::new();
    let mut all_sound = true;
    for (i, q) in questions.iter().enumerate() {
        let Value::Object(object) = q else {
            out.push(format!(
                "questions[{i}] must be a question object, not {}",
                q.type_name()
            ));
            all_sound = false;
            continue;
        };
        let label = match object
            .get("id")
            .and_then(Value::as_str)
            .and_then(PyStr::as_str)
        {
            Some(id) if !id.trim().is_empty() => id.to_string(),
            _ => format!("questions[{i}]"),
        };
        let Some(normalised) = check_fields(object, QUESTION_FIELDS, &label, "", &mut out) else {
            all_sound = false;
            continue;
        };
        let question =
            serde_json::to_value(&normalised).and_then(serde_json::from_value::<Question>);
        match question {
            Ok(question) => {
                out.extend(question.violations().iter().map(ToString::to_string));
                typed.push(question);
            }
            Err(error) => {
                out.push(format!("{label}: {error}"));
                all_sound = false;
            }
        }
    }
    if !all_sound {
        return (out, None);
    }
    match status {
        Some(status) => {
            let outcome = StageOutcome {
                status,
                artefact: String::new(),
                outputs: Vec::new(),
                questions: typed,
                reason: String::new(),
                commit: None,
                pr_title: None,
            };
            if let Err(found) = outcome.check() {
                // the per-question part comes first, and is already listed above
                let own: usize = outcome.questions.iter().map(|q| q.violations().len()).sum();
                out.extend(found.iter().skip(own).map(ToString::to_string));
            }
            (out, Some(outcome.questions))
        }
        None => {
            for id in repeated(typed.iter().map(|q| q.id.as_str())) {
                out.push(format!("{id}: the id is not unique"));
            }
            (out, Some(typed))
        }
    }
}

/// A top-level value as the parser holds it; `None` stands for Python's `None`.
enum Slot {
    None,
    Str(String),
    Paths(Vec<String>),
    Questions(Vec<Value>),
}

/// Parses and checks the result block's lines, `block[0]` being `msw-result:` on reply line
/// `first`. Returns the outcome, or every violation in `save-output.py`'s order.
pub(crate) fn parse_result(block: &[&str], first: usize) -> Result<StageOutcome, Vec<String>> {
    let mut violations = Vec::new();
    let mut values: Vec<(&'static str, Slot)> = Vec::new();
    if !is_tail(&block[0]["msw-result:".len()..]) {
        violations.push(format!(
            "line {first}: text after 'msw-result:'; its keys go on the lines below"
        ));
    }
    let mut k = 1;
    while k < block.len() {
        let line = block[k];
        let here = first + k;
        if py_strip(line).is_empty() || line.trim_start_matches([' ', '\t']).starts_with('#') {
            k += 1;
            continue;
        }
        let Some((key, raw)) = key_line(line) else {
            violations.push(format!(
                "line {here}: expected a key indented by two spaces, found {:?}",
                py_strip(line)
            ));
            k += 1;
            continue;
        };
        k += 1;
        let Some(&key) = REQUIRED_KEYS
            .iter()
            .chain(OPTIONAL_KEYS)
            .find(|&&n| n == key)
        else {
            violations.push(format!("line {here}: unknown key {key:?}"));
            while k < block.len() && block[k].starts_with("   ") {
                k += 1;
            }
            continue;
        };
        if values.iter().any(|(seen, _)| *seen == key) {
            violations.push(format!("line {here}: duplicate key {key:?}"));
        }
        let value = match key {
            "questions" => {
                let (questions, next, found) = read_questions(block, k - 1, raw, first);
                k = next;
                violations.extend(found);
                questions.map_or(Slot::None, Slot::Questions)
            }
            "outputs" => match read_outputs(raw) {
                Ok(paths) => Slot::Paths(paths),
                Err(problem) => {
                    violations.push(format!("line {here}: {problem}"));
                    Slot::None
                }
            },
            _ => {
                let value = match read_scalar(raw) {
                    Err(problem) => {
                        violations.push(format!("line {here}: {key}: {problem}"));
                        Slot::None
                    }
                    Ok(Scalar::Str(s)) => Slot::Str(s),
                    Ok(Scalar::Null) if OPTIONAL_KEYS.contains(&key) => Slot::None,
                    Ok(other) => {
                        let what = match other {
                            Scalar::Other(what) => format!("reads as {what}"),
                            _ => "empty or null".into(),
                        };
                        violations.push(format!(
                            "line {here}: {key} must be a string but {what}; quote it, for example \"\""
                        ));
                        Slot::None
                    }
                };
                if let (Slot::Str(status), "status") = (&value, key)
                    && !STATUSES.contains(&status.as_str())
                {
                    violations.push(format!(
                        "line {here}: status must be one of {}, not {status:?}",
                        STATUSES.join(", ")
                    ));
                }
                value
            }
        };
        if !values.iter().any(|(seen, _)| *seen == key) {
            values.push((key, value));
        }
    }
    for key in REQUIRED_KEYS {
        if !values.iter().any(|(seen, _)| seen == key) {
            violations.push(format!("missing key {key:?}"));
        }
    }
    let get = |key: &str| values.iter().find(|(seen, _)| *seen == key).map(|(_, v)| v);
    let text = |key: &str| match get(key) {
        Some(Slot::Str(s)) => Some(s.clone()),
        _ => None,
    };
    let status = text("status").as_deref().and_then(status_of);
    let mut questions = None;
    if let Some(Slot::Questions(items)) = get("questions") {
        let (found, typed) = check_questions(items, status);
        violations.extend(found);
        questions = typed;
    }
    if !violations.is_empty() {
        return Err(violations);
    }
    let paths = match get("outputs") {
        Some(Slot::Paths(paths)) => Some(paths.clone()),
        _ => None,
    };
    match (status, text("artefact"), paths, questions, text("reason")) {
        (Some(status), Some(artefact), Some(outputs), Some(questions), Some(reason)) => {
            Ok(StageOutcome {
                status,
                artefact,
                outputs,
                questions,
                reason,
                commit: text("commit"),
                pr_title: text("pr_title"),
            })
        }
        _ => Err(vec![
            "the result block lacks a required value without a violation".into(),
        ]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_lines() {
        assert_eq!(key_line("  status: complete"), Some(("status", "complete")));
        assert_eq!(key_line("  status:"), Some(("status", "")));
        assert_eq!(key_line("  status:   "), Some(("status", "")));
        assert_eq!(key_line("  status:\t x "), Some(("status", "x ")));
        assert_eq!(key_line("  pr-title_2: x"), Some(("pr-title_2", "x")));
        assert_eq!(key_line("  status:x"), None);
        assert_eq!(key_line("   status: x"), None);
        assert_eq!(key_line(" status: x"), None);
        assert_eq!(key_line("  9a: x"), None);
    }

    #[test]
    fn tails_and_comments() {
        assert!(is_tail(""));
        assert!(is_tail("  \t"));
        assert!(is_tail("  # note"));
        assert!(!is_tail("# note"));
        assert!(!is_tail(" x"));
        assert_eq!(comment_at("a #b"), Some(1));
        assert_eq!(comment_at("a#b"), None);
    }

    #[test]
    fn yaml_numbers() {
        for yes in [
            "1", "-1", "+12", "0o17", "0x1F", "1.5", "1.", ".5", "-.5", "1e5", "1.5E-3", ".inf",
            "-.Inf", ".NaN",
        ] {
            assert!(is_yaml_number(yes), "{yes}");
        }
        for no in [
            "", "-", "1.2.3", "e5", "1e", "0o8", "0x", "-.nan", "1_000", "v1", ".", "1e+",
        ] {
            assert!(!is_yaml_number(no), "{no}");
        }
    }

    #[test]
    fn plain_scalars() {
        assert!(matches!(read_scalar("complete  # note"), Ok(Scalar::Str(s)) if s == "complete"));
        assert!(matches!(read_scalar("# only a comment"), Ok(Scalar::Null)));
        assert!(matches!(read_scalar("~"), Ok(Scalar::Null)));
        assert!(matches!(read_scalar("yes"), Ok(Scalar::Str(_))));
        assert!(matches!(
            read_scalar("True"),
            Ok(Scalar::Other("a boolean"))
        ));
        assert_eq!(
            read_scalar("- x").err().as_deref(),
            Some("starts with '-'; quote it with double quotes")
        );
        assert_eq!(
            read_scalar("a: b").err().as_deref(),
            Some("holds ': '; quote it with double quotes")
        );
        assert_eq!(
            read_scalar("\"a\" x").err().as_deref(),
            Some("text after the closing quote: \"x\"")
        );
    }
}
