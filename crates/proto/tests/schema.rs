//! The generated `StageResult` schema: the committed file matches the
//! descriptors, it uses only the keyword subset, a fully populated message
//! and the result-block fixtures fit it exactly, and the walker refuses the
//! field kinds the subset cannot describe.
//!
//! Run with `MINTSWARM_WRITE_SCHEMAS=1` to rewrite the committed file.

#![cfg(test)]

mod common;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use common::full_stage_result;
use mintswarm_proto::prost_types::field_descriptor_proto::{Label, Type};
use mintswarm_proto::prost_types::{
    DescriptorProto, EnumDescriptorProto, EnumValueDescriptorProto, FieldDescriptorProto,
    FileDescriptorProto, MessageOptions, OneofDescriptorProto,
};
use mintswarm_proto::runner::v1::{
    QuestionKind, QuestionReferenceKind, QuestionShape, StageStatus,
};
use mintswarm_proto::schema::{self, Error, STAGE_RESULT_SCHEMA};
use prost_reflect::DescriptorPool;
use serde_json::Value;

/// The keywords the generated schema may use.
const KEYWORDS: [&str; 5] = [
    "type",
    "enum",
    "properties",
    "additionalProperties",
    "items",
];

/// The committed schema's path.
fn committed_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(STAGE_RESULT_SCHEMA)
}

/// The generated schema's text.
fn generated() -> String {
    schema::stage_result_schema().expect("the schema generates")
}

/// The generated schema, parsed.
fn generated_value() -> Value {
    serde_json::from_str(&generated()).expect("the schema is JSON")
}

/// The result-block fixtures, by file name.
fn fixtures() -> Vec<(String, Value)> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/result-blocks");
    let mut fixtures: Vec<(String, Value)> = std::fs::read_dir(&dir)
        .expect("fixtures directory")
        .map(|entry| {
            let path = entry.expect("directory entry").path();
            let text = std::fs::read_to_string(&path).expect("fixture reads");
            let name = path
                .file_name()
                .expect("file name")
                .to_string_lossy()
                .into_owned();
            (name, serde_json::from_str(&text).expect("fixture is JSON"))
        })
        .collect();
    fixtures.sort_by(|a, b| a.0.cmp(&b.0));
    assert!(fixtures.len() >= 3, "read {} fixtures", fixtures.len());
    fixtures
}

#[test]
fn committed_schema_matches_the_descriptors() {
    let path = committed_path();
    let text = generated();
    if std::env::var_os("MINTSWARM_WRITE_SCHEMAS").is_some_and(|value| value == "1") {
        std::fs::create_dir_all(path.parent().expect("parent directory")).expect("creates");
        std::fs::write(&path, &text).expect("writes the schema");
        return;
    }
    let committed = std::fs::read_to_string(&path).unwrap_or_default();
    assert!(
        committed == text,
        "`{STAGE_RESULT_SCHEMA}` differs from the descriptors; rewrite it with \
         `MINTSWARM_WRITE_SCHEMAS=1 cargo test -p mintswarm-proto --test schema`"
    );
}

#[test]
fn generation_is_reproducible() {
    assert_eq!(generated(), generated());
}

/// Checks that `schema` uses only [`KEYWORDS`], never allows additional
/// properties and lists no zero enum value.
fn check_subset(schema: &Value, at: &str) {
    let object = schema
        .as_object()
        .unwrap_or_else(|| panic!("{at} is an object"));
    for (key, value) in object {
        assert!(KEYWORDS.contains(&key.as_str()), "{at} uses {key}");
        match key.as_str() {
            "additionalProperties" => assert_eq!(value, &Value::Bool(false), "{at}"),
            "enum" => {
                let names = value.as_array().unwrap_or_else(|| panic!("{at}.enum"));
                assert!(!names.is_empty(), "{at}.enum is empty");
                for name in names {
                    let name = name.as_str().unwrap_or_else(|| panic!("{at}.enum"));
                    assert!(!name.ends_with("UNSPECIFIED"), "{at} lists {name}");
                }
            }
            "properties" => {
                let properties = value.as_object().unwrap_or_else(|| panic!("{at}"));
                for (name, property) in properties {
                    check_subset(property, &format!("{at}.{name}"));
                }
            }
            "items" => check_subset(value, &format!("{at}[]")),
            _ => {}
        }
    }
}

