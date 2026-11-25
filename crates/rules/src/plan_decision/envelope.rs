//! The judgement layer: the assessment's envelope as `envelope_problems`, `envelope_fields` and
//! `judgement_layer` in `.mintswarm/dogfood/plan-approval/decision.py` read it.

use mintswarm_domain::records::{Band, Doubt, ExpectedJudge, Json, JudgementLayer, LayerStatus};

use super::Input;
use crate::binding::digest;
use crate::json::{self, ReadError, ReadErrorKind};
use crate::markdown::py_strip;
use crate::render::{AUTHORITY_TRIGGERS, HARD_TRIGGERS, inert};

/// The assess stage's named bands: identifier, band, display name, lowest and highest
/// confidence, in `decision.py`'s order.
pub const BANDS: [(&str, Band, &str, u32, u32); 5] = [
    ("blocked", Band::Blocked, "Blocked", 0, 0),
    ("uncertain", Band::Uncertain, "Uncertain", 1, 49),
    ("needs-review", Band::NeedsReview, "Needs review", 50, 79),
    ("likely-safe", Band::LikelySafe, "Likely safe", 80, 94),
    ("safe", Band::Safe, "Safe", 95, 100),
];

/// The envelope's keys, in order.
const ENVELOPE_KEYS: [&str; 5] = ["confidence", "band", "hard_triggers", "reason", "doubts"];
/// A doubt's keys, in order.
const DOUBT_KEYS: [&str; 3] = ["statement", "consequence", "resolution"];
/// The most doubts an envelope may hold.
const MAX_DOUBTS: usize = 10;
const FENCE_OPEN: &str = "```json";
const FENCE_CLOSE: &str = "```";
/// The noun the problems name the document by.
const NOUN: &str = "assessment";

/// The band's display name, such as `Likely safe`.
pub fn band_name(band: Band) -> &'static str {
    BANDS
        .iter()
        .find(|entry| entry.1 == band)
        .map_or("", |entry| entry.2)
}

/// A usable envelope's content.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Envelope {
    pub confidence: u32,
    pub band: Band,
    pub hard_triggers: Vec<String>,
    pub reason: String,
    pub doubts: Vec<Doubt>,
}

/// Whether `value` is a string that is not empty once whitespace, control and format characters
/// are removed: `is_text`.
fn is_text(value: &Json) -> bool {
    matches!(value, Json::String(s) if !inert(s).is_empty())
}

fn keys(entries: &[(String, Json)]) -> Vec<&str> {
    entries.iter().map(|(key, _)| key.as_str()).collect()
}

fn text(value: &Json) -> String {
    match value {
        Json::String(s) => s.clone(),
        _ => String::new(),
    }
}

/// Problems with the envelope's parsed content, as `envelope_fields` gives the first of them.
fn envelope_fields(envelope: &Json) -> Result<Envelope, String> {
    let entries = match envelope {
        Json::Object(entries) if keys(entries) == ENVELOPE_KEYS => entries,
        _ => {
            return Err(format!(
                "the {NOUN}'s envelope is not an object with exactly {}, in order",
                ENVELOPE_KEYS.join(", ")
            ));
        }
    };
    let field = |index: usize| &entries[index].1;
    let confidence = match field(0) {
        Json::Int(i) if (0..=100).contains(i) => u32::try_from(*i).unwrap_or(0),
        _ => {
            return Err(format!(
                "the {NOUN}'s confidence is not an integer from 0 to 100"
            ));
        }
    };
    let band = match field(1) {
        Json::String(name) => BANDS.iter().find(|entry| entry.0 == name),
        _ => None,
    };
    let Some(&(band_id, band, _, low, high)) = band else {
        let names: Vec<&str> = BANDS.iter().map(|entry| entry.0).collect();
        return Err(format!(
            "the {NOUN}'s band is not one of {}",
            names.join(", ")
        ));
    };
    if !(low..=high).contains(&confidence) {
        return Err(format!(
            "the {NOUN}'s confidence is outside its band's range"
        ));
    }
    let hard: Vec<String> = match field(2) {
        Json::Array(items)
            if items
                .iter()
                .all(|t| matches!(t, Json::String(s) if HARD_TRIGGERS.contains(&s.as_str()))) =>
        {
            items.iter().map(text).collect()
        }
        _ => {
            return Err(format!(
                "the {NOUN}'s hard_triggers is not a list of known hard trigger identifiers"
            ));
        }
    };
    if hard
        .iter()
        .enumerate()
        .any(|(index, t)| hard[..index].contains(t))
    {
        return Err(format!("the {NOUN}'s hard_triggers repeats a trigger"));
    }
    let unusable = hard
        .iter()
        .any(|t| !AUTHORITY_TRIGGERS.contains(&t.as_str()));
    if unusable != (band_id == "blocked") {
        return Err(format!(
            "the {NOUN}'s band must be blocked exactly when hard_triggers lists an unusable trigger"
        ));
    }
    if !is_text(field(3)) {
        return Err(format!("the {NOUN}'s envelope gives no reason"));
    }
    let doubts = match field(4) {
        Json::Array(items) if items.len() <= MAX_DOUBTS => items,
        _ => {
            return Err(format!(
                "the {NOUN}'s doubts is not a list of at most {MAX_DOUBTS}"
            ));
        }
    };
    let mut parsed = Vec::with_capacity(doubts.len());
    for doubt in doubts {
        match doubt {
            Json::Object(entries)
                if keys(entries) == DOUBT_KEYS && entries.iter().all(|(_, v)| is_text(v)) =>
            {
                parsed.push(Doubt {
                    statement: text(&entries[0].1),
                    consequence: text(&entries[1].1),
                    resolution: text(&entries[2].1),
                });
            }
            _ => {
                return Err(format!(
                    "a doubt is not an object with exactly non-empty {}, in order",
                    DOUBT_KEYS.join(", ")
                ));
            }
        }
    }
    if band_id == "likely-safe" && parsed.is_empty() {
        return Err(format!("the {NOUN}'s band is likely-safe without a doubt"));
    }
    if band_id == "safe" && !parsed.is_empty() {
        return Err(format!("the {NOUN}'s band is safe with a doubt"));
    }
    Ok(Envelope {
        confidence,
        band,
        hard_triggers: hard,
        reason: text(field(3)),
        doubts: parsed,
    })
}

