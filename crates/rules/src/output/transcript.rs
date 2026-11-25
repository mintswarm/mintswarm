//! The reply in a Claude Code subagent transcript, selected as `save-output.py` does without
//! `--text`.
//!
//! The transcript is split into lines with universal newlines (`\r\n`, `\r` and `\n`), as Python
//! reads a text file, and each line is decoded as `json.loads` does; lines that do not decode are
//! skipped. Only records of `type: assistant` whose `message.role` is `assistant` count. Records
//! that share a non-empty `message.id` are one message, in order of first appearance; a record
//! without one is a message of its own. A message's text is its `text` parts and the `message`
//! input of its `SubagentHandback` tool calls, joined by newlines; messages whose text is blank
//! are not counted.
//!
//! Divergences, both where Python would fail rather than select: a lone surrogate in extracted
//! text becomes U+FFFD (Python fails to write the document), and a `text` or `message` value that
//! is not a string counts as empty (Python fails to join it).

use super::pyjson::{self, PyStr, Value};
use super::{Capture, Fault, extract, opener};
use crate::markdown::py_strip;

/// The assistant message a reply was taken from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Selected {
    /// Its 0-based index among the transcript's assistant messages.
    pub index: usize,
    /// How many assistant messages the transcript holds.
    pub count: usize,
    /// Its `message.id`, when it has one.
    pub id: Option<String>,
}

impl Selected {
    /// The message as `save-output.py` names it: `assistant message K of M (<id or no id>)`.
    pub fn describe(&self) -> String {
        format!(
            "assistant message {} of {} ({})",
            self.index + 1,
            self.count,
            self.id.as_deref().unwrap_or("no id")
        )
    }
}

/// What [`capture_transcript`] found: the message it checked, if any, and the outcome.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TranscriptCapture {
    /// The selected message; `None` when the transcript has no assistant text.
    pub message: Option<Selected>,
    /// The capture of that message, or its fault.
    pub result: Result<Capture, Fault>,
}

/// Whether `value` is the string `s`.
fn is(value: Option<&Value>, s: &str) -> bool {
    value.and_then(Value::as_str).is_some_and(|v| v.is(s))
}

/// The text parts of a message's `content`.
fn message_text(content: Option<&Value>) -> Vec<String> {
    let text_of = |value: Option<&Value>| {
        value
            .and_then(Value::as_str)
            .map(|s| s.lossy().to_string())
            .unwrap_or_default()
    };
    let items = match content {
        Some(Value::String(s)) => return vec![s.lossy().to_string()],
        Some(Value::Array(items)) => items,
        _ => return Vec::new(),
    };
    let mut parts = Vec::new();
    for item in items {
        let Value::Object(c) = item else { continue };
        if is(c.get("type"), "text") {
            parts.push(text_of(c.get("text")));
        } else if is(c.get("type"), "tool_use") && is(c.get("name"), "SubagentHandback") {
            let message = match c.get("input") {
                Some(Value::Object(input)) => input.get("message"),
                _ => None,
            };
            parts.push(text_of(message));
        }
    }
    parts
}

/// The transcript's assistant messages as `(id, text)`, in order.
fn assistant_messages(transcript: &str) -> Vec<(Option<String>, String)> {
    let mut messages: Vec<(Option<PyStr>, Vec<String>)> = Vec::new();
    let normalised = transcript.replace("\r\n", "\n").replace('\r', "\n");
    for line in normalised.split('\n') {
        let Ok(Value::Object(rec)) = pyjson::loads(line) else {
            continue;
        };
        if !is(rec.get("type"), "assistant") {
            continue;
        }
        let Some(Value::Object(msg)) = rec.get("message") else {
            continue;
        };
        if !is(msg.get("role"), "assistant") {
            continue;
        }
        let parts: Vec<String> = message_text(msg.get("content"))
            .into_iter()
            .filter(|p| !p.is_empty())
            .collect();
        match msg.get("id").and_then(Value::as_str) {
            Some(id) if !id.lossy().is_empty() => {
                match messages
                    .iter_mut()
                    .find(|(seen, _)| seen.as_ref() == Some(id))
                {
                    Some((_, joined)) => joined.extend(parts),
                    None => messages.push((Some(id.clone()), parts)),
                }
            }
            _ => messages.push((None, parts)),
        }
    }
    messages
        .into_iter()
        .map(|(id, parts)| (id.map(|id| id.lossy().to_string()), parts.join("\n")))
        .filter(|(_, text)| !py_strip(text).is_empty())
        .collect()
}

fn has_opener(text: &str) -> bool {
    text.split('\n')
        .any(|line| opener(line.strip_suffix('\r').unwrap_or(line)).is_some())
}

/// Whether `text` has no document opener but starts with a `# ` title.
fn unfenced_document(text: &str) -> bool {
    if has_opener(text) {
        return false;
    }
    text.split('\n')
        .find(|line| !py_strip(line).is_empty())
        .is_some_and(|line| line.starts_with("# "))
}

/// The index of the reply: the last message if it is an unfenced document, else the last message
/// with a document opener, else the last message.
fn pick_reply(messages: &[(Option<String>, String)]) -> usize {
    let last = messages.len() - 1;
    if unfenced_document(&messages[last].1) {
        return last;
    }
    (0..messages.len())
        .rev()
        .find(|&k| has_opener(&messages[k].1))
        .unwrap_or(last)
}

/// Selects the reply in a Claude Code transcript and captures it, as `save-output.py` does
/// without `--text`.
pub fn capture_transcript(transcript: &str) -> TranscriptCapture {
    let messages = assistant_messages(transcript);
    if messages.is_empty() {
        return TranscriptCapture {
            message: None,
            result: Err(Fault::new("no assistant text in the transcript", "rule 1")),
        };
    }
    let index = pick_reply(&messages);
    let (id, text) = &messages[index];
    let result = if unfenced_document(text) {
        Err(Fault::new(
            "no document block: the final message starts with a '# ' title but is not in a block \
             opened by a line of 4 or more backticks followed by 'markdown'; no earlier block is \
             used",
            "rule 1",
        ))
    } else {
        extract(text)
    };
    TranscriptCapture {
        message: Some(Selected {
            index,
            count: messages.len(),
            id: id.clone(),
        }),
        result,
    }
}
