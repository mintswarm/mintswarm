//! The control plane's settings, `mintswarm.yaml`: the address `mintswarm-server serve` listens
//! on, the database it keeps its state in, and the projects it serves.
//!
//! A fixed set of environment variables overrides keys of the file, each also in a `_FILE` form
//! that names a file holding the value. The database URL and password are secrets: neither the
//! loaded settings nor any error shows them.
//!
//! The caller finds and reads the file; this module parses its text.

use std::ffi::OsString;
use std::fmt;
use std::net::SocketAddr;

use mintswarm_domain::ids::ProjectId;
use serde_json::{Map, Value};

use crate::{Error, Path, parse_yaml};

/// The variable that overrides `listen`.
pub const LISTEN_ENV: &str = "MINTSWARM_LISTEN";
/// The variable that overrides `database.url`.
pub const DATABASE_URL_ENV: &str = "MINTSWARM_DATABASE_URL";
/// The variable that overrides `database.password`.
pub const DATABASE_PASSWORD_ENV: &str = "MINTSWARM_DATABASE_PASSWORD";
/// The suffix of the form of an override variable that names a file holding the value.
pub const FILE_SUFFIX: &str = "_FILE";

/// A text that is never shown: its `Debug` gives `Secret(<redacted>)`, and it has no `Display`.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    /// A secret holding `text`.
    pub fn new(text: impl Into<String>) -> Self {
        Self(text.into())
    }

    /// The text, for the one place that uses it.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(<redacted>)")
    }
}

/// The validated `mintswarm.yaml`, with the overrides applied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaneConfig {
    /// `listen`: the address `serve` listens on.
    pub listen: SocketAddr,
    /// `database`: where the plane keeps its state.
    pub database: DatabaseSettings,
    /// `projects`: the projects the plane serves, in the file's order.
    pub projects: Vec<ProjectEntry>,
}

/// `database`: the Postgres connection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DatabaseSettings {
    /// `database.url`: a Postgres connection string, URL or key-value form.
    pub url: Secret,
    /// `database.password`: replaces the URL's password when given.
    pub password: Option<Secret>,
}

/// The code host of a project.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Host {
    /// `github`.
    Github,
}

impl Host {
    /// The host's name in the config and in the `projects` table.
    pub fn name(self) -> &'static str {
        match self {
            Host::Github => "github",
        }
    }
}

/// One entry of `projects`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectEntry {
    /// `project_id`: the project's id, unique in the file.
    pub project_id: ProjectId,
    /// `repo`: the repository as `owner/name`.
    pub repo: String,
    /// `host`: the code host, `github` when absent.
    pub host: Host,
    /// `installation_id`: the GitHub App installation, a positive integer within Postgres's
    /// `bigint`.
    pub installation_id: i64,
    /// `weight`: the project's share of the work, a positive integer within Postgres's
    /// `integer`, 1 when absent.
    pub weight: i32,
}

/// An override's value and the variable it came from.
struct Override {
    value: String,
    source: String,
}

