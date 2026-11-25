//! The three document rules: `blocking-questions`, `acceptance-criteria` and `first-of-kind`,
//! `evaluate_documents` and the functions it calls in `.mintswarm/dogfood/plan-approval/floor.py`.

use mintswarm_domain::records::{Json, RuleResult};

use super::rule;
use crate::json;
use crate::markdown::{document_lines, headings, is_py_space, issue_body, py_strip, section};
use crate::py::{flat, repr_str};

const MUST_DECIDE: &str = "- **Must decide:**";
const RECOMMENDED: &str = "- **Recommended:**";
const ASSUMPTION: &str = "- **Assumption:**";
const BLOCKING: &str = "- **Blocking:**";
const NON_BLOCKING: &str = "- **Non-blocking (assumed):**";
const KIND: &str = "- **Kind:**";
const COMPONENT: &str = "- **Component:**";

/// What a `## Questions` bullet counts as.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Count {
    MustDecide,
    Recommended,
    /// The old marker, either blocking kind.
    Blocking,
    Assumption,
}

/// The `## Questions` markers, in the order they are tried.
const MARKERS: [(&str, Count); 5] = [
    (MUST_DECIDE, Count::MustDecide),
    (RECOMMENDED, Count::Recommended),
    (BLOCKING, Count::Blocking),
    (ASSUMPTION, Count::Assumption),
    (NON_BLOCKING, Count::Assumption),
];

/// A question kind of a result record.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    MustDecide,
    Recommended,
    Assumption,
}

impl Kind {
    fn name(self) -> &'static str {
        match self {
            Kind::MustDecide => "must_decide",
            Kind::Recommended => "recommended",
            Kind::Assumption => "assumption",
        }
    }
}

fn get<'j>(entries: &'j [(String, Json)], key: &str) -> Option<&'j Json> {
    entries.iter().find(|(k, _)| k == key).map(|(_, v)| v)
}

/// The `(kind, question)` pairs of a result record, as `record_questions` reads them, or its
/// fixed cause when it is not usable.
fn record_questions(text: &str) -> Result<Vec<(Kind, String)>, &'static str> {
    let record = json::read(text).map_err(|_| "it is not strict JSON")?;
    let Json::Object(entries) = &record else {
        return Err("it is not a JSON object");
    };
    let Some(Json::Array(questions)) = get(entries, "questions") else {
        return Err("its questions is not a list");
    };
    let mut pairs = Vec::with_capacity(questions.len());
    for question in questions {
        let Json::Object(fields) = question else {
            return Err("a question is not a JSON object");
        };
        let kind = match get(fields, "kind") {
            Some(Json::String(kind)) if kind == "must_decide" => Kind::MustDecide,
            Some(Json::String(kind)) if kind == "recommended" => Kind::Recommended,
            Some(Json::String(kind)) if kind == "assumption" => Kind::Assumption,
            _ => return Err("a question's kind is not must_decide, recommended or assumption"),
        };
        let Some(Json::String(text)) = get(fields, "question") else {
            return Err("a question's question is not a string");
        };
        pairs.push((kind, text.clone()));
    }
    Ok(pairs)
}

