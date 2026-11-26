//! The `schemas` task: `api/schemas/fact.schema.json`, generated from `domain::facts::Fact`.
//!
//! schemars derives the schema with every subschema inlined; this module then reduces it to the
//! keyword subset the Python validator reads (`type`, `enum`, `const`, `required`,
//! `properties`, `additionalProperties: false`, `oneOf` on `kind`, `items`, `minimum`,
//! `pattern`) and adds an `x-otlp` annotation to each kind's branch from [`FactKind::otlp`].

use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

use mintswarm_domain::facts::{Fact, FactKind, Signal};
use schemars::generate::SchemaSettings;
use serde_json::{Map, Value, json};

/// The fact schema's path, relative to the workspace root.
pub const FACT_SCHEMA: &str = "api/schemas/fact.schema.json";

/// The command that regenerates the schemas.
pub const FIX: &str = "cargo xtask schemas";

/// Keywords schemars emits that the schema does not carry: documentation stays in rustdoc.
const STRIPPED: [&str; 6] = [
    "$schema",
    "title",
    "description",
    "format",
    "default",
    "examples",
];

/// Why the task failed.
#[derive(Debug)]
pub enum Error {
    /// The schema file could not be read or written.
    Io(PathBuf, io::Error),
    /// The committed schema differs from the generated one, or is missing.
    Drift(PathBuf),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io(path, error) => write!(f, "{}: {error}", path.display()),
            Error::Drift(path) => write!(
                f,
                "{} differs from the schema generated from the Rust types; run `{FIX}` and \
                 commit the result",
                path.display()
            ),
        }
    }
}

impl std::error::Error for Error {}

/// The fact schema's text: pretty-printed JSON with sorted keys and a trailing newline.
pub fn fact_schema() -> String {
    let generator = SchemaSettings::draft2020_12()
        .with(|settings| {
            settings.inline_subschemas = true;
            settings.meta_schema = None;
        })
        .into_generator();
    let mut schema = generator.into_root_schema_for::<Fact>().to_value();
    simplify(&mut schema);
    annotate(&mut schema);
    let mut text = serde_json::to_string_pretty(&schema)
        .expect("a JSON value with string keys always serialises");
    text.push('\n');
    text
}

/// Writes the fact schema under `root`.
pub fn write(root: &Path) -> Result<(), Error> {
    let path = root.join(FACT_SCHEMA);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| Error::Io(parent.to_path_buf(), e))?;
    }
    std::fs::write(&path, fact_schema()).map_err(|e| Error::Io(path, e))
}

/// Compares the fact schema under `root` with the generated one.
pub fn check(root: &Path) -> Result<(), Error> {
    let path = root.join(FACT_SCHEMA);
    match std::fs::read_to_string(&path) {
        Ok(text) if text == fact_schema() => Ok(()),
        Ok(_) => Err(Error::Drift(PathBuf::from(FACT_SCHEMA))),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            Err(Error::Drift(PathBuf::from(FACT_SCHEMA)))
        }
        Err(e) => Err(Error::Io(path, e)),
    }
}

fn is_null_schema(schema: &Value) -> bool {
    schema.get("type") == Some(&Value::from("null"))
}

/// Reduces a schema and its subschemas to the keyword subset, bottom up.
fn simplify(schema: &mut Value) {
    let Value::Object(object) = schema else {
        return;
    };
    for key in ["items", "additionalProperties"] {
        if let Some(sub) = object.get_mut(key) {
            simplify(sub);
        }
    }
    if let Some(Value::Object(properties)) = object.get_mut("properties") {
        properties.values_mut().for_each(simplify);
    }
    for key in ["oneOf", "anyOf", "allOf"] {
        if let Some(Value::Array(branches)) = object.get_mut(key) {
            branches.iter_mut().for_each(simplify);
        }
    }
    for key in STRIPPED {
        object.remove(key);
    }
    // An optional field is absent rather than null: drop the null alternatives schemars adds.
    if let Some(Value::Array(branches)) = object.get("anyOf") {
        let rest: Vec<&Value> = branches.iter().filter(|b| !is_null_schema(b)).collect();
        if let [Value::Object(only)] = rest.as_slice()
            && rest.len() < branches.len()
        {
            let only = only.clone();
            object.remove("anyOf");
            object.extend(only);
        }
    }
    if let Some(Value::Array(types)) = object.get_mut("type") {
        types.retain(|t| t != "null");
        if let [only] = types.as_slice() {
            let only = only.clone();
            object.insert("type".into(), only);
        }
    }
    if let Some(Value::Array(values)) = object.get_mut("enum") {
        values.retain(|v| !v.is_null());
    }
    // Documented unit variants come out as a `oneOf` of `const`s: one `enum` says the same.
    if let Some(Value::Array(branches)) = object.get("oneOf")
        && !branches.is_empty()
        && branches.iter().all(|b| {
            b.as_object().is_some_and(|b| {
                b.get("const").is_some_and(Value::is_string)
                    && b.keys().all(|k| k == "const" || k == "type")
            })
        })
    {
        let values: Vec<Value> = branches.iter().map(|b| b["const"].clone()).collect();
        object.remove("oneOf");
        object.insert("type".into(), Value::from("string"));
        object.insert("enum".into(), Value::Array(values));
    }
}