#[test]
fn uses_only_the_keyword_subset() {
    check_subset(&generated_value(), "$");
}

/// Checks that `value`'s keys at every object are exactly the schema's
/// properties, and that each enum value is in its list.
fn check_exact(value: &Value, schema: &Value, at: &str) {
    if let Some(names) = schema.get("enum").and_then(Value::as_array) {
        assert!(names.contains(value), "{at}: {value} is not in {names:?}");
    }
    match schema["type"].as_str() {
        Some("object") => {
            let object = value
                .as_object()
                .unwrap_or_else(|| panic!("{at} is an object"));
            let properties = schema["properties"].as_object().expect("properties");
            let keys: BTreeSet<&String> = object.keys().collect();
            let allowed: BTreeSet<&String> = properties.keys().collect();
            assert_eq!(keys, allowed, "{at}");
            for (key, field) in object {
                check_exact(field, &properties[key], &format!("{at}.{key}"));
            }
        }
        Some("array") => {
            let items = value
                .as_array()
                .unwrap_or_else(|| panic!("{at} is an array"));
            assert!(!items.is_empty(), "{at} is empty");
            for (index, item) in items.iter().enumerate() {
                check_exact(item, &schema["items"], &format!("{at}[{index}]"));
            }
        }
        _ => {}
    }
}

/// The names of the generated Rust enum's non-zero values.
fn rust_names<E: TryFrom<i32>>(name: fn(&E) -> &'static str) -> Vec<Value> {
    (1..)
        .map_while(|number| E::try_from(number).ok())
        .map(|value| Value::from(name(&value)))
        .collect()
}

#[test]
fn full_stage_result_matches_the_schema() {
    let schema = generated_value();
    let json =
        mintswarm_proto::json::serialize(&full_stage_result(), serde_json::value::Serializer)
            .expect("serialises");
    check_exact(&json, &schema, "$");

    let question = &schema["properties"]["questions"]["items"]["properties"];
    let reference = &question["references"]["items"]["properties"];
    let lists = [
        (
            "StageStatus",
            &schema["properties"]["status"]["enum"],
            rust_names(StageStatus::as_str_name),
        ),
        (
            "QuestionKind",
            &question["kind"]["enum"],
            rust_names(QuestionKind::as_str_name),
        ),
        (
            "QuestionShape",
            &question["shape"]["enum"],
            rust_names(QuestionShape::as_str_name),
        ),
        (
            "QuestionReferenceKind",
            &reference["kind"]["enum"],
            rust_names(QuestionReferenceKind::as_str_name),
        ),
    ];
    for (name, list, expected) in lists {
        assert_eq!(list.as_array(), Some(&expected), "{name}");
    }
}

/// Collects into `problems` where `value` breaks `schema`, read through the
/// keyword subset.
fn validate(value: &Value, schema: &Value, at: &str, problems: &mut Vec<String>) {
    let fits = match schema["type"].as_str() {
        Some("object") => value.is_object(),
        Some("array") => value.is_array(),
        Some("string") => value.is_string(),
        Some("boolean") => value.is_boolean(),
        other => {
            problems.push(format!("{at}: unknown type {other:?}"));
            return;
        }
    };
    if !fits {
        problems.push(format!("{at}: {value} is not {}", schema["type"]));
        return;
    }
    if let Some(names) = schema.get("enum").and_then(Value::as_array)
        && !names.contains(value)
    {
        problems.push(format!("{at}: {value} is not in the enum"));
    }
    if let Some(object) = value.as_object() {
        let closed = schema.get("additionalProperties") == Some(&Value::Bool(false));
        for (key, field) in object {
            match schema["properties"].get(key) {
                Some(property) => validate(field, property, &format!("{at}.{key}"), problems),
                None if closed => problems.push(format!("{at}: unknown key {key}")),
                None => {}
            }
        }
    }
    if let Some(items) = value.as_array() {
        for (index, item) in items.iter().enumerate() {
            validate(item, &schema["items"], &format!("{at}[{index}]"), problems);
        }
    }
}

