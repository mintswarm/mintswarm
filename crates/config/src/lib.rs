//! Loaders for mintswarm's YAML configuration: the repository config `.mintswarm/config.yaml`,
//! the runner config, the capability probe definitions and the activity rules, which share one
//! schema between the runner's defaults and `metrics.activity` in the repository config.
//!
//! YAML is read natively, with the same result as `yq -p=yaml -o=json .` (yq v4.45.1) gives for
//! it, and the repository config is then checked as the dogfood floors check it.

mod activity;
mod probes;
mod runner;
mod schema;
mod state;
mod trust;
mod yaml;

use std::fmt;

pub use activity::{ActivityMatch, ActivityRule, load_activity_rules};
pub use probes::{
    DEFAULT_PROBE_TIMEOUT, DetectMethod, ProbeDefinition, ProbeOrigin, load_probe_definition,
    merge_probe_definitions,
};
pub use runner::{DEFAULT_WALL_CLOCK, RunnerConfig, Stage, load_runner_config};
pub use schema::{
    DependencyReview, Formatter, GatesSummary, Landing, LimitOverrides, Limits, Mode, PlanApproval,
    RepoConfig, RequireHuman, ReviewClear, load_repo_config, load_repo_config_with_roles,
};
pub use state::{
    AWAITING_APPROVAL, AWAITING_MERGE, BLOCKED, DONE, NONE, PLAN_APPROVED, PLANNING, QUEUED,
    REQUIRED_OPTIONS, StateOption, StateSettings, StatusMode,
};
pub use trust::{Action, ActionTable, ActionTrust, People, Trust, TrustMatch};
pub use yaml::parse_yaml;

/// An error from loading a config, with the key path it concerns.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    /// The text is not YAML that converts to JSON as yq converts it.
    #[error("{}", Located { path, message })]
    Yaml {
        /// The key path, empty for the document as a whole.
        path: String,
        /// What is wrong.
        message: String,
    },
    /// The document does not follow the config's schema.
    #[error("{}", Located { path, message })]
    Schema {
        /// The key path, empty for the document root.
        path: String,
        /// What is wrong.
        message: String,
    },
}

impl Error {
    /// The key path the error concerns: keys joined by `.`, with `[i]` for sequence items, and
    /// empty for the document root.
    pub fn path(&self) -> &str {
        match self {
            Error::Yaml { path, .. } | Error::Schema { path, .. } => path,
        }
    }

    /// What is wrong, without the path.
    pub fn message(&self) -> &str {
        match self {
            Error::Yaml { message, .. } | Error::Schema { message, .. } => message,
        }
    }

    pub(crate) fn yaml(path: &Path, message: impl Into<String>) -> Self {
        Error::Yaml {
            path: path.to_string(),
            message: message.into(),
        }
    }

    pub(crate) fn schema(path: &Path, message: impl Into<String>) -> Self {
        Error::Schema {
            path: path.to_string(),
            message: message.into(),
        }
    }
}

/// `<path>: <message>`, or the message alone at the document root.
struct Located<'a> {
    path: &'a str,
    message: &'a str,
}

impl fmt::Display for Located<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.path.is_empty() {
            f.write_str(self.message)
        } else {
            write!(f, "{}: {}", self.path, self.message)
        }
    }
}

/// One step of a key path.
#[derive(Debug, Clone)]
pub(crate) enum Segment {
    Key(String),
    Index(usize),
}

/// A key path, built while walking a document.
#[derive(Debug, Clone, Default)]
pub(crate) struct Path(Vec<Segment>);

impl Path {
    pub(crate) fn root() -> Self {
        Path(Vec::new())
    }

    pub(crate) fn key(&self, key: &str) -> Self {
        let mut path = self.clone();
        path.0.push(Segment::Key(key.to_owned()));
        path
    }

    pub(crate) fn index(&self, index: usize) -> Self {
        let mut path = self.clone();
        path.0.push(Segment::Index(index));
        path
    }
}

impl fmt::Display for Path {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (n, segment) in self.0.iter().enumerate() {
            match segment {
                Segment::Key(key) if n == 0 => f.write_str(key)?,
                Segment::Key(key) => write!(f, ".{key}")?,
                Segment::Index(index) => write!(f, "[{index}]")?,
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{Error, Path};

    #[test]
    fn path_joins_keys_and_indices() {
        let path = Path::root().key("gates").key("run").index(3).key("name");
        assert_eq!(path.to_string(), "gates.run[3].name");
        assert_eq!(Path::root().index(0).to_string(), "[0]");
    }

    #[test]
    fn display_is_path_then_message() {
        let error = Error::schema(&Path::root().key("a").key("b"), "unknown key");
        assert_eq!(error.to_string(), "a.b: unknown key");
        assert_eq!(error.path(), "a.b");
        let error = Error::yaml(&Path::root(), "empty input");
        assert_eq!(error.to_string(), "empty input");
        assert_eq!(error.path(), "");
    }
}
