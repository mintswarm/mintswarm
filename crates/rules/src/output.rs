//! A stage's output document and `msw-result` block, captured from its final reply.
//!
//! [`capture`] is an exact port of `.mintswarm/dogfood/save-output.py --text` without its I/O: it
//! applies rules 1–6 of "Returning the output" in `.mintswarm/stages.md`, reads the result block
//! in the strict subset of YAML that "Result block" and "Questions" describe, and runs the
//! question checks. It gives either the document exactly as `save-output.py` saves it, with the
//! result record it writes beside it, or the one [`Fault`] it reports, whose
//! [`Fault::explain`] is the text it prints after `save-output: `. [`capture_transcript`] first
//! selects the reply from a Claude Code transcript, as `save-output.py` does without `--text`.
//!
//! Lines are split at LF only; a trailing CR is ignored when matching and kept in the saved
//! document. Rule texts are quoted verbatim from `.mintswarm/stages.md`.
//!
//! [`check_expected`] adds what `save-output.py` cannot check, because it does not know the
//! stage: the `artefact`, the `outputs` and a `failed` outcome's `reason`. The runner calls it
//! after [`capture`].
//!
//! Writing the files, the exit codes and the command line belong to the caller.

mod block;
mod pyjson;
mod repr;
mod transcript;

use mintswarm_domain::work::{OutcomeStatus, StageOutcome};

use crate::json::{self, Style};
use crate::markdown::py_strip;

pub use repr::py_repr;
pub use transcript::{Selected, TranscriptCapture, capture_transcript};

/// The file every rule is quoted from.
pub const STAGES: &str = ".mintswarm/stages.md";

/// Rules 1–6 of "Returning the output" and the start of "Result block" in
/// `.mintswarm/stages.md`, verbatim, by the name a [`Fault`] gives them.
pub const RULES: &[(&str, &str)] = &[
    (
        "rule 1",
        "The opening line is N backticks immediately followed by `markdown`, at the start of the \
         line with no indentation; N is at least 4. Trailing whitespace is allowed, nothing else.",
    ),
    (
        "rule 2",
        "The closing line is exactly N backticks at the start of the line, optionally followed by \
         whitespace.",
    ),
    (
        "rule 3",
        "N is greater than the longest run of backticks that starts any line of the document, \
         ignoring leading spaces. That is 4 for most documents, and 5 when the document contains \
         a four-backtick block, for example one quoting the decompose or review template.",
    ),
    (
        "rule 4",
        "The runner saves the lines strictly between the opening and closing lines, unchanged, \
         with a final newline. The first of them is the document's `# ` title.",
    ),
    (
        "rule 5",
        "The reply contains exactly one such block before the result block. Lines inside it are \
         part of the document, never the start of another block.",
    ),
    (
        "rule 6",
        "After the closing line come only blank lines, then exactly one result block (below). \
         Text after the result block is ignored. A few lines of text may come before the \
         document block.",
    ),
    (
        "Result block",
        "End your final reply with exactly one block of this form; anything after it is \
         ignored:\n\n```yaml\nmsw-result:",
    ),
];

/// The last sentence of "Questions" in `.mintswarm/stages.md`, verbatim: the rule quoted when the
/// result block's content breaks the subset or the checks.
pub const CONTENT_RULE: &str = "The result block's keys and values follow the subset above, and \
                                its questions pass every check in this section; save-output.py \
                                lists every violation at once and saves nothing until all are \
                                fixed.";

/// The result-block template of "Result block" in `.mintswarm/stages.md`, verbatim, with its five
/// required keys: the rule [`check_expected`] quotes.
pub const EXPECTED_RULE: &str = "End your final reply with exactly one block of this form; \
anything after it is ignored:

