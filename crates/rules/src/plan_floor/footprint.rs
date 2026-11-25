//! The plan's footprint: `locate_footprint`, `parse_footprint`, `check_path`, `check_list` and
//! `check_entries` in `.mintswarm/dogfood/plan-approval/floor.py`.

use std::fmt;

use mintswarm_domain::records::Json;

use crate::json::{self, Style};
use crate::markdown::{document_lines, py_strip};
use crate::py::{lower, repr, repr_str};

/// The seven footprint keys, in their required order.
pub const KEYS: [&str; 7] = [
    "files",
    "new_files",
    "estimated_lines",
    "new_dependencies",
    "external_steps",
    "gate_changes",
    "overridden_assumptions",
];

/// The eight-key footprint: `production_lines` right after `estimated_lines`.
pub const KEYS_V2: [&str; 8] = [
    "files",
    "new_files",
    "estimated_lines",
    "production_lines",
    "new_dependencies",
    "external_steps",
    "gate_changes",
    "overridden_assumptions",
];

/// A package the plan adds, named with the manifest that declares it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Dependency {
    /// `name`.
    pub name: String,
    /// `manifest`, a declared path.
    pub manifest: String,
}

/// An assumption of the analysis the plan overrides.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Assumption {
    /// `assumption`.
    pub assumption: String,
    /// `reason`.
    pub reason: String,
}

/// A footprint, normalised to the eight-key shape: a seven-key footprint's `production_lines` is
/// its `estimated_lines`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Footprint {
    /// `files`, never empty.
    pub files: Vec<String>,
    /// `new_files`, each also in `files`.
    pub new_files: Vec<String>,
    /// `estimated_lines`.
    pub estimated_lines: u64,
    /// `production_lines`, at most `estimated_lines`.
    pub production_lines: u64,
    /// `new_dependencies`.
    pub new_dependencies: Vec<Dependency>,
    /// `external_steps`.
    pub external_steps: Vec<String>,
    /// `gate_changes`.
    pub gate_changes: Vec<String>,
    /// `overridden_assumptions`.
    pub overridden_assumptions: Vec<Assumption>,
}

impl Footprint {
    /// The declared paths: `files`, then each new dependency's manifest that is not among them.
    pub fn declared(&self) -> Vec<&str> {
        let mut declared: Vec<&str> = self.files.iter().map(String::as_str).collect();
        for dependency in &self.new_dependencies {
            if !declared.contains(&dependency.manifest.as_str()) {
                declared.push(&dependency.manifest);
            }
        }
        declared
    }
}

/// Why the plan has no usable footprint: the `footprint` rule's reason, word for word.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FootprintError {
    message: String,
}

impl FootprintError {
    fn new(message: impl Into<String>) -> Self {
        FootprintError {
            message: message.into(),
        }
    }

