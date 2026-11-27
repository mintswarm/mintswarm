//! The migration tree's layout, file names and header grammar, checked by one function.
//!
//! This file uses `std` only: the library declares it as a module, and `build.rs` compiles the
//! same file with `#[path]`, so the build and the library check a tree the same way.
//!
//! The tree holds `<schema>/migrations/<version>_<name>.up.sql`, each with an optional
//! `.down.sql`, and the only schema is [`SCHEMA`]. A header is a line `-- migrate: <key>` or
//! `-- migrate: <key> <value>` among a file's leading lines that are blank or start with `--`.

use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;
use std::fs;
use std::io;
use std::path::Path;

/// The only schema, and so the only top-level folder of the tree.
pub const SCHEMA: &str = "mintswarm";

/// The folder under a schema that holds its migrations.
const MIGRATIONS: &str = "migrations";

/// The suffix of an up file.
const UP: &str = ".up.sql";

/// The suffix of a down file.
const DOWN: &str = ".down.sql";

/// The prefix of a header line, after the line's leading whitespace.
const PREFIX: &str = "-- migrate:";

/// The key that runs a file outside a transaction.
const NO_TRANSACTION: &str = "no-transaction";

/// A header key and where it may be used for now.
struct Key {
    name: &'static str,
    /// Supported on an up file; an unsupported key is refused until its child lifts it.
    on_up: bool,
    /// Allowed on a down file, which may only ever carry `no-transaction`.
    on_down: bool,
    /// Takes a value after the key.
    value: bool,
}

/// A header key's table entry: where it may be used for now, and whether it takes a value.
const fn key(name: &'static str, on_up: bool, on_down: bool, value: bool) -> Key {
    Key {
        name,
        on_up,
        on_down,
        value,
    }
}

/// Every recognised header key, as `key(name, on_up, on_down, value)`. Lifting a refusal
/// changes one entry.
const KEYS: &[Key] = &[
    key(NO_TRANSACTION, true, true, false),
    key("depends-on", false, false, true),
    key("irreversible-enum", false, false, true),
    key("code-floor", false, false, true),
    key("replaces", false, false, true),
    key("pile-hash", false, false, true),
    key("pile-commit", false, false, true),
    key("declared-difference", false, false, true),
];

/// Whether `name` is a valid migration name: one or more ASCII letters, digits or underscores.
pub fn is_valid_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

/// Whether `version` is a valid version: exactly 14 ASCII digits that form a real UTC date and
/// time, `YYYYMMDDhhmmss`, leap days included and leap seconds not.
pub fn is_valid_version(version: &str) -> bool {
    if version.len() != 14 || !version.bytes().all(|byte| byte.is_ascii_digit()) {
        return false;
    }
    let field = |range: std::ops::Range<usize>| {
        version
            .get(range)
            .and_then(|digits| digits.parse::<u32>().ok())
    };
    let (Some(year), Some(month), Some(day), Some(hour), Some(minute), Some(second)) = (
        field(0..4),
        field(4..6),
        field(6..8),
        field(8..10),
        field(10..12),
        field(12..14),
    ) else {
        return false;
    };
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let days = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => return false,
    };
    (1..=days).contains(&day) && hour < 24 && minute < 60 && second < 60
}

/// One migration of the tree: an up file and its optional down file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Migration {
    /// The schema, the name of the top-level folder.
    pub schema: String,
    /// The version, a UTC second as `YYYYMMDDhhmmss`.
    pub version: String,
    /// The name after the version.
    pub name: String,
    /// The up file's path relative to the tree root, with `/` separators.
    pub up: String,
    /// Whether the up file runs outside a transaction.
    pub no_transaction: bool,
    /// The down file, if there is one.
    pub down: Option<Down>,
}

/// A migration's down file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Down {
    /// The down file's path relative to the tree root, with `/` separators.
    pub file: String,
    /// Whether the down file runs outside a transaction.
    pub no_transaction: bool,
}