/// The `blocking-questions` reasons of one document, as `question_reasons` gives them; `record`
/// is the text of its result record, or `None` when it has none.
fn question_reasons(text: &str, what: &str, record: Option<&str>) -> Vec<String> {
    let mut reasons = Vec::new();
    let mut questions = None;
    if let Some(record) = record {
        match record_questions(record) {
            Err(cause) => reasons.push(format!("{what}: the result record is not usable: {cause}")),
            Ok(pairs) => {
                for (kind, question) in &pairs {
                    if *kind != Kind::Assumption {
                        reasons.push(format!(
                            "{what}: blocking question ({}): {}",
                            kind.name(),
                            flat(question)
                        ));
                    }
                }
                questions = Some(pairs);
            }
        }
    }
    let lines = document_lines(text);
    let questions_section = match section(&lines, "Questions", &[2]) {
        Ok(found) => found,
        Err(error) => {
            reasons.push(error.message(what, "Questions"));
            return reasons;
        }
    };
    let (mut must_decide, mut recommended, mut blocking, mut assumption, mut unknown) =
        (0usize, 0usize, 0usize, 0usize, 0usize);
    for (offset, line) in questions_section.body.iter().enumerate() {
        let number = questions_section.start + offset + 1;
        let entry = line.trim_start_matches([' ', '\t']);
        if entry.is_empty() {
            continue;
        }
        let indented = line.starts_with([' ', '\t']);
        let marker = MARKERS.iter().find(|(m, _)| entry.starts_with(m));
        match marker {
            Some((m, count)) if *count != Count::Assumption || !indented => {
                if record.is_none() && *count != Count::Assumption {
                    reasons.push(format!(
                        "{what}: blocking question: {}",
                        flat(&entry[m.len()..])
                    ));
                }
                match count {
                    Count::MustDecide => must_decide += 1,
                    Count::Recommended => recommended += 1,
                    Count::Blocking => blocking += 1,
                    Count::Assumption => assumption += 1,
                }
            }
            _ if indented => {}
            _ if line.starts_with("# ") => reasons.push(format!(
                "{what}: line {number} of '## Questions' is a '# ' heading inside the section"
            )),
            _ => {
                reasons.push(format!(
                    "{what}: line {number} of '## Questions' is not an entry with a known marker"
                ));
                unknown += 1;
            }
        }
    }
    if let Some(pairs) = questions {
        let of = |kind: Kind| pairs.iter().filter(|(k, _)| *k == kind).count();
        let (m, r, a) = (
            of(Kind::MustDecide),
            of(Kind::Recommended),
            of(Kind::Assumption),
        );
        let agree = unknown == 0
            && assumption == a
            && must_decide <= m
            && recommended <= r
            && (m - must_decide) + (r - recommended) == blocking;
        if !agree {
            reasons.push(format!(
                "{what}: the '## Questions' bullets (must decide {must_decide}, \
                 recommended {recommended}, blocking {blocking}, assumption {assumption}, \
                 unknown {unknown}) disagree with the result record (must decide {m}, \
                 recommended {r}, assumption {a})"
            ));
        }
    }
    reasons
}