/// Adds `x-otlp` to each kind's branch of the root `oneOf`.
fn annotate(schema: &mut Value) {
    let Some(Value::Array(branches)) = schema.get_mut("oneOf") else {
        return;
    };
    for branch in branches {
        let tag = branch["properties"]["kind"]["const"].clone();
        let Some(kind) = FactKind::ALL.iter().find(|k| tag == k.tag()) else {
            continue;
        };
        let otlp = kind.otlp();
        let mut annotation = Map::new();
        annotation.insert("signal".into(), Value::from(otlp.signal.as_str()));
        annotation.insert("name".into(), Value::from(otlp.name));
        annotation.insert(
            "attaches_to".into(),
            otlp.attaches_to.iter().map(|a| a.as_str()).collect(),
        );
        if otlp.signal == Signal::Log {
            annotation.insert("natural_key".into(), json!(otlp.natural_key));
        }
        if let Some(object) = branch.as_object_mut() {
            object.insert("x-otlp".into(), Value::Object(annotation));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The keywords a schema position may carry.
    const SUBSET: [&str; 9] = [
        "type",
        "enum",
        "const",
        "required",
        "properties",
        "additionalProperties",
        "items",
        "minimum",
        "pattern",
    ];

    fn schema() -> Value {
        serde_json::from_str(&fact_schema()).unwrap()
    }

    /// Walks every schema position below the root, collecting what breaks the subset.
    fn walk(schema: &Value, at: &str, problems: &mut Vec<String>) {
        let Some(object) = schema.as_object() else {
            problems.push(format!("{at}: not an object schema"));
            return;
        };
        for key in object.keys() {
            if !SUBSET.contains(&key.as_str()) {
                problems.push(format!("{at}: keyword `{key}`"));
            }
        }
        match object.get("additionalProperties") {
            Some(Value::Bool(false)) => {}
            Some(other) => problems.push(format!("{at}: additionalProperties {other}")),
            None if object.get("type") == Some(&Value::from("object")) => {
                problems.push(format!("{at}: object without additionalProperties"));
            }
            None => {}
        }
        if let Some(properties) = object.get("properties") {
            for (name, sub) in properties.as_object().into_iter().flatten() {
                walk(sub, &format!("{at}/properties/{name}"), problems);
            }
        }
        if let Some(items) = object.get("items") {
            walk(items, &format!("{at}/items"), problems);
        }
    }

    #[test]
    fn uses_only_the_keyword_subset() {
        let schema = schema();
        let root = schema.as_object().unwrap();
        assert_eq!(root.keys().collect::<Vec<_>>(), ["oneOf"]);
        let mut problems = Vec::new();
        for (index, branch) in schema["oneOf"].as_array().unwrap().iter().enumerate() {
            let mut branch = branch.clone();
            branch.as_object_mut().unwrap().remove("x-otlp");
            walk(&branch, &format!("/oneOf/{index}"), &mut problems);
        }
        assert!(problems.is_empty(), "{problems:#?}");
    }

    #[test]
    fn one_branch_per_kind_with_x_otlp() {
        let schema = schema();
        let branches = schema["oneOf"].as_array().unwrap();
        assert_eq!(branches.len(), FactKind::ALL.len());
        for (branch, kind) in branches.iter().zip(FactKind::ALL) {
            assert_eq!(branch["properties"]["kind"]["const"], kind.tag());
            assert_eq!(branch["required"][0], "kind");
            assert_eq!(branch["additionalProperties"], false);
            let otlp = &branch["x-otlp"];
            assert_eq!(otlp["signal"], kind.otlp().signal.as_str());
            assert_eq!(otlp["name"], kind.otlp().name);
            assert!(!otlp["attaches_to"].as_array().unwrap().is_empty());
            let expected = match kind.otlp().signal {
                Signal::Log => json!(kind.otlp().natural_key),
                Signal::Span => Value::Null,
            };
            assert_eq!(otlp["natural_key"], expected);
        }
    }

    #[test]
    fn optional_fields_are_absent_not_null() {
        let text = fact_schema();
        assert!(!text.contains("\"null\""));
        let schema = schema();
        let log = &schema["oneOf"][11];
        assert_eq!(log["properties"]["kind"]["const"], "intervention");
        let required = log["required"].as_array().unwrap();
        assert!(!required.contains(&json!("concerns")));
        assert_eq!(log["properties"]["concerns"]["type"], "object");
    }

    #[test]
    fn generation_is_reproducible() {
        assert_eq!(fact_schema(), fact_schema());
        assert!(fact_schema().ends_with("}\n"));
    }

    #[test]
    fn check_reports_drift() {
        let root = tempfile::tempdir().unwrap();
        assert!(matches!(check(root.path()), Err(Error::Drift(_))));
        write(root.path()).unwrap();
        check(root.path()).unwrap();
        let path = root.path().join(FACT_SCHEMA);
        let mut text = std::fs::read_to_string(&path).unwrap();
        text.push(' ');
        std::fs::write(&path, text).unwrap();
        assert!(matches!(check(root.path()), Err(Error::Drift(_))));
        let message = check(root.path()).unwrap_err().to_string();
        assert!(message.contains(FACT_SCHEMA) && message.contains(FIX));
    }
}