```yaml
msw-result:
  status: complete          # complete | needs_input | failed
  artefact: analysis        # the document you return: analysis | design | decomposition | plan | assessment | handoff | address | gates | review | landing
  outputs: [.mintswarm/job/outputs/analysis.md]   # where the runner saves it
  questions: []             # the open questions and assumptions as a JSON array of question objects (see Questions); `[]` when none
  reason: \"\"                # failed: why, in one sentence";

/// The rule text [`RULES`] gives `rule`.
fn rule_text(rule: &str) -> &'static str {
    RULES
        .iter()
        .find(|(name, _)| *name == rule)
        .map_or("", |(_, text)| text)
}

/// Why a reply cannot be saved: the fault, and the rule it breaks with that rule's text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fault {
    /// What is wrong, naming the line where there is one; may span several lines.
    pub fault: String,
    /// `rule 1` to `rule 6`, or `Result block`.
    pub rule: &'static str,
    /// The rule quoted verbatim from `.mintswarm/stages.md`.
    pub quote: &'static str,
}

impl Fault {
    fn new(fault: impl Into<String>, rule: &'static str) -> Fault {
        Fault {
            fault: fault.into(),
            rule,
            quote: rule_text(rule),
        }
    }

    /// A list of violations under `Result block`, quoting `quote`.
    fn violations(violations: &[String], quote: &'static str) -> Fault {
        let count = violations.len();
        let plural = if count > 1 { "s" } else { "" };
        let listed: Vec<String> = violations.iter().map(|v| format!("- {v}")).collect();
        Fault {
            fault: format!(
                "the result block has {count} violation{plural}:\n{}",
                listed.join("\n")
            ),
            rule: "Result block",
            quote,
        }
    }

    /// The fault and the rule it breaks, as `save-output.py` prints them after `save-output: `.
    pub fn explain(&self) -> String {
        format!(
            "{}\nbreaks {} of {STAGES}: {}",
            self.fault, self.rule, self.quote
        )
    }
}

impl std::fmt::Display for Fault {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.explain())
    }
}

impl std::error::Error for Fault {}

/// A reply that follows the rules: what `save-output.py` saves and prints for it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Capture {
    /// The document: the lines strictly between the fences, unchanged, then a final newline.
    pub document: String,
    /// The result block from `msw-result:` on, without its fences.
    pub result: String,
    /// The result block, read.
    pub outcome: StageOutcome,
    /// The result record: the outcome as `json.dumps(indent=2, ensure_ascii=False)` writes it,
    /// with a final newline.
    pub record: String,
    /// The document's length in characters (code points).
    pub characters: usize,
}

/// The backtick run of a document opening line, `` (`{4,})markdown[ \t]* ``, in bytes.
pub(crate) fn opener(line: &str) -> Option<usize> {
    let run = line.len() - line.trim_start_matches('`').len();
    let rest = line[run..].strip_prefix("markdown")?;
    (run >= 4 && rest.trim_start_matches([' ', '\t']).is_empty()).then_some(run)
}

/// Whether `line` is exactly `prefix` followed by spaces and tabs.
fn is_fence(line: &str, prefix: &str) -> bool {
    line.strip_prefix(prefix)
        .is_some_and(|rest| rest.trim_start_matches([' ', '\t']).is_empty())
}

/// `RESULT_FENCE`, ``` ```(?!`)(.*) ```: the text after a fence of exactly three backticks.
fn result_fence(line: &str) -> Option<&str> {
    let rest = line.strip_prefix("```")?;
    (!rest.starts_with('`')).then_some(rest)
}

fn leading_backticks(line: &str) -> usize {
    let stripped = line.trim_start_matches(' ');
    stripped.len() - stripped.trim_start_matches('`').len()
}

/// Whether `lines[after..]` hold only blank lines, then a three-backtick fence line.
fn result_follows(lines: &[&str], after: usize) -> bool {
    let mut i = after;
    while i < lines.len() && py_strip(lines[i]).is_empty() {
        i += 1;
    }
    i < lines.len() && result_fence(lines[i]).is_some()
}

/// The document block as `(first, last, fence)`: the document is `lines[first..last]`, opened
/// with `fence` backticks.
fn find_document(lines: &[&str]) -> Result<(usize, usize, usize), Fault> {
    let mut blocks = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let Some(fence) = opener(lines[i]) else {
            i += 1;
            continue;
        };
        let closer = "`".repeat(fence);
        let Some(j) = (i + 1..lines.len()).find(|&j| is_fence(lines[j], &closer)) else {
            return Err(Fault::new(
                format!(
                    "line {}: document block opened with {fence} backticks is never closed",
                    i + 1
                ),
                "rule 2",
            ));
        };
        blocks.push((i + 1, j, fence));
        if result_follows(lines, j + 1) {
            break;
        }
        i = j + 1;
    }
    match blocks.as_slice() {
        [] => Err(Fault::new(
            "no document block (a line of 4 or more backticks followed by 'markdown')",
            "rule 1",
        )),
        [only] => Ok(*only),
        _ => {
            let starts: Vec<String> = blocks
                .iter()
                .map(|(first, _, _)| first.to_string())
                .collect();
            Err(Fault::new(
                format!(
                    "{} document blocks, opened on lines {}; expected exactly one",
                    blocks.len(),
                    starts.join(", ")
                ),
                "rule 5",
            ))
        }
    }
}