/// A valid tree: its migrations sorted by version, and the warnings it gave.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    /// The migrations, sorted by schema and version.
    pub migrations: Vec<Migration>,
    /// The warnings, in path order.
    pub warnings: Vec<Warning>,
}

/// Something a valid tree may hold but should not.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Warning {
    /// An up file has no down file.
    MissingDown {
        /// The up file.
        file: String,
    },
}

impl fmt::Display for Warning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingDown { file } => write!(f, "{file}: the migration has no down file"),
        }
    }
}

/// Why a tree is refused. Each error names the file, relative to the tree root.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TreeError {
    /// The path is not `<schema>/migrations/<file>.up.sql` or `.down.sql` of a known schema.
    UnexpectedPath {
        /// The file.
        file: String,
    },
    /// The file name has no `_` between the version and the name.
    BadFileName {
        /// The file.
        file: String,
    },
    /// The version is not 14 digits that form a real UTC date and time.
    BadVersion {
        /// The file.
        file: String,
    },
    /// The name has a character other than a letter, digit or underscore, or is empty.
    BadName {
        /// The file.
        file: String,
    },
    /// Another migration of the schema already has this file's version.
    DuplicateVersion {
        /// The later file with the version.
        file: String,
    },
    /// A down file has no up file with its version and name.
    DownWithoutUp {
        /// The down file.
        file: String,
    },
    /// The file is not UTF-8.
    NotUtf8 {
        /// The file.
        file: String,
    },
    /// A header names a key that is not recognised.
    UnknownKey {
        /// The file.
        file: String,
        /// The key.
        key: String,
    },
    /// A header line comes after the first statement.
    KeyBelowStatement {
        /// The file.
        file: String,
        /// The key on that line.
        key: String,
    },
    /// A recognised key is not supported on an up file yet.
    Unsupported {
        /// The file.
        file: String,
        /// The key.
        key: String,
    },
    /// A down file carries a key other than `no-transaction`.
    NotOnDown {
        /// The file.
        file: String,
        /// The key.
        key: String,
    },
    /// A header line is not `-- migrate: <key>` or `-- migrate: <key> <value>` as its key
    /// needs, or repeats a key.
    MalformedHeader {
        /// The file.
        file: String,
        /// The line, counted from 1.
        line: usize,
    },
    /// The file or folder cannot be read.
    Unreadable {
        /// The file or folder.
        file: String,
    },
}

impl fmt::Display for TreeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnexpectedPath { file } => write!(
                f,
                "{file}: only {SCHEMA}/{MIGRATIONS}/<version>_<name>{UP} and {DOWN} files belong in the tree"
            ),
            Self::BadFileName { file } => {
                write!(f, "{file}: the file name is not <version>_<name>")
            }
            Self::BadVersion { file } => write!(
                f,
                "{file}: the version is not 14 digits forming a UTC date and time, YYYYMMDDhhmmss"
            ),
            Self::BadName { file } => write!(
                f,
                "{file}: the name is not one or more letters, digits and underscores"
            ),
            Self::DuplicateVersion { file } => write!(
                f,
                "{file}: another migration of the schema already has this version"
            ),
            Self::DownWithoutUp { file } => {
                write!(f, "{file}: the down file has no up file")
            }
            Self::NotUtf8 { file } => write!(f, "{file}: the file is not UTF-8"),
            Self::UnknownKey { file, key } => {
                write!(f, "{file}: the header key {key:?} is not recognised")
            }
            Self::KeyBelowStatement { file, key } => write!(
                f,
                "{file}: the header key {key:?} comes after the first statement; headers are leading comment lines only"
            ),
            Self::Unsupported { file, key } => write!(
                f,
                "{file}: the header key {key:?} is not supported on an up file yet"
            ),
            Self::NotOnDown { file, key } => write!(
                f,
                "{file}: a down file may carry only {NO_TRANSACTION:?}, not {key:?}"
            ),
            Self::MalformedHeader { file, line } => write!(
                f,
                "{file}:{line}: the header is not `{PREFIX} <key>` or `{PREFIX} <key> <value>` as its key needs, or repeats a key"
            ),
            Self::Unreadable { file } => write!(f, "{file}: cannot be read"),
        }
    }
}