/// The confidence and reason of a summary line `<confidence> confidence: <reason>`, as
/// `decision.py`'s `SUMMARY` matches it: 0, or 1 to 3 digits without a leading zero.
fn summary(line: &str) -> Option<(u32, &str)> {
    let digits = line.len() - line.trim_start_matches(|c: char| c.is_ascii_digit()).len();
    let (number, rest) = line.split_at(digits);
    let reason = rest.strip_prefix(" confidence: ")?;
    let valid = number == "0" || (1..=3).contains(&number.len()) && !number.starts_with('0');
    if !valid {
        return None;
    }
    Some((number.parse().ok()?, reason))
}

/// The envelope of an assessment's `text`, or the problem that makes it unusable, as
/// `envelope_problems` gives it.
pub(crate) fn envelope_problems(text: &str) -> Result<Envelope, String> {
    let lines: Vec<&str> = text
        .split('\n')
        .map(|line| line.trim_end_matches(['\r', ' ', '\t']))
        .collect();
    if lines.iter().any(|line| line.starts_with("Risk:")) {
        return Err(format!("the {NOUN} has a line starting with 'Risk:'"));
    }
    let content: Vec<&str> = lines
        .iter()
        .copied()
        .filter(|line| !py_strip(line).is_empty())
        .collect();
    if !content.first().is_some_and(|line| line.starts_with("# ")) {
        return Err(format!("the {NOUN} does not start with a '# ' title"));
    }
    let Some((stated, reason)) = content.get(1).and_then(|line| summary(line)) else {
        return Err(format!(
            "the line after the {NOUN}'s title is not '<confidence> confidence: <reason>'"
        ));
    };
    if inert(reason).is_empty() {
        return Err(format!("the {NOUN}'s summary gives no reason"));
    }
    let opens: Vec<usize> = (0..lines.len())
        .filter(|&i| lines[i] == FENCE_OPEN)
        .collect();
    let start = match opens.as_slice() {
        [] => return Err(format!("the {NOUN} has no envelope")),
        [open] => open + 1,
        _ => return Err(format!("the {NOUN} has more than one envelope")),
    };
    let Some(close) = (start..lines.len()).find(|&i| lines[i] == FENCE_CLOSE) else {
        return Err(format!("the {NOUN}'s envelope is not closed"));
    };
    let (envelope, surrogate) = match read_keeping_surrogates(&lines[start..close].join("\n")) {
        Ok(read) => read,
        Err(_) => return Err(format!("the {NOUN}'s envelope is not strict JSON")),
    };
    if surrogate {
        return Err(format!(
            "the {NOUN}'s envelope holds a string with a lone surrogate"
        ));
    }
    let envelope = envelope_fields(&envelope)?;
    if stated != envelope.confidence {
        return Err(format!(
            "the {NOUN}'s summary confidence differs from its envelope"
        ));
    }
    Ok(envelope)
}

/// `text` read as strict JSON as Python reads it, which keeps a `\u` escape of a lone surrogate:
/// the value, with each lone surrogate standing as a code point of the plane-15 private use area
/// (U+F0000 plus its offset from U+D800), and whether it held one.
///
/// Deliberate divergence: a key that holds such a code point as it is and another that holds the
/// lone surrogate it stands for read as duplicates here, and not in Python.
pub(crate) fn read_keeping_surrogates(text: &str) -> Result<(Json, bool), ReadError> {
    match json::read(text) {
        Ok(value) => Ok((value, false)),
        Err(error) if error.kind == ReadErrorKind::LoneSurrogate => {
            json::read(&replace_lone_surrogates(text)).map(|value| (value, true))
        }
        Err(error) => Err(error),
    }
}

