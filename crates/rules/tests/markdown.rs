//! Headings, sections and fences against the `markdown` lines of `tests/fixtures/python.jsonl`,
//! which `capture.py` records from `floor.py` and `collect-facts.py`.

use std::path::PathBuf;

use mintswarm_domain::records::Json;
use mintswarm_rules::json::read;
use mintswarm_rules::markdown::{
    SectionError, document_lines, headings, is_py_space, issue_body, py_rstrip, py_strip,
    scan_fences, section,
};

fn field<'j>(value: &'j Json, key: &str) -> &'j Json {
    let Json::Object(entries) = value else {
        panic!("not an object: {value:?}");
    };
    entries
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v)
        .unwrap_or_else(|| panic!("no {key} in {value:?}"))
}

fn has(value: &Json, key: &str) -> bool {
    matches!(value, Json::Object(entries) if entries.iter().any(|(k, _)| k == key))
}

fn string(value: &Json) -> &str {
    match value {
        Json::String(s) => s,
        other => panic!("not a string: {other:?}"),
    }
}

fn number(value: &Json) -> usize {
    match value {
        Json::Int(i) => usize::try_from(*i).expect("the number is an index"),
        other => panic!("not an integer: {other:?}"),
    }
}

fn array(value: &Json) -> &[Json] {
    match value {
        Json::Array(items) => items,
        other => panic!("not an array: {other:?}"),
    }
}

fn documents() -> Vec<Json> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/python.jsonl");
    let text = std::fs::read_to_string(&path).expect("the Python fixture is readable");
    let documents: Vec<Json> = text
        .lines()
        .map(|line| read(line).expect("the Python fixture is strict JSON"))
        .filter(|entry| string(field(entry, "kind")) == "markdown")
        .collect();
    assert!(documents.len() >= 20, "the plan, review and edge documents");
    documents
}

#[test]
fn headings_match_floor_py() {
    for document in documents() {
        let name = string(field(&document, "name"));
        let lines = document_lines(string(field(&document, "text")));
        let want: Vec<(usize, u8)> = array(field(&document, "headings"))
            .iter()
            .map(|mark| {
                let pair = array(mark);
                let level = u8::try_from(number(&pair[1])).expect("a level fits");
                (number(&pair[0]), level)
            })
            .collect();
        assert_eq!(headings(&lines), want, "{name}");
    }
}

#[test]
fn sections_match_floor_py() {
    let mut errors = 0;
    for document in documents() {
        let name = string(field(&document, "name"));
        let what = string(field(&document, "what"));
        let lines = document_lines(string(field(&document, "text")));
        for entry in array(field(&document, "sections")) {
            let title = string(field(entry, "title"));
            let ends: Vec<u8> = array(field(entry, "ends"))
                .iter()
                .map(|end| u8::try_from(number(end)).expect("a level fits"))
                .collect();
            let got = section(&lines, title, &ends);
            if has(entry, "error") {
                let error = got.expect_err("Python finds no single section");
                assert_eq!(
                    error.message(what, title),
                    string(field(entry, "error")),
                    "{name}: {title}"
                );
                errors += 1;
            } else {
                let got = got.unwrap_or_else(|e| panic!("{name}: {title}: {e:?}"));
                assert_eq!(got.start, number(field(entry, "start")), "{name}: {title}");
                assert_eq!(
                    got.body.len(),
                    number(field(entry, "length")),
                    "{name}: {title} {ends:?}"
                );
                assert_eq!(got.body, &lines[got.start..got.start + got.body.len()]);
            }
        }
    }
    assert!(
        errors > 0,
        "the fixture holds missing and repeated sections"
    );
}

#[test]
fn issue_body_matches_floor_py() {
    for document in documents() {
        let name = string(field(&document, "name"));
        let lines = document_lines(string(field(&document, "text")));
        assert_eq!(
            issue_body(&lines).len(),
            number(field(&document, "issue_body")),
            "{name}"
        );
    }
}

#[test]
fn fences_match_collect_facts_py() {
    for document in documents() {
        let name = string(field(&document, "name"));
        let text = string(field(&document, "text")).replace("\r\n", "\n");
        let lines: Vec<&str> = text.split('\n').collect();
        let fences = scan_fences(&lines);
        let want = field(&document, "fences");
        let outside: Vec<usize> = array(field(want, "outside")).iter().map(number).collect();
        let got: Vec<usize> = fences.outside.iter().map(|(i, _)| *i).collect();
        assert_eq!(got, outside, "{name}: outside");
        assert!(fences.outside.iter().all(|(i, line)| lines[*i] == *line));
        let blocks: Vec<(&str, Vec<&str>)> = array(field(want, "blocks"))
            .iter()
            .map(|block| {
                let content = array(field(block, "content")).iter().map(string).collect();
                (string(field(block, "info")), content)
            })
            .collect();
        let got: Vec<(&str, Vec<&str>)> = fences
            .blocks
            .iter()
            .map(|block| (block.info, block.content.clone()))
            .collect();
        assert_eq!(got, blocks, "{name}: blocks");
    }
}

#[test]
fn whitespace_is_pythons_set() {
    for c in [
        '\u{1c}', '\u{1d}', '\u{1e}', '\u{1f}', '\u{85}', '\u{a0}', '\u{3000}', '\u{b}',
    ] {
        assert!(is_py_space(c), "{c:?}");
    }
    for c in ['\u{200b}', '\u{feff}', 'a', '\u{0}'] {
        assert!(!is_py_space(c), "{c:?}");
    }
    assert_eq!(py_strip("\u{1c} a \u{1f}"), "a");
    assert_eq!(py_rstrip(" a\u{2028}"), " a");
}

#[test]
fn section_errors_rebuild_floor_py_messages() {
    assert_eq!(
        SectionError::Missing.message("the plan", "Verification"),
        "the plan: no '## Verification' section"
    );
    assert_eq!(
        SectionError::Repeated(2).message("the plan", "Verification"),
        "the plan: 2 '## Verification' sections"
    );
}

#[test]
fn document_lines_strip_one_carriage_return() {
    assert_eq!(document_lines("a\r\r\nb\n"), vec!["a\r", "b", ""]);
}