    /// The `footprint` rule's reason.
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for FootprintError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for FootprintError {}

type Parsed<T> = Result<T, FootprintError>;

fn is_heading(line: &str) -> bool {
    line.starts_with("## ")
}

/// Whether `line` opens a json fence by `floor.py`'s `FENCE`: after any spaces and tabs, a run of
/// three or more backticks or tildes followed by `json` in any case. Python's `re.IGNORECASE`
/// folds Unicode case, under which only `s` gains a letter: U+017F LATIN SMALL LETTER LONG S.
fn is_json_fence(line: &str) -> bool {
    let stripped = line.trim_start_matches([' ', '\t']);
    ['`', '~'].into_iter().any(|c| {
        let rest = stripped.trim_start_matches(c);
        let fold = |c: char| {
            if c == '\u{17f}' {
                's'
            } else {
                c.to_ascii_lowercase()
            }
        };
        stripped.len() - rest.len() >= 3 && rest.chars().take(4).map(fold).eq("json".chars())
    })
}

/// The text of the footprint block, as `locate_footprint` finds it.
fn locate(lines: &[&str]) -> Parsed<String> {
    let sections: Vec<usize> = (0..lines.len())
        .filter(|i| lines[*i] == "## Footprint")
        .collect();
    let section = match sections.as_slice() {
        [] => return Err(FootprintError::new("no '## Footprint' section")),
        [section] => *section,
        _ => {
            return Err(FootprintError::new(format!(
                "{} '## Footprint' sections",
                sections.len()
            )));
        }
    };
    match lines[..section].iter().rev().find(|line| is_heading(line)) {
        Some(&"## Steps") => {}
        before => {
            let found = before.map_or_else(|| "nothing".to_owned(), |line| repr_str(line));
            return Err(FootprintError::new(format!(
                "'## Footprint' must follow '## Steps', it follows {found}"
            )));
        }
    }
    let fences: Vec<usize> = (0..lines.len())
        .filter(|i| is_json_fence(lines[*i]))
        .collect();
    let start = match fences.as_slice() {
        [start] => *start,
        _ => {
            return Err(FootprintError::new(format!(
                "{} json fences in the plan, expected 1",
                fences.len()
            )));
        }
    };
    if lines[start] != "```json" {
        return Err(FootprintError::new(format!(
            "line {}: the json fence must be exactly '```json'",
            start + 1
        )));
    }
    let previous = lines[..start]
        .iter()
        .rev()
        .find(|line| !py_strip(line).is_empty());
    if previous != Some(&"## Footprint") {
        return Err(FootprintError::new(
            "the json fence must come right after '## Footprint'",
        ));
    }
    let end = (start + 1..lines.len())
        .find(|i| lines[*i] == "```")
        .ok_or_else(|| FootprintError::new("the json fence is not closed"))?;
    for (i, line) in lines.iter().enumerate().skip(end + 1) {
        if is_heading(line) {
            break;
        }
        if !py_strip(line).is_empty() {
            return Err(FootprintError::new(format!(
                "line {}: text after the footprint block",
                i + 1
            )));
        }
    }
    Ok(lines[start + 1..end].join("\n"))
}

/// `check_path`: a declared path is relative, plain and outside `.mintswarm/job/`.
fn check_path(path: &str, place: &str) -> Parsed<()> {
    let bad = |why: &str| {
        Err(FootprintError::new(format!(
            "{place}: {} {why}",
            repr_str(path)
        )))
    };
    if path.is_empty() {
        return bad("is empty");
    }
    if path.chars().any(char::is_control) {
        return bad("has a control character");
    }
    for c in ['\\', '*', '?', '[', ']'] {
        if path.contains(c) {
            return bad(&format!("has {}", repr_str(&c.to_string())));
        }
    }
    if path.starts_with('/') {
        return bad("is absolute");
    }
    if path.starts_with("./") {
        return bad("starts with './'");
    }
    if path.ends_with('/') {
        return bad("ends with '/'");
    }
    if path
        .split('/')
        .any(|segment| matches!(segment, "" | "." | ".."))
    {
        return bad("has an empty, '.' or '..' segment");
    }
    let lowered = lower(path);
    if lowered == ".mintswarm/job" || lowered.starts_with(".mintswarm/job/") {
        return bad("is under .mintswarm/job/");
    }
    Ok(())
}

/// `check_list`: a list without null and without a value twice, compared in canonical form.
fn check_list<'j>(value: &'j Json, key: &str) -> Parsed<&'j [Json]> {
    let Json::Array(items) = value else {
        return Err(FootprintError::new(format!("{key} must be a list")));
    };
    let mut seen: Vec<String> = Vec::with_capacity(items.len());
    for item in items {
        if *item == Json::Null {
            return Err(FootprintError::new(format!("{key} holds null")));
        }
        let form = json::dumps(item, Style::CANONICAL);
        if seen.contains(&form) {
            return Err(FootprintError::new(format!("{key} holds {form} twice")));
        }
        seen.push(form);
    }
    Ok(items)
}

fn get<'j>(entries: &'j [(String, Json)], key: &str) -> Option<&'j Json> {
    entries.iter().find(|(k, _)| k == key).map(|(_, v)| v)
}

/// `check_entries`: each entry an object with exactly the string fields `fields`.
fn check_entries(items: &[Json], key: &str, fields: [&str; 2]) -> Parsed<Vec<[String; 2]>> {
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        let entries = match item {
            Json::Object(entries)
                if entries.len() == fields.len()
                    && fields.iter().all(|field| get(entries, field).is_some()) =>
            {
                entries
            }
            _ => {
                return Err(FootprintError::new(format!(
                    "each {key} entry must have exactly {}",
                    fields.join(", ")
                )));
            }
        };
        let mut values = [String::new(), String::new()];
        for (slot, field) in values.iter_mut().zip(fields) {
            match get(entries, field) {
                Some(Json::String(s)) => *slot = s.clone(),
                _ => {
                    return Err(FootprintError::new(format!(
                        "{key}.{field} must be a string"
                    )));
                }
            }
        }
        out.push(values);
    }
    Ok(out)
}

/// A non-negative integer that is not a boolean, as Python's `isinstance(v, int)` reads it.
fn count(value: &Json) -> Option<u64> {
    match value {
        Json::Int(i) => u64::try_from(*i).ok(),
        Json::UInt(u) => Some(*u),
        _ => None,
    }
}