/// Parse and validate `mintswarm.yaml`, applying the environment overrides that `lookup` gives.
///
/// The keys are `listen` (an address and port, such as `127.0.0.1:8080`, required),
/// `database`, a mapping of `url` (a non-empty Postgres connection string, required) and
/// `password` (a non-empty string, optional), and `projects`, a sequence of mappings of
/// `project_id` (a non-empty string, unique), `repo` (`owner/name`), `host` (`github`, the
/// default), `installation_id` (a positive integer) and `weight` (a positive integer, default
/// 1). Any other key is rejected. An empty text holds no keys.
///
/// [`LISTEN_ENV`], [`DATABASE_URL_ENV`] and [`DATABASE_PASSWORD_ENV`] override `listen`,
/// `database.url` and `database.password`. Each may instead be given with [`FILE_SUFFIX`], as
/// the path of a file that holds the value, with one trailing newline removed. Required keys
/// are checked after the overrides, so a value given only by an override is enough.
///
/// An override that cannot be used is [`Error::Schema`] with the variable's name as its path:
/// both forms of one variable set, an empty or non-UTF-8 value, a file that cannot be read or is
/// empty, and a [`LISTEN_ENV`] value that is not an address. No error quotes a value, only key
/// and variable names, so neither the database URL nor the password can appear in one.
pub fn load_plane_config(
    text: &str,
    lookup: impl Fn(&str) -> Option<OsString>,
) -> Result<PlaneConfig, Error> {
    let value = if text.trim().is_empty() {
        Value::Null
    } else {
        parse_yaml(text).map_err(redact_database)?
    };
    let root = Path::root();
    let empty = Map::new();
    let map = match &value {
        Value::Object(map) => map,
        // An empty file, or one whose document is null, holds no keys.
        Value::Null => &empty,
        _ => return Err(Error::schema(&root, "mintswarm.yaml must be a mapping")),
    };
    if let Some(key) = map
        .keys()
        .find(|key| !["listen", "database", "projects"].contains(&key.as_str()))
    {
        return Err(Error::schema(&root.key(key), "unknown key"));
    }
    let database_path = root.key("database");
    let database = match map.get("database") {
        None | Some(Value::Null) => &empty,
        Some(Value::Object(database)) => database,
        Some(_) => return Err(Error::schema(&database_path, "must be a mapping")),
    };
    if let Some(key) = database
        .keys()
        .find(|key| !["url", "password"].contains(&key.as_str()))
    {
        return Err(Error::schema(&database_path.key(key), "unknown key"));
    }

    let listen_override = read_override(LISTEN_ENV, &lookup)?;
    let url_override = read_override(DATABASE_URL_ENV, &lookup)?;
    let password_override = read_override(DATABASE_PASSWORD_ENV, &lookup)?;

    let listen = match listen_override {
        Some(given) => given.value.parse().map_err(|_| Error::Schema {
            path: given.source,
            message: "must be an address and port, such as 127.0.0.1:8080".to_owned(),
        })?,
        None => listen(map.get("listen"), &root.key("listen"))?,
    };
    let url = match url_override {
        Some(given) => Secret(given.value),
        None => match database.get("url") {
            Some(Value::String(url)) if !url.is_empty() => Secret(url.clone()),
            Some(_) => {
                return Err(Error::schema(
                    &database_path.key("url"),
                    "must be a non-empty string",
                ));
            }
            None => return Err(Error::schema(&database_path.key("url"), "is missing")),
        },
    };
    let password = match password_override {
        Some(given) => Some(Secret(given.value)),
        None => match database.get("password") {
            None => None,
            Some(Value::String(password)) if !password.is_empty() => Some(Secret(password.clone())),
            Some(_) => {
                return Err(Error::schema(
                    &database_path.key("password"),
                    "must be a non-empty string",
                ));
            }
        },
    };
    let projects = match map.get("projects") {
        None | Some(Value::Null) => Vec::new(),
        Some(value) => projects(value, &root.key("projects"))?,
    };
    Ok(PlaneConfig {
        listen,
        database: DatabaseSettings { url, password },
        projects,
    })
}

/// A YAML error under `database` with its message replaced, since a message may quote the
/// scalar it concerns.
fn redact_database(error: Error) -> Error {
    match error {
        Error::Yaml { path, .. } if path == "database" || path.starts_with("database.") => {
            Error::Yaml {
                path,
                message: "is not a value YAML converts; the value is not shown".to_owned(),
            }
        }
        error => error,
    }
}

/// The value of the variable `name`, or of the file `<name>_FILE` names, if either is set.
fn read_override(
    name: &str,
    lookup: &impl Fn(&str) -> Option<OsString>,
) -> Result<Option<Override>, Error> {
    let file_name = format!("{name}{FILE_SUFFIX}");
    let schema = |path: &str, message: &str| Error::Schema {
        path: path.to_owned(),
        message: message.to_owned(),
    };
    match (lookup(name), lookup(&file_name)) {
        (None, None) => Ok(None),
        (Some(_), Some(_)) => Err(schema(
            name,
            &format!("is set together with {file_name}; set only one of them"),
        )),
        (Some(value), None) => {
            let value = value
                .into_string()
                .map_err(|_| schema(name, "is not UTF-8"))?;
            if value.is_empty() {
                return Err(schema(name, "is empty"));
            }
            Ok(Some(Override {
                value,
                source: name.to_owned(),
            }))
        }
        (None, Some(path)) => {
            if path.is_empty() {
                return Err(schema(&file_name, "is empty"));
            }
            let bytes = std::fs::read(&path)
                .map_err(|_| schema(&file_name, "names a file that cannot be read"))?;
            let mut value = String::from_utf8(bytes)
                .map_err(|_| schema(&file_name, "names a file that is not UTF-8"))?;
            if value.ends_with('\n') {
                value.pop();
            }
            if value.is_empty() {
                return Err(schema(&file_name, "names an empty file"));
            }
            Ok(Some(Override {
                value,
                source: file_name,
            }))
        }
    }
}

