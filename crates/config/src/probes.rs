//! Capability probe definitions: their schema, and the merge of the built-in, machine and
//! repository sets.
//!
//! A definition is one YAML file. The caller finds and reads the files of each source; this module
//! parses each text and merges the sources. Nothing here runs a probe: the regexes, globs and
//! scripts are checked for shape only.

use std::collections::BTreeMap;
use std::fmt;
use std::time::Duration;

use mintswarm_domain::probe::ProbeKind;
use serde_json::{Map, Value};

use crate::{Error, Path, parse_yaml};

/// The timeout of a probe whose definition gives none.
pub const DEFAULT_PROBE_TIMEOUT: Duration = Duration::from_secs(10);

/// The keys of a definition.
const DEFINITION_KEYS: [&str; 4] = ["id", "kind", "detect", "timeout"];
/// The keys that name a detection method, one per `detect` item.
const METHOD_KEYS: [&str; 4] = ["command", "path", "env", "script"];

/// One way of detecting a capability, tried in the order `detect` lists them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DetectMethod {
    /// Run `command` and read the version from its output with `version_regex`.
    Command {
        /// The command line to run.
        command: String,
        /// The regex whose first group is the version.
        version_regex: String,
    },
    /// A file or directory exists at `path`, which may be a glob.
    Path {
        /// The path or glob.
        path: String,
    },
    /// The environment variable `env` is set.
    Env {
        /// The variable's name.
        env: String,
    },
    /// Run `script`, which prints the result as JSON.
    Script {
        /// The script to run.
        script: String,
    },
}

/// A validated probe definition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeDefinition {
    id: String,
    kind: ProbeKind,
    detect: Vec<DetectMethod>,
    timeout: Duration,
}

impl ProbeDefinition {
    /// `id`: the probe's name within its kind, never empty.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// `kind`: what the probe detects.
    pub fn kind(&self) -> ProbeKind {
        self.kind
    }

    /// `detect`: the methods to try, never empty.
    pub fn detect(&self) -> &[DetectMethod] {
        &self.detect
    }

    /// `timeout`, [`DEFAULT_PROBE_TIMEOUT`] when absent.
    pub fn timeout(&self) -> Duration {
        self.timeout
    }
}

/// Parse and validate one probe definition.
///
/// The keys are `id` (a non-empty string, required), `kind` (`toolchain`, `engine`, `tool`,
/// `service` or `harness`, required), `detect` (a non-empty list, required) and `timeout` (a
/// positive number of seconds, default 10). Each `detect` item holds exactly one of `command`
/// (with `version_regex`), `path`, `env` or `script`, each a non-empty string. Any other key is
/// rejected, and so is a duplicate key.
pub fn load_probe_definition(text: &str) -> Result<ProbeDefinition, Error> {
    let value = parse_yaml(text)?;
    let root = Path::root();
    let map = value
        .as_object()
        .ok_or_else(|| Error::schema(&root, "a probe definition must be a mapping"))?;
    if let Some(key) = map
        .keys()
        .find(|key| !DEFINITION_KEYS.contains(&key.as_str()))
    {
        return Err(Error::schema(&root.key(key), "unknown key"));
    }

    let id = required_string(map, &root, "id")?;

    let kind_path = root.key("kind");
    let kind = match map.get("kind") {
        None => return Err(Error::schema(&kind_path, "is missing")),
        Some(value) => value
            .as_str()
            .and_then(ProbeKind::from_name)
            .ok_or_else(|| {
                Error::schema(
                    &kind_path,
                    format!(
                        "must be one of {}, not {value}",
                        ProbeKind::ALL.map(ProbeKind::name).join(", ")
                    ),
                )
            })?,
    };

    let detect_path = root.key("detect");
    let detect = match map.get("detect") {
        None => return Err(Error::schema(&detect_path, "is missing")),
        Some(Value::Array(items)) if !items.is_empty() => items
            .iter()
            .enumerate()
            .map(|(index, item)| detect_method(item, &detect_path.index(index)))
            .collect::<Result<Vec<_>, _>>()?,
        Some(_) => return Err(Error::schema(&detect_path, "must be a non-empty list")),
    };

    let timeout = match map.get("timeout") {
        None => DEFAULT_PROBE_TIMEOUT,
        Some(seconds) => seconds
            .as_f64()
            .filter(|s| s.is_finite() && *s > 0.0)
            .and_then(|s| Duration::try_from_secs_f64(s).ok())
            // A value too small for a nanosecond converts to zero, which is not positive.
            .filter(|limit| !limit.is_zero())
            .ok_or_else(|| {
                Error::schema(&root.key("timeout"), "must be a positive number of seconds")
            })?,
    };

    Ok(ProbeDefinition {
        id,
        kind,
        detect,
        timeout,
    })
}

