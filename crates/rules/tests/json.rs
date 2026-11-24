//! The strict JSON reader and the `json.dumps` writer.

use mintswarm_domain::records::Json;
use mintswarm_rules::json::{MAX_DEPTH, ReadError, ReadErrorKind, read};

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
        assert_eq!(kind(text), ReadErrorKind::Syntax, "{text:?}");
    }
    let error = at("[1,\n  ]");
    assert_eq!((error.line, error.column), (2, 3));
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