fn check_document(lines: &[&str], first: usize, last: usize, fence: usize) -> Result<(), Fault> {
    let body = &lines[first..last];
    if !body.first().is_some_and(|line| line.starts_with("# ")) {
        return Err(Fault::new(
            format!(
                "line {}: the document does not start with a '# ' title",
                first + 1
            ),
            "rule 4",
        ));
    }
    for (k, line) in body.iter().enumerate() {
        let run = leading_backticks(line);
        if run >= fence {
            return Err(Fault::new(
                format!(
                    "line {}: a run of {run} backticks inside a document opened with {fence}; \
                     open it with more backticks",
                    first + k + 1
                ),
                "rule 3",
            ));
        }
    }
    Ok(())
}

/// The result block's lines from `msw-result:` on, and the reply line of `msw-result:`.
fn find_result<'l>(lines: &[&'l str], after: usize) -> Result<(Vec<&'l str>, usize), Fault> {
    let mut i = after;
    while i < lines.len() && py_strip(lines[i]).is_empty() {
        i += 1;
    }
    if i == lines.len() {
        return Err(Fault::new(
            "no msw-result block after the document block",
            "rule 6",
        ));
    }
    let Some(tag) = result_fence(lines[i]) else {
        return Err(Fault::new(
            format!(
                "line {}: text between the document block and the result block: {}",
                i + 1,
                py_repr(py_strip(lines[i]))
            ),
            "rule 6",
        ));
    };
    if !is_fence(lines[i], "```yaml") {
        let tag = py_strip(tag);
        let found = if tag.is_empty() {
            "untagged".to_string()
        } else {
            format!("tagged {}", py_repr(tag))
        };
        return Err(Fault::new(
            format!(
                "line {}: the result block's fence is {found}; it must be ```yaml",
                i + 1
            ),
            "Result block",
        ));
    }
    if !lines
        .get(i + 1)
        .is_some_and(|line| line.starts_with("msw-result:"))
    {
        let found = lines
            .get(i + 1)
            .map_or_else(|| "nothing".to_string(), |line| py_repr(line));
        return Err(Fault::new(
            format!(
                "line {}: the result block's first line is {found}; it must be 'msw-result:'",
                i + 2
            ),
            "Result block",
        ));
    }
    let Some(j) = (i + 2..lines.len()).find(|&j| is_fence(lines[j], "```")) else {
        return Err(Fault::new(
            format!("line {}: result block is never closed", i + 1),
            "Result block",
        ));
    };
    Ok((lines[i + 1..j].to_vec(), i + 2))
}

/// `save-output.py`'s `extract`: the reply's document, result block and outcome.
fn extract(text: &str) -> Result<Capture, Fault> {
    let raw: Vec<&str> = text.split('\n').collect();
    let lines: Vec<&str> = raw
        .iter()
        .map(|line| line.strip_suffix('\r').unwrap_or(line))
        .collect();
    let (first, last, fence) = find_document(&lines)?;
    check_document(&lines, first, last, fence)?;
    let (block, at) = find_result(&lines, last + 1)?;
    let outcome =
        block::parse_result(&block, at).map_err(|found| Fault::violations(&found, CONTENT_RULE))?;
    let mut document = raw[first..last].join("\n");
    document.push('\n');
    let tree = json::to_json(&outcome).expect("a StageOutcome serialises to strict JSON");
    Ok(Capture {
        characters: document.chars().count(),
        document,
        result: block.join("\n"),
        record: json::to_file_text(&tree, Style::DISK),
        outcome,
    })
}

/// Captures the document and result block of `reply`, as `save-output.py --text` reads it.
pub fn capture(reply: &str) -> Result<Capture, Fault> {
    if py_strip(reply).is_empty() {
        return Err(Fault::new("no reply text", "rule 1"));
    }
    extract(reply)
}

/// Checks a captured outcome against the stage that returned it: its `artefact` is `artefact`,
/// its `outputs` are exactly `[.mintswarm/job/outputs/<artefact>.md]`, and a `failed` outcome has
/// a non-blank `reason`. Every mismatch is listed at once, each starting with its field, under
/// `Result block`, quoting [`EXPECTED_RULE`].
pub fn check_expected(outcome: &StageOutcome, artefact: &str) -> Result<(), Fault> {
    let mut found = Vec::new();
    if outcome.artefact != artefact {
        found.push(format!(
            "artefact: {:?} is not the stage's artefact {artefact:?}",
            outcome.artefact
        ));
    }
    let expected = format!(".mintswarm/job/outputs/{artefact}.md");
    if outcome.outputs != [expected.as_str()] {
        found.push(format!(
            "outputs: [{}] must be exactly [{expected}]",
            outcome.outputs.join(", ")
        ));
    }
    if outcome.status == OutcomeStatus::Failed && outcome.reason.trim().is_empty() {
        found.push("reason: a failed outcome needs a reason, in one sentence".into());
    }
    if found.is_empty() {
        Ok(())
    } else {
        Err(Fault::violations(&found, EXPECTED_RULE))
    }
}