/// The words of `text` joined by single spaces: `" ".join(text.split())`.
fn normalise(text: &str) -> String {
    text.split(is_py_space)
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// What follows a list marker when `rest` is all of the line after it: `None` unless it is empty
/// or starts with a space or tab, which `(?:[ \t]+|$)(.*)` then drops.
fn after_marker(rest: &str) -> Option<&str> {
    if rest.is_empty() || rest.starts_with([' ', '\t']) {
        Some(rest.trim_start_matches([' ', '\t']))
    } else {
        None
    }
}

/// The text of a criterion item on `line`, by `floor.py`'s `ITEM` and `CHECKBOX`.
fn item(line: &str) -> Option<&str> {
    let digits = line.len() - line.trim_start_matches(|c: char| c.is_ascii_digit()).len();
    let rest = if digits > 0 {
        line[digits..].strip_prefix(['.', ')'])?
    } else {
        line.strip_prefix(['-', '*', '+'])?
    };
    let rest = after_marker(rest)?;
    let boxed = rest
        .strip_prefix("[ ]")
        .or_else(|| rest.strip_prefix("[x]"))
        .and_then(after_marker);
    Some(boxed.unwrap_or(rest))
}

/// The issue's criteria, as `issue_criteria` reads them.
fn issue_criteria(text: &str) -> Result<Vec<String>, String> {
    let lines = document_lines(text);
    let body = issue_body(&lines);
    let found = section(body, "Acceptance criteria", &[1, 2])
        .map_err(|error| error.message("the issue document", "Acceptance criteria"))?;
    let mut criteria: Vec<Vec<&str>> = Vec::new();
    let mut open = false;
    let mut gap = false;
    for line in found.body {
        if py_strip(line).is_empty() {
            gap = true;
            continue;
        }
        if let Some(text) = item(line) {
            criteria.push(vec![text]);
            open = true;
        } else if open && (!gap || line.starts_with([' ', '\t'])) {
            if let Some(current) = criteria.last_mut() {
                current.push(line);
            }
        } else {
            open = false;
        }
        gap = false;
    }
    Ok(criteria
        .iter()
        .map(|parts| normalise(&parts.join(" ")))
        .collect())
}

/// The cells of a table row, as `table_cells` splits them.
fn table_cells(line: &str) -> Vec<String> {
    let mut row = py_strip(line);
    if let Some(rest) = row.strip_prefix('|') {
        row = rest;
    }
    if row.ends_with('|') && !row.ends_with("\\|") {
        row = &row[..row.len() - 1];
    }
    let mut cells = Vec::new();
    let mut cell = String::new();
    let mut previous = None;
    for c in row.chars() {
        if c == '|' && previous != Some('\\') {
            cells.push(std::mem::take(&mut cell));
        } else {
            cell.push(c);
        }
        previous = Some(c);
    }
    cells.push(cell);
    cells
        .into_iter()
        .map(|cell| cell.replace("\\|", "|"))
        .collect()
}

/// Whether `cell` is a separator cell: `[ \t]*:?-{3,}:?[ \t]*`.
fn is_separator(cell: &str) -> bool {
    let core = cell.trim_matches([' ', '\t']);
    let core = core.strip_prefix(':').unwrap_or(core);
    let core = core.strip_suffix(':').unwrap_or(core);
    core.len() >= 3 && core.chars().all(|c| c == '-')
}

/// The criteria the plan's `## Verification` table quotes, as `plan_criteria` reads them.
fn plan_criteria(text: &str) -> Result<Vec<String>, String> {
    let lines = document_lines(text);
    let found = section(&lines, "Verification", &[1, 2])
        .map_err(|error| error.message("the plan", "Verification"))?;
    let rows = found
        .body
        .iter()
        .position(|line| !py_strip(line).is_empty())
        .map_or(&[][..], |first| &found.body[first..]);
    let table: Vec<&str> = rows
        .iter()
        .take_while(|line| line.trim_start_matches([' ', '\t']).starts_with('|'))
        .copied()
        .collect();
    if table.len() < 2 || !table_cells(table[1]).iter().all(|c| is_separator(c)) {
        return Err(
            "the plan: '## Verification' does not start with a table header and a '---' row".into(),
        );
    }
    Ok(table[2..]
        .iter()
        .map(|line| {
            table_cells(line)
                .first()
                .map_or_else(String::new, |cell| normalise(cell))
        })
        .collect())
}

/// The `acceptance-criteria` reasons, as `criteria_reasons` gives them.
fn criteria_reasons(issue: &str, plan: &str) -> Vec<String> {
    let wanted = match issue_criteria(issue) {
        Ok(wanted) if wanted.is_empty() => {
            return vec!["the issue document: '## Acceptance criteria' holds no criteria".into()];
        }
        Ok(wanted) => wanted,
        Err(reason) => return vec![reason],
    };
    let quoted = match plan_criteria(plan) {
        Ok(quoted) => quoted,
        Err(reason) => return vec![reason],
    };
    let mut reasons = Vec::new();
    let mut remaining = quoted.clone();
    for criterion in &wanted {
        match remaining.iter().position(|row| row == criterion) {
            Some(at) => {
                remaining.remove(at);
            }
            None => reasons.push(format!(
                "the Verification table lacks the criterion: {}",
                flat(criterion)
            )),
        }
    }
    let mut extra = wanted;
    for row in &quoted {
        match extra.iter().position(|criterion| criterion == row) {
            Some(at) => {
                extra.remove(at);
            }
            None => reasons.push(format!(
                "the Verification table has a row that is not a criterion of the issue: {}",
                flat(row)
            )),
        }
    }
    reasons
}

/// The value of the one header line starting with `marker`, or why there is none.
fn header_value<'a>(header: &[&'a str], marker: &str) -> Result<&'a str, String> {
    let found: Vec<&str> = header
        .iter()
        .copied()
        .filter(|line| line.starts_with(marker))
        .collect();
    match found.as_slice() {
        [line] => Ok(py_strip(&line[marker.len()..])),
        _ => Err(format!(
            "the issue document's header has {} '{marker}' lines, expected 1",
            found.len()
        )),
    }
}