fn strings(items: &[Json], key: &str) -> Parsed<Vec<String>> {
    items
        .iter()
        .map(|item| match item {
            Json::String(s) => Ok(s.clone()),
            _ => Err(FootprintError::new(format!("{key} must hold strings"))),
        })
        .collect()
}

/// The footprint of the plan `text`, as `parse_footprint` reads it, normalised to eight keys.
pub fn parse(text: &str) -> Parsed<Footprint> {
    let lines = document_lines(text);
    let block = locate(&lines)?;
    let document = json::read(&block).map_err(|error| {
        FootprintError::new(format!(
            "the footprint is not valid JSON: {}",
            error.python_message()
        ))
    })?;
    let footprint = match &document {
        Json::Object(entries) if entries.len() == 1 && entries[0].0 == "footprint" => &entries[0].1,
        _ => {
            return Err(FootprintError::new(
                "the top level must be an object with the single key 'footprint'",
            ));
        }
    };
    let Json::Object(entries) = footprint else {
        return Err(FootprintError::new("footprint must be an object"));
    };
    let keys: Vec<&str> = entries.iter().map(|(key, _)| key.as_str()).collect();
    if keys != KEYS && keys != KEYS_V2 {
        return Err(FootprintError::new(format!(
            "footprint keys must be exactly {}, in that order, \
             optionally with production_lines right after estimated_lines",
            KEYS.join(", ")
        )));
    }
    let field = |key: &str| get(entries, key).unwrap_or(&Json::Null);
    let lines_value = field("estimated_lines");
    let estimated_lines = count(lines_value).ok_or_else(|| {
        FootprintError::new(format!(
            "estimated_lines must be an integer of 0 or more, not {}",
            repr(lines_value)
        ))
    })?;
    let production_value = get(entries, "production_lines").unwrap_or(lines_value);
    let production_lines = count(production_value)
        .filter(|production| *production <= estimated_lines)
        .ok_or_else(|| {
            FootprintError::new(format!(
                "production_lines must be an integer from 0 to estimated_lines, not {}",
                repr(production_value)
            ))
        })?;
    let mut lists = Vec::with_capacity(KEYS.len() - 1);
    for key in KEYS {
        if key != "estimated_lines" {
            lists.push((key, check_list(field(key), key)?));
        }
    }
    let list = |key: &str| {
        lists
            .iter()
            .find(|(k, _)| *k == key)
            .map_or(&[][..], |(_, items)| *items)
    };
    let files = strings(list("files"), "files")?;
    let new_files = strings(list("new_files"), "new_files")?;
    let external_steps = strings(list("external_steps"), "external_steps")?;
    let gate_changes = strings(list("gate_changes"), "gate_changes")?;
    let new_dependencies: Vec<Dependency> = check_entries(
        list("new_dependencies"),
        "new_dependencies",
        ["name", "manifest"],
    )?
    .into_iter()
    .map(|[name, manifest]| Dependency { name, manifest })
    .collect();
    let overridden_assumptions: Vec<Assumption> = check_entries(
        list("overridden_assumptions"),
        "overridden_assumptions",
        ["assumption", "reason"],
    )?
    .into_iter()
    .map(|[assumption, reason]| Assumption { assumption, reason })
    .collect();
    if files.is_empty() {
        return Err(FootprintError::new(
            "files is empty: a plan changes at least one file",
        ));
    }
    for path in &new_files {
        if !files.contains(path) {
            return Err(FootprintError::new(format!(
                "new_files: {} is not in files",
                repr_str(path)
            )));
        }
    }
    for path in &files {
        check_path(path, "files")?;
    }
    for path in &new_files {
        check_path(path, "new_files")?;
    }
    for dependency in &new_dependencies {
        check_path(&dependency.manifest, "new_dependencies.manifest")?;
    }
    Ok(Footprint {
        files,
        new_files,
        estimated_lines,
        production_lines,
        new_dependencies,
        external_steps,
        gate_changes,
        overridden_assumptions,
    })
}

#[cfg(test)]
mod tests {
    use super::is_json_fence;

    #[test]
    fn a_json_fence_folds_case_as_python_does() {
        for line in ["```json", "  ~~~~JSON", "\t```JſON x", "```jſon"] {
            assert!(is_json_fence(line), "{line:?}");
        }
        for line in ["``json", "```jso", "```yaml", "```jzon"] {
            assert!(!is_json_fence(line), "{line:?}");
        }
    }
}
