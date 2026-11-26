//! `tracker.state`: the `msw-state` values and the issue status each one maps to, design §5.2.
//!
//! The values live in `tracker.state.options` only. The eight names the pipeline itself sets
//! are the constants here, the only Rust copy of any value; there is no enum of the values.

use mintswarm_domain::field::{FieldRef, FieldSource};
use serde_json::Value;

use crate::schema::{kind, known, mapping};
use crate::{Error, Path};

/// The value of an issue mintswarm has not touched.
pub const NONE: &str = "none";
/// The value of an issue waiting for a runner.
pub const QUEUED: &str = "queued";
/// The value of an issue waiting on another one.
pub const BLOCKED: &str = "blocked";
/// The value of an issue a runner is planning.
pub const PLANNING: &str = "planning";
/// The value of an issue whose plan waits for an approval.
pub const AWAITING_APPROVAL: &str = "awaiting approval";
/// The value of an issue whose plan is approved.
pub const PLAN_APPROVED: &str = "plan approved";
/// The value of an issue whose pull request waits to be merged.
pub const AWAITING_MERGE: &str = "awaiting merge";
/// The value of an issue whose work has landed.
pub const DONE: &str = "done";
/// The names `tracker.state.options` must list, matched exactly, in lifecycle order.
pub const REQUIRED_OPTIONS: [&str; 8] = [
    NONE,
    QUEUED,
    BLOCKED,
    PLANNING,
    AWAITING_APPROVAL,
    PLAN_APPROVED,
    AWAITING_MERGE,
    DONE,
];

/// `tracker.state.status`: whether setting a value also sets the issue status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusMode {
    /// A value with a `status` sets the GitHub issue status of that name.
    IssueStatus,
    /// The issue status is left alone.
    Off,
}

/// One `tracker.state.options` entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StateOption {
    /// The value's name, as the field or label holds it.
    pub name: String,
    /// The issue status the value sets, `None` to leave the status alone.
    pub status: Option<String>,
}

/// `tracker.state`, typed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StateSettings {
    /// Where the value lives on the tracker.
    pub source: FieldSource,
    /// `status`.
    pub status: StatusMode,
    /// `options`, in config order.
    pub options: Vec<StateOption>,
}

impl StateSettings {
    /// The field the tracker reads and writes: the source and the option names, in order.
    pub fn field_ref(&self) -> FieldRef {
        FieldRef {
            source: self.source.clone(),
            options: self.options.iter().map(|o| o.name.clone()).collect(),
        }
    }

    /// The option of exactly that name.
    pub fn option(&self, name: &str) -> Option<&StateOption> {
        self.options.iter().find(|option| option.name == name)
    }
}

/// `tracker.state.options`, checked as `msw-state.py` `settings` checks it, in the same order,
/// failing at the first error.
pub(crate) fn check_options(value: Option<&Value>, path: &Path) -> Result<Vec<StateOption>, Error> {
    let items = match value {
        Some(Value::Array(items)) => items,
        None => return Err(Error::schema(path, "is missing")),
        Some(other) => {
            return Err(Error::schema(
                path,
                format!("must be a list, not {}", kind(other)),
            ));
        }
    };
    let mut options: Vec<StateOption> = Vec::with_capacity(items.len());
    for (index, item) in items.iter().enumerate() {
        let item_path = path.index(index);
        let entry = mapping(item, &item_path)?;
        known(entry, &item_path, &["name", "status"])?;
        let name_path = item_path.key("name");
        let name = match entry.get("name") {
            Some(Value::String(name)) if !name.trim().is_empty() => name,
            _ => return Err(Error::schema(&name_path, "must be a non-empty string")),
        };
        if let Some(earlier) = options
            .iter()
            .find(|option| option.name.eq_ignore_ascii_case(name))
        {
            return Err(Error::schema(
                &name_path,
                format!("repeats {:?} in any case", earlier.name),
            ));
        }
        let status = match entry.get("status") {
            None => None,
            Some(Value::String(status)) if !status.trim().is_empty() => Some(status.clone()),
            Some(_) => {
                return Err(Error::schema(
                    &item_path.key("status"),
                    "must be a non-empty string when present",
                ));
            }
        };
        options.push(StateOption {
            name: name.clone(),
            status,
        });
    }
    let missing: Vec<&str> = REQUIRED_OPTIONS
        .into_iter()
        .filter(|required| !options.iter().any(|option| option.name == *required))
        .collect();
    if !missing.is_empty() {
        return Err(Error::schema(
            path,
            format!("must list {}", missing.join(", ")),
        ));
    }
    Ok(options)
}