impl Error for TreeError {}

/// Whether a file is an up or a down file.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Direction {
    Up,
    Down,
}

/// A file whose path is valid.
struct Entry<'a> {
    file: &'a str,
    direction: Direction,
    version: &'a str,
    name: &'a str,
    no_transaction: bool,
}

/// Checks a tree given as `(path relative to the root, bytes)` pairs, with `/` separators.
///
/// Gives the migrations sorted by version with the warnings, or every error at once, in path
/// order.
pub fn validate(files: &[(&str, &[u8])]) -> Result<Report, Vec<TreeError>> {
    let mut sorted: Vec<(&str, &[u8])> = files.to_vec();
    sorted.sort_by(|left, right| left.0.cmp(right.0));

    let mut errors = Vec::new();
    let mut entries = Vec::new();
    for (file, bytes) in sorted {
        let Some((direction, version, name)) = parse_path(file, &mut errors) else {
            continue;
        };
        let Ok(text) = std::str::from_utf8(bytes) else {
            errors.push(TreeError::NotUtf8 {
                file: file.to_owned(),
            });
            continue;
        };
        let Some(no_transaction) = check_headers(file, text, direction, &mut errors) else {
            continue;
        };
        entries.push(Entry {
            file,
            direction,
            version,
            name,
            no_transaction,
        });
    }

    // Up files first, by version: the first up file of a version owns it.
    let mut ups: BTreeMap<&str, Migration> = BTreeMap::new();
    for entry in entries.iter().filter(|e| e.direction == Direction::Up) {
        if ups.contains_key(entry.version) {
            errors.push(TreeError::DuplicateVersion {
                file: entry.file.to_owned(),
            });
            continue;
        }
        ups.insert(
            entry.version,
            Migration {
                schema: SCHEMA.to_owned(),
                version: entry.version.to_owned(),
                name: entry.name.to_owned(),
                up: entry.file.to_owned(),
                no_transaction: entry.no_transaction,
                down: None,
            },
        );
    }
    for entry in entries.iter().filter(|e| e.direction == Direction::Down) {
        match ups.get_mut(entry.version) {
            Some(migration) if migration.name == entry.name => {
                migration.down = Some(Down {
                    file: entry.file.to_owned(),
                    no_transaction: entry.no_transaction,
                });
            }
            _ => errors.push(TreeError::DownWithoutUp {
                file: entry.file.to_owned(),
            }),
        }
    }

    if !errors.is_empty() {
        return Err(errors);
    }
    let migrations: Vec<Migration> = ups.into_values().collect();
    let warnings = migrations
        .iter()
        .filter(|migration| migration.down.is_none())
        .map(|migration| Warning::MissingDown {
            file: migration.up.clone(),
        })
        .collect();
    Ok(Report {
        migrations,
        warnings,
    })
}

/// Splits a path into its direction, version and name, or records why it is refused.
fn parse_path<'a>(
    file: &'a str,
    errors: &mut Vec<TreeError>,
) -> Option<(Direction, &'a str, &'a str)> {
    let mut refuse = |error: fn(String) -> TreeError| {
        errors.push(error(file.to_owned()));
        None
    };
    let parts: Vec<&str> = file.split('/').collect();
    let [SCHEMA, MIGRATIONS, file_name] = parts.as_slice() else {
        return refuse(|file| TreeError::UnexpectedPath { file });
    };
    let (direction, stem) = if let Some(stem) = file_name.strip_suffix(UP) {
        (Direction::Up, stem)
    } else if let Some(stem) = file_name.strip_suffix(DOWN) {
        (Direction::Down, stem)
    } else {
        return refuse(|file| TreeError::UnexpectedPath { file });
    };
    let Some((version, name)) = stem.split_once('_') else {
        return refuse(|file| TreeError::BadFileName { file });
    };
    if !is_valid_version(version) {
        return refuse(|file| TreeError::BadVersion { file });
    }
    if !is_valid_name(name) {
        return refuse(|file| TreeError::BadName { file });
    }
    Some((direction, version, name))
}