/// One `detect` item at `path`.
fn detect_method(item: &Value, path: &Path) -> Result<DetectMethod, Error> {
    let map = item
        .as_object()
        .ok_or_else(|| Error::schema(path, "must be a mapping"))?;
    if let Some(key) = map
        .keys()
        .find(|key| *key != "version_regex" && !METHOD_KEYS.contains(&key.as_str()))
    {
        return Err(Error::schema(&path.key(key), "unknown key"));
    }
    let mut methods = METHOD_KEYS.into_iter().filter(|key| map.contains_key(*key));
    let method = methods.next().ok_or_else(|| {
        Error::schema(path, format!("must hold one of {}", METHOD_KEYS.join(", ")))
    })?;
    if let Some(second) = methods.next() {
        return Err(Error::schema(
            &path.key(second),
            format!("a detect item holds one method, and this one already has {method}"),
        ));
    }
    if method != "command" && map.contains_key("version_regex") {
        return Err(Error::schema(
            &path.key("version_regex"),
            "unknown key: version_regex goes only with command",
        ));
    }
    let value = required_string(map, path, method)?;
    Ok(match method {
        "command" => DetectMethod::Command {
            command: value,
            version_regex: required_string(map, path, "version_regex")?,
        },
        "path" => DetectMethod::Path { path: value },
        "env" => DetectMethod::Env { env: value },
        _ => DetectMethod::Script { script: value },
    })
}

/// The non-empty string at `key` of `map`.
fn required_string(map: &Map<String, Value>, path: &Path, key: &str) -> Result<String, Error> {
    match map.get(key) {
        Some(Value::String(text)) if !text.is_empty() => Ok(text.clone()),
        Some(_) => Err(Error::schema(&path.key(key), "must be a non-empty string")),
        None => Err(Error::schema(&path.key(key), "is missing")),
    }
}

/// Where a probe definition comes from, in override order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ProbeOrigin {
    /// Built into the runner.
    Builtin,
    /// The machine's own probes.
    Machine,
    /// The repository's probes.
    Repository,
}

impl ProbeOrigin {
    /// The origin's name: `builtin`, `machine` or `repository`.
    pub fn name(self) -> &'static str {
        match self {
            ProbeOrigin::Builtin => "builtin",
            ProbeOrigin::Machine => "machine",
            ProbeOrigin::Repository => "repository",
        }
    }
}

impl fmt::Display for ProbeOrigin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// Merge the built-in, machine and repository definitions, in that order, by id.
///
/// Each source is a list of `(file name, definition)`. A later source's definition replaces an
/// earlier one with the same id, and the result keeps each id once, with the source it came from.
/// The same id twice within one source is an error at `id` that names both files.
pub fn merge_probe_definitions(
    builtin: &[(String, ProbeDefinition)],
    machine: &[(String, ProbeDefinition)],
    repository: &[(String, ProbeDefinition)],
) -> Result<BTreeMap<String, (ProbeOrigin, ProbeDefinition)>, Error> {
    let mut merged = BTreeMap::new();
    for (origin, source) in [
        (ProbeOrigin::Builtin, builtin),
        (ProbeOrigin::Machine, machine),
        (ProbeOrigin::Repository, repository),
    ] {
        let mut seen: BTreeMap<&str, &str> = BTreeMap::new();
        for (name, definition) in source {
            if let Some(first) = seen.insert(definition.id(), name) {
                return Err(Error::schema(
                    &Path::root().key("id"),
                    format!(
                        "the {origin} probes define `{}` twice, in {first} and {name}",
                        definition.id()
                    ),
                ));
            }
            merged.insert(definition.id().to_owned(), (origin, definition.clone()));
        }
    }
    Ok(merged)
}
