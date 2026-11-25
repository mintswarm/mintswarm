//! The strict JSON reader and the `json.dumps` writer.

use std::path::PathBuf;

use mintswarm_domain::records::{
    DecisionRecord, FactsRecord, FloorRecord, Json, LandingFloorRecord, RoundRecord,
};
use mintswarm_rules::json::{
    MAX_DEPTH, ReadError, ReadErrorKind, Scanner, Style, dumps, read, to_file_text, to_json,
};
use serde::Serialize;
use serde::de::DeserializeOwned;

fn kind(text: &str) -> ReadErrorKind {
    read(text).expect_err("the text is rejected").kind
}

fn at(text: &str) -> ReadError {
    read(text).expect_err("the text is rejected")
}

#[test]
fn duplicate_keys_are_rejected_at_top_level_and_nested() {
    assert_eq!(
        kind(r#"{"tool": 1, "tool": 2}"#),
        ReadErrorKind::DuplicateKey("tool".into())
    );
    assert_eq!(
        kind(r#"{"a": [{"b": 1, "c": 2, "b": 3}]}"#),
        ReadErrorKind::DuplicateKey("b".into())
    );
    let error = at("{\"a\": 1,\n \"a\": 2}");
    assert_eq!((error.line, error.column), (2, 2));
}

#[test]
fn duplicates_are_found_when_the_object_closes() {
    // object_pairs_hook only runs on a complete object, so a syntax error inside comes first
    assert_eq!(
        kind(r#"{"a": 1, "a": 2, "b" 3}"#),
        ReadErrorKind::Syntax(Scanner::ExpectingColon)
    );
    // and an inner object closes before its outer one
    assert_eq!(
        kind(r#"{"x": 1, "x": {"y": 1, "y": 2}}"#),
        ReadErrorKind::DuplicateKey("y".into())
    );
}

#[test]
fn constants_are_rejected() {
    for name in ["NaN", "Infinity", "-Infinity"] {
        assert_eq!(kind(name), ReadErrorKind::Constant(name.into()));
        assert_eq!(
            kind(&format!("[1, {name}]")),
            ReadErrorKind::Constant(name.into())
        );
    }
    assert_eq!(kind(r#"{"a": NaN}"#), ReadErrorKind::Constant("NaN".into()));
}

#[test]
fn overflow_to_infinity_is_rejected() {
    assert_eq!(kind("1e400"), ReadErrorKind::NonFinite);
    assert_eq!(kind("-1.5e999"), ReadErrorKind::NonFinite);
    assert_eq!(read("1e-400"), Ok(Json::Float(0.0)));
}

#[test]
fn integers_outside_i64_and_u64_are_rejected() {
    assert_eq!(
        kind("18446744073709551616"),
        ReadErrorKind::IntegerOutOfRange
    );
    assert_eq!(
        kind("-9223372036854775809"),
        ReadErrorKind::IntegerOutOfRange
    );
}

#[test]
fn lone_surrogates_are_rejected() {
    assert_eq!(kind(r#""\ud800""#), ReadErrorKind::LoneSurrogate);
    assert_eq!(kind(r#""\udc00""#), ReadErrorKind::LoneSurrogate);
    assert_eq!(kind(r#""\ud800A""#), ReadErrorKind::LoneSurrogate);
    let error = at(r#"["ab\ud800"]"#);
    assert_eq!((error.line, error.column), (1, 5));
}

#[test]
fn a_byte_order_mark_is_rejected() {
    let error = at("\u{feff}{}");
    assert_eq!(error.kind, ReadErrorKind::Bom);
    assert_eq!((error.line, error.column), (1, 1));
}

#[test]
fn extra_data_is_rejected_where_python_finds_it() {
    let error = at("[1] x");
    assert_eq!(error.kind, ReadErrorKind::ExtraData);
    assert_eq!((error.line, error.column), (1, 5));
    assert_eq!(kind("01"), ReadErrorKind::ExtraData);
    assert_eq!(kind("1."), ReadErrorKind::ExtraData);
    assert_eq!(kind("1e"), ReadErrorKind::ExtraData);
    assert_eq!(kind("{} {}"), ReadErrorKind::ExtraData);
}

#[test]
fn control_characters_in_strings_are_rejected() {
    let error = at("{\"é\": \"a\tb\"}");
    assert_eq!(error.kind, ReadErrorKind::ControlCharacter);
    assert_eq!((error.line, error.column), (1, 9));
    assert_eq!(kind("\"a\nb\""), ReadErrorKind::ControlCharacter);
}

#[test]
fn malformed_text_is_a_syntax_error() {
    for text in [
        "",
        " ",
        "-",
        "[1,]",
        r#"{"a": 1,}"#,
        "{'a': 1}",
        r#""abc"#,
        r#""\x""#,
        r#""\u12g4""#,
        "nul",
        "+1",
        ".5",
        "\u{a0}1",
        "[1 2]",
    ] {
        assert!(matches!(kind(text), ReadErrorKind::Syntax(_)), "{text:?}");
    }
    let error = at("[1,\n  ]");
    assert_eq!((error.line, error.column), (2, 3));
}

/// Texts and the message `str()` of the exception gives for `json.loads` with the tools' hooks,
/// as CPython 3.11 prints them.
const PYTHON_MESSAGES: &[(&str, &str)] = &[
    ("", "Expecting value: line 1 column 1 (char 0)"),
    (" ", "Expecting value: line 1 column 2 (char 1)"),
    ("-", "Expecting value: line 1 column 1 (char 0)"),
    ("[1,]", "Expecting value: line 1 column 4 (char 3)"),
    (
        "{\"a\": 1,}",
        "Expecting property name enclosed in double quotes: line 1 column 9 (char 8)",
    ),
    (
        "{'a': 1}",
        "Expecting property name enclosed in double quotes: line 1 column 2 (char 1)",
    ),
    (
        "\"abc",
        "Unterminated string starting at: line 1 column 1 (char 0)",
    ),
    ("\"\\x\"", "Invalid \\escape: line 1 column 2 (char 1)"),
    (
        "\"\\u12g4\"",
        "Invalid \\uXXXX escape: line 1 column 3 (char 2)",
    ),
    ("nul", "Expecting value: line 1 column 1 (char 0)"),
    ("+1", "Expecting value: line 1 column 1 (char 0)"),
    (".5", "Expecting value: line 1 column 1 (char 0)"),
    ("\u{a0}1", "Expecting value: line 1 column 1 (char 0)"),
    ("[1 2]", "Expecting ',' delimiter: line 1 column 4 (char 3)"),
    (
        "{ not json",
        "Expecting property name enclosed in double quotes: line 1 column 3 (char 2)",
    ),
    (
        "{\"a\" 1}",
        "Expecting ':' delimiter: line 1 column 6 (char 5)",
    ),
    (
        "{\"a\": 1 \"b\": 2}",
        "Expecting ',' delimiter: line 1 column 9 (char 8)",
    ),
    ("[1] x", "Extra data: line 1 column 5 (char 4)"),
    (
        "\"a\\",
        "Unterminated string starting at: line 1 column 1 (char 0)",
    ),
    (
        "\"\\u0041",
        "Invalid \\uXXXX escape: line 1 column 3 (char 2)",
    ),
    (
        "\"\\ud800\\uzzzz\"",
        "Invalid \\uXXXX escape: line 1 column 9 (char 8)",
    ),
    (
        "{\"\u{e9}\": \"a\tb\"}",
        "Invalid control character at: line 1 column 9 (char 8)",
    ),
    (
        "\u{feff}{}",
        "Unexpected UTF-8 BOM (decode using utf-8-sig): line 1 column 1 (char 0)",
    ),
    ("[1,\n  ]", "Expecting value: line 2 column 3 (char 6)"),
    ("{\"a\": 1, \"a\": 2}", "duplicate key 'a'"),
    ("{\"it's\": 1, \"it's\": 2}", "duplicate key \"it's\""),
    ("[NaN]", "NaN is not allowed"),
    ("-Infinity", "-Infinity is not allowed"),
];

#[test]
fn errors_read_as_python_words_them() {
    for (text, message) in PYTHON_MESSAGES {
        assert_eq!(at(text).python_message(), *message, "{text:?}");
    }
}

#[test]
fn nesting_is_capped() {
    let deep = |n: usize| format!("{}{}", "[".repeat(n), "]".repeat(n));
    assert!(read(&deep(MAX_DEPTH)).is_ok());
    assert_eq!(kind(&deep(MAX_DEPTH + 1)), ReadErrorKind::TooDeep);
    let objects = format!(
        "{}1{}",
        r#"{"a": "#.repeat(MAX_DEPTH + 1),
        "}".repeat(MAX_DEPTH + 1)
    );
    assert_eq!(kind(&objects), ReadErrorKind::TooDeep);
}

#[test]
fn valid_documents_read_as_python_reads_them() {
    assert_eq!(read("18446744073709551615"), Ok(Json::UInt(u64::MAX)));
    assert_eq!(read("9223372036854775807"), Ok(Json::Int(i64::MAX)));
    assert_eq!(read("-9223372036854775808"), Ok(Json::Int(i64::MIN)));
    assert_eq!(read("-0"), Ok(Json::Int(0)));
    let Ok(Json::Float(zero)) = read("-0.0") else {
        panic!("-0.0 is a float");
    };
    assert!(zero == 0.0 && zero.is_sign_negative());
    assert_eq!(read("1E+2"), Ok(Json::Float(100.0)));
    assert_eq!(read("1.0"), Ok(Json::Float(1.0)));
    assert_eq!(
        read(r#""😀 é\/\b\f\n\r\t\"\\""#),
        Ok(Json::String("\u{1f600} é/\u{8}\u{c}\n\r\t\"\\".into()))
    );
    assert_eq!(
        read(" \t\r\n{\"b\": [true, false, null], \"a\": {}, \"c\": []}\n"),
        Ok(Json::Object(vec![
            (
                "b".into(),
                Json::Array(vec![Json::Bool(true), Json::Bool(false), Json::Null])
            ),
            ("a".into(), Json::Object(vec![])),
            ("c".into(), Json::Array(vec![])),
        ]))
    );
    assert_eq!(read("\"\u{7f}é\""), Ok(Json::String("\u{7f}é".into())));
}

fn manifest() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// The `kind` lines of `tests/fixtures/python.jsonl`.
fn python(kind: &str) -> Vec<Json> {
    let path = manifest().join("tests/fixtures/python.jsonl");
    let text = std::fs::read_to_string(&path).expect("the Python fixture is readable");
    text.lines()
        .map(|line| read(line).expect("the Python fixture is strict JSON"))
        .filter(|entry| str_field(entry, "kind") == kind)
        .collect()
}

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

fn str_field<'j>(value: &'j Json, key: &str) -> &'j str {
    match field(value, key) {
        Json::String(s) => s,
        other => panic!("{key} is not a string: {other:?}"),
    }
}

#[test]
fn floats_are_written_as_python_repr() {
    let lines = python("float");
    assert!(lines.len() >= 20);
    for entry in &lines {
        let text = str_field(entry, "text");
        let value: f64 = text.parse().expect("the float text parses");
        assert_eq!(
            dumps(&Json::Float(value), Style::LINE),
            str_field(entry, "dumps"),
            "{text}"
        );
    }
}

#[test]
fn non_finite_floats_are_written_as_python_writes_them() {
    assert_eq!(dumps(&Json::Float(f64::NAN), Style::LINE), "NaN");
    assert_eq!(dumps(&Json::Float(f64::INFINITY), Style::LINE), "Infinity");
    assert_eq!(
        dumps(&Json::Float(f64::NEG_INFINITY), Style::LINE),
        "-Infinity"
    );
}

#[test]
fn strings_are_escaped_as_python_escapes_them() {
    let s = |text: &str| Json::String(text.into());
    let text = "\u{7f}\u{1f600}\u{0}\u{1f}\u{8}\u{c}\n\r\t\"\\/\u{2028}é";
    assert_eq!(
        dumps(&s(text), Style::DISK_ASCII),
        r#""\u007f\ud83d\ude00\u0000\u001f\b\f\n\r\t\"\\/\u2028\u00e9""#
    );
    assert_eq!(
        dumps(&s(text), Style::LINE),
        "\"\u{7f}\u{1f600}\\u0000\\u001f\\b\\f\\n\\r\\t\\\"\\\\/\u{2028}é\""
    );
}

#[test]
fn layout_follows_python_separators() {
    let value = Json::Object(vec![
        (
            "b".into(),
            Json::Array(vec![Json::Int(1), Json::UInt(u64::MAX)]),
        ),
        ("a".into(), Json::Object(vec![])),
        ("c".into(), Json::Array(vec![])),
        ("é".into(), Json::Null),
        ("Z".into(), Json::Bool(true)),
    ]);
    assert_eq!(
        dumps(&value, Style::LINE),
        r#"{"b": [1, 18446744073709551615], "a": {}, "c": [], "é": null, "Z": true}"#
    );
    assert_eq!(
        dumps(&value, Style::CANONICAL),
        r#"{"Z": true, "a": {}, "b": [1, 18446744073709551615], "c": [], "é": null}"#
    );
    assert_eq!(
        to_file_text(&value, Style::DISK),
        "{\n  \"b\": [\n    1,\n    18446744073709551615\n  ],\n  \"a\": {},\n  \"c\": [],\n  \
         \"é\": null,\n  \"Z\": true\n}\n"
    );
    assert_eq!(dumps(&Json::Array(vec![]), Style::DISK), "[]");
    assert_eq!(dumps(&Json::Object(vec![]), Style::DISK), "{}");
}

/// The line read strictly as its record type and written back through `to_json`.
fn through<T: DeserializeOwned + Serialize>(line: &str) -> Json {
    read(line).expect("the record line is strict JSON");
    let record: T = serde_json::from_str(line).expect("the line is its record type");
    to_json(&record).expect("the record converts")
}

#[test]
fn every_record_type_is_written_as_the_python_tools_write_it() {
    let path = manifest().join("../domain/tests/fixtures/records.jsonl");
    let text = std::fs::read_to_string(&path).expect("the record fixtures are readable");
    let records: Vec<&str> = text.lines().collect();
    let lines = python("dumps");
    assert_eq!(lines.len(), records.len(), "one dumps line per record");
    let mut seen = Vec::new();
    for entry in &lines {
        let Json::Int(number) = *field(entry, "line") else {
            panic!("line is not an integer");
        };
        let number = usize::try_from(number).expect("the line number fits");
        let line = records[number - 1];
        let tool = str_field(entry, "tool");
        let tree = match tool {
            FloorRecord::TOOL => through::<FloorRecord>(line),
            DecisionRecord::TOOL => through::<DecisionRecord>(line),
            RoundRecord::TOOL => through::<RoundRecord>(line),
            FactsRecord::TOOL => through::<FactsRecord>(line),
            LandingFloorRecord::TOOL => through::<LandingFloorRecord>(line),
            other => panic!("line {number}: unknown tool {other}"),
        };
        let disk = if tool == RoundRecord::TOOL {
            Style::DISK_ASCII
        } else {
            Style::DISK
        };
        assert_eq!(
            to_file_text(&tree, disk),
            str_field(entry, "disk"),
            "line {number} disk"
        );
        assert_eq!(
            to_file_text(&tree, Style::DISK_ASCII),
            str_field(entry, "ascii"),
            "line {number} ascii"
        );
        assert_eq!(
            dumps(&tree, Style::CANONICAL),
            str_field(entry, "canonical"),
            "line {number} canonical"
        );
        assert_eq!(dumps(&tree, Style::LINE), line, "line {number} one-line");
        seen.push(tool.to_owned());
    }
    for tool in [
        FloorRecord::TOOL,
        DecisionRecord::TOOL,
        RoundRecord::TOOL,
        FactsRecord::TOOL,
        LandingFloorRecord::TOOL,
    ] {
        assert!(seen.iter().any(|t| t == tool), "no fixture for {tool}");
    }
    let escaped = lines
        .iter()
        .filter(|entry| str_field(entry, "ascii") != str_field(entry, "disk"))
        .count();
    assert!(escaped > 0, "some record has non-ASCII text to escape");
}