/// `listen` from the file.
fn listen(value: Option<&Value>, path: &Path) -> Result<SocketAddr, Error> {
    match value {
        Some(Value::String(text)) => text.parse().map_err(|_| {
            Error::schema(path, "must be an address and port, such as 127.0.0.1:8080")
        }),
        Some(_) => Err(Error::schema(
            path,
            "must be an address and port, such as 127.0.0.1:8080",
        )),
        None => Err(Error::schema(path, "is missing")),
    }
}

/// The `projects` sequence at `path`.
fn projects(value: &Value, path: &Path) -> Result<Vec<ProjectEntry>, Error> {
    let items = value
        .as_array()
        .ok_or_else(|| Error::schema(path, "must be a sequence"))?;
    let mut entries: Vec<ProjectEntry> = Vec::with_capacity(items.len());
    for (index, item) in items.iter().enumerate() {
        let entry = project(item, &path.index(index))?;
        if entries
            .iter()
            .any(|earlier| earlier.project_id == entry.project_id)
        {
            return Err(Error::schema(
                &path.index(index).key("project_id"),
                "is already used by an earlier entry",
            ));
        }
        entries.push(entry);
    }
    Ok(entries)
}

/// One `projects` entry at `path`.
fn project(value: &Value, path: &Path) -> Result<ProjectEntry, Error> {
    let map = value
        .as_object()
        .ok_or_else(|| Error::schema(path, "must be a mapping"))?;
    if let Some(key) = map.keys().find(|key| {
        !["project_id", "repo", "host", "installation_id", "weight"].contains(&key.as_str())
    }) {
        return Err(Error::schema(&path.key(key), "unknown key"));
    }
    let project_id = match map.get("project_id") {
        Some(Value::String(id)) if !id.is_empty() => ProjectId(id.clone()),
        Some(_) => {
            return Err(Error::schema(
                &path.key("project_id"),
                "must be a non-empty string",
            ));
        }
        None => return Err(Error::schema(&path.key("project_id"), "is missing")),
    };
    let repo = match map.get("repo") {
        Some(Value::String(repo)) if is_owner_and_name(repo) => repo.clone(),
        Some(_) => {
            return Err(Error::schema(
                &path.key("repo"),
                "must be a repository as owner/name",
            ));
        }
        None => return Err(Error::schema(&path.key("repo"), "is missing")),
    };
    let host = match map.get("host") {
        None => Host::Github,
        Some(Value::String(host)) if host == Host::Github.name() => Host::Github,
        Some(_) => return Err(Error::schema(&path.key("host"), "must be github")),
    };
    let installation_id = match map.get("installation_id") {
        Some(value) => value.as_i64().filter(|&id| id > 0).ok_or_else(|| {
            Error::schema(&path.key("installation_id"), "must be a positive integer")
        })?,
        None => return Err(Error::schema(&path.key("installation_id"), "is missing")),
    };
    let weight = match map.get("weight") {
        None => 1,
        Some(value) => value
            .as_i64()
            .filter(|&weight| weight > 0)
            .and_then(|weight| i32::try_from(weight).ok())
            .ok_or_else(|| {
                Error::schema(
                    &path.key("weight"),
                    format!("must be an integer from 1 to {}", i32::MAX),
                )
            })?,
    };
    Ok(ProjectEntry {
        project_id,
        repo,
        host,
        installation_id,
        weight,
    })
}

/// Whether `repo` is `owner/name`: two non-empty parts without whitespace, split by one `/`.
fn is_owner_and_name(repo: &str) -> bool {
    match repo.split_once('/') {
        Some((owner, name)) => [owner, name]
            .iter()
            .all(|part| !part.is_empty() && !part.contains(['/', ' ', '\t', '\n', '\r'])),
        None => false,
    }
}
