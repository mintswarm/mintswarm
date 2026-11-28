//! The strict JSON reader and the `json.dumps` writer.

use std::path::PathBuf;

use mintswarm_domain::records::{
    DecisionRecord, FactsRecord, FloorRecord, Json, LandingFloorRecord, RoundRecord,
};
use mintswarm_rules::json::{
    MAX_DEPTH, ReadError, ReadErrorKind, Style, dumps, read, to_file_text, to_json,
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
    assert_eq!(kind(r#"{"a": 1, "a": 2, "b" 3}"#), ReadErrorKind::Syntax);
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
fn a_byte_order_mark_is_a_syntax_error() {
    let error = at("\u{feff}{}");
    assert_eq!(error.kind, ReadErrorKind::Syntax);
    assert_eq!((error.line, error.column), (1, 1));
}

#[test]
fn extra_data_is_a_syntax_error_where_python_finds_it() {
    let error = at("[1] x");
    assert_eq!(error.kind, ReadErrorKind::Syntax);
    assert_eq!((error.line, error.column), (1, 5));
    for (text, column) in [("01", 2), ("1.", 2), ("1e", 2), ("{} {}", 4)] {
        let error = at(text);
        assert_eq!(
            (error.kind, error.line, error.column),
            (ReadErrorKind::Syntax, 1, column),
            "{text:?}"
        );
    }
}

#[test]
fn control_characters_in_strings_are_a_syntax_error() {
    let error = at("{\"é\": \"a\tb\"}");
    assert_eq!(error.kind, ReadErrorKind::Syntax);
    assert_eq!((error.line, error.column), (1, 9));
    let error = at("\"a\nb\"");
    assert_eq!(
        (error.kind, error.line, error.column),
        (ReadErrorKind::Syntax, 1, 3)
    );
}

#[test]
fn a_trailing_comma_is_reported_at_the_comma() {
    for (text, line, column) in [
        ("[1,]", 1, 3),
        (r#"{"a": 1,}"#, 1, 8),
        ("[1,\n  ]", 1, 3),
        ("[1,,]", 1, 4),
        ("[,]", 1, 2),
        ("[1,}", 1, 3),
        (r#"{"a":1,]"#, 1, 7),
    ] {
        let error = at(text);
        assert_eq!(
            (error.kind, error.line, error.column),
            (ReadErrorKind::Syntax, line, column),
            "{text:?}"
        );
    }
}

#[test]
fn errors_display_with_a_position_unless_a_hook_finds_them() {
    assert_eq!(at(r#"{"a": 1, "a": 2}"#).to_string(), "duplicate key 'a'");
    assert_eq!(at("[NaN]").to_string(), "NaN is not allowed");
    assert_eq!(
        at(&format!("[{}]", "9".repeat(5000))).to_string(),
        "an integer is out of range"
    );
    assert_eq!(
        at("{ not json").to_string(),
        "not valid JSON: line 1 column 3"
    );
    assert_eq!(
        at(&"[".repeat(MAX_DEPTH + 1)).to_string(),
        format!(
            "nested deeper than {MAX_DEPTH}: line 1 column {}",
            MAX_DEPTH + 1
        )
    );
    assert_eq!(
        at("1e400").to_string(),
        "a number is not finite: line 1 column 1"
    );
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
        assert_eq!(kind(text), ReadErrorKind::Syntax, "{text:?}");
    }
    let error = at("[1 2]");
    assert_eq!((error.line, error.column), (1, 4));
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