/// Checks a file's headers. Gives whether the file runs outside a transaction, or `None` after
/// recording why its headers are refused.
fn check_headers(
    file: &str,
    text: &str,
    direction: Direction,
    errors: &mut Vec<TreeError>,
) -> Option<bool> {
    let before = errors.len();
    let mut in_header = true;
    let mut seen: Vec<&str> = Vec::new();
    for (index, line) in text.lines().enumerate() {
        let trimmed = line.trim_start();
        if in_header && !(trimmed.is_empty() || trimmed.starts_with("--")) {
            in_header = false;
        }
        let Some(rest) = header_rest(trimmed) else {
            continue;
        };
        let mut words = rest.split_whitespace();
        let key = words.next().unwrap_or_default();
        if !in_header {
            errors.push(TreeError::KeyBelowStatement {
                file: file.to_owned(),
                key: key.to_owned(),
            });
            continue;
        }
        let malformed = TreeError::MalformedHeader {
            file: file.to_owned(),
            line: index + 1,
        };
        if !trimmed.starts_with(PREFIX) || key.is_empty() {
            errors.push(malformed);
            continue;
        }
        let Some(spec) = KEYS.iter().find(|spec| spec.name == key) else {
            errors.push(TreeError::UnknownKey {
                file: file.to_owned(),
                key: key.to_owned(),
            });
            continue;
        };
        let refusal = match direction {
            Direction::Down if !spec.on_down => Some(TreeError::NotOnDown {
                file: file.to_owned(),
                key: key.to_owned(),
            }),
            Direction::Up if !spec.on_up => Some(TreeError::Unsupported {
                file: file.to_owned(),
                key: key.to_owned(),
            }),
            _ => None,
        };
        if let Some(refusal) = refusal {
            errors.push(refusal);
            continue;
        }
        let has_value = words.next().is_some();
        if has_value != spec.value || seen.contains(&key) {
            errors.push(malformed);
            continue;
        }
        seen.push(key);
    }
    (errors.len() == before).then(|| seen.contains(&NO_TRANSACTION))
}

/// The text after `migrate:` when a trimmed line is a header line, including the misspelt
/// forms that differ only in the spacing after `--`, so that none is silently ignored.
fn header_rest(trimmed: &str) -> Option<&str> {
    trimmed
        .strip_prefix("--")?
        .trim_start()
        .strip_prefix("migrate:")
}

/// Reads a tree from disk as `(path relative to root, bytes)` pairs, sorted by path, with `/`
/// separators. A missing root is an empty tree.
pub fn read_tree(root: &Path) -> Result<Vec<(String, Vec<u8>)>, TreeError> {
    let mut files = Vec::new();
    match fs::read_dir(root) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(files),
        Err(_) => {
            return Err(TreeError::Unreadable {
                file: root.display().to_string(),
            });
        }
        Ok(_) => {}
    }
    walk(root, "", &mut files)?;
    files.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(files)
}

/// Collects the files under `dir`, whose path relative to the root is `prefix`.
fn walk(dir: &Path, prefix: &str, files: &mut Vec<(String, Vec<u8>)>) -> Result<(), TreeError> {
    let unreadable = |file: &str| TreeError::Unreadable {
        file: if file.is_empty() { "." } else { file }.to_owned(),
    };
    let entries = fs::read_dir(dir).map_err(|_| unreadable(prefix))?;
    for entry in entries {
        let entry = entry.map_err(|_| unreadable(prefix))?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let relative = if prefix.is_empty() {
            name.into_owned()
        } else {
            format!("{prefix}/{name}")
        };
        let path = entry.path();
        if path.to_str().is_none() {
            return Err(TreeError::UnexpectedPath { file: relative });
        }
        if path.is_dir() {
            walk(&path, &relative, files)?;
        } else {
            let bytes = fs::read(&path).map_err(|_| unreadable(&relative))?;
            files.push((relative, bytes));
        }
    }
    Ok(())
}