/// The problems `value` has against `schema`.
fn problems(value: &Value, schema: &Value) -> Vec<String> {
    let mut problems = Vec::new();
    validate(value, schema, "$", &mut problems);
    problems
}

#[test]
fn result_block_fixtures_validate() {
    let schema = generated_value();
    for (name, fixture) in fixtures() {
        assert_eq!(problems(&fixture, &schema), Vec::<String>::new(), "{name}");
    }
}

#[test]
fn an_unknown_key_fails_validation() {
    let schema = generated_value();
    for (name, fixture) in fixtures() {
        let mut copy = fixture;
        copy["unknownKey"] = Value::Bool(true);
        assert!(!problems(&copy, &schema).is_empty(), "{name}");
    }

    let (_, mut nested) = fixtures()
        .into_iter()
        .find(|(name, _)| name == "needs-input.json")
        .expect("needs-input.json");
    nested["questions"][0]["unknownKey"] = Value::Bool(true);
    assert!(!problems(&nested, &schema).is_empty());
}

/// A change that breaks a fixture.
type Mutation = fn(&mut Value);

#[test]
fn a_wrong_type_or_enum_name_fails_validation() {
    let schema = generated_value();
    let (_, fixture) = fixtures()
        .into_iter()
        .find(|(name, _)| name == "needs-input.json")
        .expect("needs-input.json");
    let mutations: [(&str, Mutation); 5] = [
        ("a string for an array", |value| {
            value["outputs"] = Value::from("x");
        }),
        ("an unknown enum name", |value| {
            value["status"] = Value::from("STAGE_STATUS_DONE");
        }),
        ("an object for a string", |value| {
            value["artefact"] = serde_json::json!({});
        }),
        ("a number for a string in an array", |value| {
            value["outputs"][0] = Value::from(1);
        }),
        ("an unknown nested enum name", |value| {
            value["questions"][0]["kind"] = Value::from("QUESTION_KIND_MAYBE");
        }),
    ];
    for (name, mutate) in mutations {
        let mut copy = fixture.clone();
        mutate(&mut copy);
        assert!(!problems(&copy, &schema).is_empty(), "{name}");
    }
}

/// A field of `kind`, optionally of the named message type and in a oneof.
fn field(
    name: &str,
    kind: Type,
    type_name: Option<&str>,
    oneof: Option<i32>,
) -> FieldDescriptorProto {
    FieldDescriptorProto {
        name: Some(name.to_owned()),
        number: Some(1),
        label: Some(Label::Optional.into()),
        r#type: Some(kind.into()),
        type_name: type_name.map(str::to_owned),
        oneof_index: oneof,
        ..FieldDescriptorProto::default()
    }
}

/// A message named `name` with `fields` and `oneofs`.
fn message(name: &str, fields: Vec<FieldDescriptorProto>, oneofs: &[&str]) -> DescriptorProto {
    DescriptorProto {
        name: Some(name.to_owned()),
        field: fields,
        oneof_decl: oneofs
            .iter()
            .map(|oneof| OneofDescriptorProto {
                name: Some((*oneof).to_owned()),
                ..OneofDescriptorProto::default()
            })
            .collect(),
        ..DescriptorProto::default()
    }
}

/// A message with one map field `a` from string to string.
fn map_message() -> DescriptorProto {
    let mut a = field("a", Type::Message, Some(".test.v1.WithMap.AEntry"), None);
    a.label = Some(Label::Repeated.into());
    let mut value = field("value", Type::String, None, None);
    value.number = Some(2);
    let mut entry = message(
        "AEntry",
        vec![field("key", Type::String, None, None), value],
        &[],
    );
    entry.options = Some(MessageOptions {
        map_entry: Some(true),
        ..MessageOptions::default()
    });
    let mut with_map = message("WithMap", vec![a], &[]);
    with_map.nested_type = vec![entry];
    with_map
}

/// An enum with a zero and a negative value.
fn negative_enum() -> EnumDescriptorProto {
    let value = |name: &str, number| EnumValueDescriptorProto {
        name: Some(name.to_owned()),
        number: Some(number),
        ..EnumValueDescriptorProto::default()
    };
    EnumDescriptorProto {
        name: Some("Signed".to_owned()),
        value: vec![value("SIGNED_UNSPECIFIED", 0), value("SIGNED_NEGATIVE", -1)],
        ..EnumDescriptorProto::default()
    }
}