/// Whether `subject` starts as `[A-Za-z]+\(<component>\)!?: ` matches.
fn scoped(subject: &str, component: &str) -> bool {
    let rest = subject.trim_start_matches(|c: char| c.is_ascii_alphabetic());
    if rest.len() == subject.len() {
        return false;
    }
    let Some(rest) = rest
        .strip_prefix('(')
        .and_then(|rest| rest.strip_prefix(component))
        .and_then(|rest| rest.strip_prefix(')'))
    else {
        return false;
    };
    rest.strip_prefix('!').unwrap_or(rest).starts_with(": ")
}

/// The `first-of-kind` reasons, as `first_of_kind_reasons` gives them.
fn first_of_kind_reasons(issue: &str, component: &str, subjects: &[&str]) -> Vec<String> {
    let lines = document_lines(issue);
    let body = issue_body(&lines);
    let first = headings(body)
        .into_iter()
        .find(|(_, level)| *level == 2)
        .map_or(body.len(), |(i, _)| i);
    let header = &body[..first];
    let mut reasons = Vec::new();
    match header_value(header, KIND) {
        Err(why) => reasons.push(why),
        Ok(kind) if kind != "implementation" => reasons.push(format!(
            "the issue's kind is {}, not implementation",
            repr_str(&flat(kind))
        )),
        Ok(_) => {}
    }
    match header_value(header, COMPONENT) {
        Err(why) => reasons.push(why),
        Ok(named) if named != component => reasons.push(format!(
            "the issue's component is {}, not --component {component}",
            repr_str(&flat(named))
        )),
        Ok(_) => {}
    }
    if reasons.is_empty() && !subjects.iter().any(|subject| scoped(subject, component)) {
        reasons.push(format!(
            "first of its kind: no commit reachable from the base commit has a subject scoped \
             ({component})"
        ));
    }
    reasons
}

/// The documents the three rules read, as text.
pub struct Documents<'a> {
    /// The plan.
    pub plan: &'a str,
    /// The issue document, when given.
    pub issue: Option<&'a str>,
    /// The analysis, when given.
    pub analysis: Option<&'a str>,
    /// The analysis's result record, when there is one; read only with the analysis.
    pub analysis_record: Option<&'a str>,
    /// The plan's result record, when there is one.
    pub plan_record: Option<&'a str>,
}

/// The three document rules, in order, as `evaluate_documents` gives them.
pub fn evaluate(documents: &Documents<'_>, component: &str, subjects: &[&str]) -> Vec<RuleResult> {
    let mut reasons = match documents.analysis {
        None => vec!["no analysis given (--analysis)".to_owned()],
        Some(analysis) => question_reasons(analysis, "analysis", documents.analysis_record),
    };
    reasons.extend(question_reasons(
        documents.plan,
        "plan",
        documents.plan_record,
    ));
    let no_issue = || vec!["no issue document given (--issue-document)".to_owned()];
    vec![
        rule("blocking-questions", reasons),
        rule(
            "acceptance-criteria",
            documents
                .issue
                .map_or_else(no_issue, |issue| criteria_reasons(issue, documents.plan)),
        ),
        rule(
            "first-of-kind",
            documents.issue.map_or_else(no_issue, |issue| {
                first_of_kind_reasons(issue, component, subjects)
            }),
        ),
    ]
}