/// `text` with every `\u` escape of a lone surrogate inside a JSON string replaced by the code
/// point that stands for it.
fn replace_lone_surrogates(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let unit = |at: usize| -> Option<u32> {
        let digits = chars.get(at..at + 4)?;
        if !digits.iter().all(char::is_ascii_hexdigit) {
            return None;
        }
        u32::from_str_radix(&digits.iter().collect::<String>(), 16).ok()
    };
    let escape = |at: usize| -> Option<u32> {
        (chars.get(at) == Some(&'\\') && chars.get(at + 1) == Some(&'u'))
            .then(|| unit(at + 2))
            .flatten()
    };
    let mut out = String::with_capacity(text.len());
    let mut in_string = false;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if !in_string {
            in_string = c == '"';
            out.push(c);
            i += 1;
            continue;
        }
        if c == '"' {
            in_string = false;
            out.push(c);
            i += 1;
            continue;
        }
        if c != '\\' {
            out.push(c);
            i += 1;
            continue;
        }
        let taken = match escape(i) {
            Some(0xd800..=0xdbff)
                if escape(i + 6).is_some_and(|low| (0xdc00..=0xdfff).contains(&low)) =>
            {
                12
            }
            Some(lone @ 0xd800..=0xdfff) => {
                out.extend(char::from_u32(0xf0000 + lone - 0xd800));
                i += 6;
                continue;
            }
            Some(_) => 6,
            None => 2,
        };
        let end = (i + taken).min(chars.len());
        out.extend(&chars[i..end]);
        i = end;
    }
    out
}

/// The judgement layer for the assessment `input` at `path`, published as `artifact` and judged
/// by `harness` with `model`, against the `expected` judge: `judgement_layer`.
pub(crate) fn judgement_layer(
    input: Input<'_>,
    path: &str,
    artifact: &str,
    harness: &str,
    model: &str,
    expected: &ExpectedJudge,
) -> JudgementLayer {
    let mut layer = JudgementLayer {
        status: LayerStatus::Unusable,
        problems: Vec::new(),
        stage: "assess".to_owned(),
        path: path.to_owned(),
        sha256: None,
        artifact: artifact.to_owned(),
        harness: harness.to_owned(),
        model: model.to_owned(),
        expected: expected.clone(),
        as_configured: expected.harness.as_deref() == Some(harness)
            && expected.model.as_deref() == Some(model),
        confidence: None,
        band: None,
        hard_triggers: Vec::new(),
        doubts: Vec::new(),
        reasons: Vec::new(),
    };
    let bytes = match input {
        Input::Missing => {
            layer.problems = vec!["the assessment is missing".to_owned()];
            return layer;
        }
        Input::Unreadable => {
            layer.problems = vec!["the assessment cannot be read".to_owned()];
            return layer;
        }
        Input::Bytes(bytes) => bytes,
    };
    layer.sha256 = Some(digest(bytes));
    let Ok(text) = std::str::from_utf8(bytes) else {
        layer.problems = vec!["the assessment is not UTF-8".to_owned()];
        return layer;
    };
    match envelope_problems(text) {
        Err(problem) => layer.problems = vec![problem],
        Ok(envelope) => {
            layer.status = LayerStatus::Usable;
            layer.confidence = Some(envelope.confidence);
            layer.band = Some(envelope.band);
            layer.hard_triggers = envelope.hard_triggers;
            layer.doubts = envelope.doubts;
            layer.reasons = vec![envelope.reason];
        }
    }
    layer
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summary_takes_python_numbers() {
        assert_eq!(summary("95 confidence: x"), Some((95, "x")));
        assert_eq!(summary("0 confidence: "), Some((0, "")));
        assert_eq!(summary("095 confidence: x"), None);
        assert_eq!(summary("1000 confidence: x"), None);
        assert_eq!(summary("low risk: x"), None);
    }

    #[test]
    fn lone_surrogates_are_kept_and_reported() {
        let (value, lone) = read_keeping_surrogates(r#"{"a": "x \ud800 y"}"#).expect("read");
        assert!(lone);
        assert!(matches!(value, Json::Object(_)));
        let (_, lone) = read_keeping_surrogates(r#"{"a": "😀"}"#).expect("read");
        assert!(!lone);
        assert!(read_keeping_surrogates(r#"{"a": "\ud800", "a": 1}"#).is_err());
        assert!(read_keeping_surrogates(r#"{"a": "\\ud800"}"#).is_ok_and(|(_, lone)| !lone));
    }
}
