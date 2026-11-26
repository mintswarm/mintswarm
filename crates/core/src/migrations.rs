//! The Postgres migration tree under `crates/core/migrations/`: its check, the list the build
//! embeds, and minting a new migration's version.
//!
//! The directory is the only source of the tree. The build script checks it with [`validate`]
//! and embeds each file's path and bytes, and [`embedded_tree`] runs the same check on them.

mod tree;

use std::collections::BTreeSet;
use std::fmt;
use std::fs::{self, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};

use mintswarm_domain::time::civil_from_days;

pub use tree::{
    Down, Migration, Report, SCHEMA, TreeError, Warning, is_valid_name, is_valid_version,
    read_tree, validate,
};

/// Every file of the tree as `(path relative to the tree root, bytes)`, sorted by path, as the
/// build found it.
static EMBEDDED: &[(&str, &[u8])] = include!(concat!(env!("OUT_DIR"), "/migrations.rs"));

/// The embedded tree's files as `(path relative to the tree root, bytes)`, sorted by path.
pub fn embedded() -> &'static [(&'static str, &'static [u8])] {
    EMBEDDED
}

/// The embedded tree, checked. The build refuses a tree with errors, so this gives the
/// migrations and warnings.
pub fn embedded_tree() -> Result<Report, Vec<TreeError>> {
    validate(EMBEDDED)
}

/// Formats a Unix second as a version, `YYYYMMDDhhmmss` in UTC.
pub fn version_of(unix_second: u64) -> String {
    let days = unix_second / 86_400;
    let of_day = unix_second % 86_400;
    let (year, month, day) = civil_from_days(days);
    let (hour, minute, second) = (of_day / 3600, of_day % 3600 / 60, of_day % 60);
    format!("{year:04}{month:02}{day:02}{hour:02}{minute:02}{second:02}")
}

/// Why a migration cannot be minted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MintError {
    /// The name is not one or more letters, digits and underscores.
    InvalidName {
        /// The name given.
        name: String,
    },
    /// The schema's migrations folder cannot be read.
    Unreadable {
        /// The folder.
        path: PathBuf,
    },
    /// The folder or the file cannot be created.
    Uncreatable {
        /// The folder or file.
        path: PathBuf,
    },
}

impl fmt::Display for MintError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidName { name } => write!(
                f,
                "the name {name:?} is not one or more letters, digits and underscores"
            ),
            Self::Unreadable { path } => write!(f, "{}: cannot be read", path.display()),
            Self::Uncreatable { path } => write!(f, "{}: cannot be created", path.display()),
        }
    }
}

impl std::error::Error for MintError {}

/// Creates an empty up file `<version>_<name>.up.sql` in the tree at `tree_root`, and gives its
/// path.
///
/// The version is `now`, a Unix second, or the next second free in the schema when it is taken.
/// An existing file is never overwritten, and an invalid name is refused before anything on
/// disk is touched.
pub fn mint(tree_root: &Path, name: &str, now: u64) -> Result<PathBuf, MintError> {
    if !is_valid_name(name) {
        return Err(MintError::InvalidName {
            name: name.to_owned(),
        });
    }
    let dir = tree_root.join(SCHEMA).join("migrations");
    fs::create_dir_all(&dir).map_err(|_| MintError::Uncreatable { path: dir.clone() })?;
    let taken = taken_versions(&dir)?;
    let mut second = now;
    loop {
        let version = version_of(second);
        if !taken.contains(&version) {
            let path = dir.join(format!("{version}_{name}.up.sql"));
            match OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(_) => return Ok(path),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                Err(_) => return Err(MintError::Uncreatable { path }),
            }
        }
        second = second
            .checked_add(1)
            .ok_or_else(|| MintError::Uncreatable { path: dir.clone() })?;
    }
}

/// The versions of the files already in a schema's migrations folder, whatever their kind.
fn taken_versions(dir: &Path) -> Result<BTreeSet<String>, MintError> {
    let unreadable = || MintError::Unreadable {
        path: dir.to_path_buf(),
    };
    let mut taken = BTreeSet::new();
    for entry in fs::read_dir(dir).map_err(|_| unreadable())? {
        let file_name = entry.map_err(|_| unreadable())?.file_name();
        let Some((version, _)) = file_name.to_str().and_then(|name| name.split_once('_')) else {
            continue;
        };
        if is_valid_version(version) {
            taken.insert(version.to_owned());
        }
    }
    Ok(taken)
}