/// A minimal stand-in for `google/protobuf/timestamp.proto`.
fn timestamp_file() -> FileDescriptorProto {
    let mut nanos = field("nanos", Type::Int32, None, None);
    nanos.number = Some(2);
    FileDescriptorProto {
        name: Some("google/protobuf/timestamp.proto".to_owned()),
        package: Some("google.protobuf".to_owned()),
        syntax: Some("proto3".to_owned()),
        message_type: vec![message(
            "Timestamp",
            vec![field("seconds", Type::Int64, None, None), nanos],
            &[],
        )],
        ..FileDescriptorProto::default()
    }
}

/// The generator's result for the message `name` of a hand-made pool.
fn generate(name: &str) -> Result<Value, Error> {
    let mut second = field("b", Type::Bool, None, Some(0));
    second.number = Some(2);
    let mut optional = field("a", Type::String, None, Some(0));
    optional.proto3_optional = Some(true);
    let file = FileDescriptorProto {
        name: Some("test/v1/kinds.proto".to_owned()),
        package: Some("test.v1".to_owned()),
        syntax: Some("proto3".to_owned()),
        dependency: vec!["google/protobuf/timestamp.proto".to_owned()],
        enum_type: vec![negative_enum()],
        message_type: vec![
            message("WithInt32", vec![field("a", Type::Int32, None, None)], &[]),
            message(
                "WithUint32",
                vec![field("a", Type::Uint32, None, None)],
                &[],
            ),
            message("WithInt64", vec![field("a", Type::Int64, None, None)], &[]),
            message(
                "WithUint64",
                vec![field("a", Type::Uint64, None, None)],
                &[],
            ),
            message("WithFloat", vec![field("a", Type::Float, None, None)], &[]),
            message(
                "WithDouble",
                vec![field("a", Type::Double, None, None)],
                &[],
            ),
            message("WithBytes", vec![field("a", Type::Bytes, None, None)], &[]),
            message(
                "WithOneof",
                vec![field("a", Type::String, None, Some(0)), second],
                &["choice"],
            ),
            message(
                "Recursive",
                vec![field(
                    "next",
                    Type::Message,
                    Some(".test.v1.Recursive"),
                    None,
                )],
                &[],
            ),
            message("WithOptional", vec![optional], &["_a"]),
            map_message(),
            message(
                "WithTimestamp",
                vec![field(
                    "a",
                    Type::Message,
                    Some(".google.protobuf.Timestamp"),
                    None,
                )],
                &[],
            ),
            message(
                "WithNegativeEnum",
                vec![field("a", Type::Enum, Some(".test.v1.Signed"), None)],
                &[],
            ),
        ],
        ..FileDescriptorProto::default()
    };
    let mut pool = DescriptorPool::new();
    pool.add_file_descriptor_proto(timestamp_file())
        .expect("the timestamp file builds");
    pool.add_file_descriptor_proto(file)
        .expect("the test file builds");
    let descriptor = pool
        .get_message_by_name(&format!("test.v1.{name}"))
        .expect("the message exists");
    schema::message_schema(&descriptor)
}

#[test]
fn unsupported_kinds_are_refused() {
    let refused = [
        ("WithInt32", "int32"),
        ("WithUint32", "uint32"),
        ("WithInt64", "int64"),
        ("WithUint64", "uint64"),
        ("WithFloat", "float"),
        ("WithDouble", "double"),
        ("WithBytes", "bytes"),
        ("WithOneof", "oneof"),
        ("WithMap", "map"),
        ("WithTimestamp", "well-known type"),
        ("WithNegativeEnum", "negative enum value"),
    ];
    for (name, expected) in refused {
        match generate(name) {
            Err(Error::UnsupportedKind { kind, .. }) => assert_eq!(kind, expected, "{name}"),
            other => panic!("{name}: {other:?}"),
        }
    }
    assert!(matches!(
        generate("Recursive"),
        Err(Error::Recursion { .. })
    ));
    let optional = generate("WithOptional").expect("a proto3 optional field maps");
    assert_eq!(optional["properties"]["a"]["type"], "string");
}
