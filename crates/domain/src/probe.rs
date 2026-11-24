//! Capability probe results: what a runner found out about its machine, and the version check
//! over it.
//!
//! A result is namespaced by the probe's kind and id (`tool.git`, `harness.claude`), carries
//! either `unknown` or what was found, and says when it was observed. A probe that fails or times
//! out reports `unknown`, which is a value, not an error, and a version check over it is neither a
//! pass nor a failure.

use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// What a probe detects.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProbeKind {
    /// `toolchain`: a compiler or language runtime, such as Rust or Node.
    Toolchain,
    /// `engine`: an engine a build needs, such as a game or rendering engine.
    Engine,
    /// `tool`: a command-line tool, such as git or bash.
    Tool,
    /// `service`: a service the machine can reach.
    Service,
    /// `harness`: a coding-agent harness CLI, such as Claude Code.
    Harness,
}

impl ProbeKind {
    /// Every kind.
    pub const ALL: [ProbeKind; 5] = [
        ProbeKind::Toolchain,
        ProbeKind::Engine,
        ProbeKind::Tool,
        ProbeKind::Service,
        ProbeKind::Harness,
    ];

    /// The kind's name, as definitions and namespaces write it.
    pub fn name(self) -> &'static str {
        match self {
            ProbeKind::Toolchain => "toolchain",
            ProbeKind::Engine => "engine",
            ProbeKind::Tool => "tool",
            ProbeKind::Service => "service",
            ProbeKind::Harness => "harness",
        }
    }

    /// The kind named `name`, if there is one.
    pub fn from_name(name: &str) -> Option<ProbeKind> {
        ProbeKind::ALL.into_iter().find(|kind| kind.name() == name)
    }
}

impl fmt::Display for ProbeKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// A result's namespace: the probe's kind and id, written `<kind>.<id>` (`tool.git`).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Namespace {
    /// The probe's kind.
    pub kind: ProbeKind,
    /// The probe's id, never empty.
    pub id: String,
}

impl Namespace {
    /// The namespace of the probe `id` of `kind`.
    pub fn new(kind: ProbeKind, id: impl Into<String>) -> Self {
        Namespace {
            kind,
            id: id.into(),
        }
    }
}

impl fmt::Display for Namespace {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}", self.kind, self.id)
    }
}

/// A string that is not a namespace.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NamespaceError {
    /// The string as given.
    pub namespace: String,
    /// What is wrong.
    pub message: String,
}

impl fmt::Display for NamespaceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "`{}` is not a probe namespace: {}",
            self.namespace, self.message
        )
    }
}

impl std::error::Error for NamespaceError {}

impl FromStr for Namespace {
    type Err = NamespaceError;

    /// `<kind>.<id>`, split at the first dot; the id may hold further dots.
    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let error = |message: &str| NamespaceError {
            namespace: text.to_owned(),
            message: message.to_owned(),
        };
        let (kind, id) = text
            .split_once('.')
            .ok_or_else(|| error("expected `<kind>.<id>`"))?;
        let kind = ProbeKind::from_name(kind).ok_or_else(|| error("unknown kind"))?;
        if id.is_empty() {
            return Err(error("the id is empty"));
        }
        Ok(Namespace::new(kind, id))
    }
}

impl TryFrom<String> for Namespace {
    type Error = NamespaceError;

    fn try_from(text: String) -> Result<Self, Self::Error> {
        text.parse()
    }
}

impl From<Namespace> for String {
    fn from(namespace: Namespace) -> Self {
        namespace.to_string()
    }
}

/// One fact a probe reports beyond its version, such as `logged_in` or `credential_mode`.
///
/// It is written as the plain JSON value, and read by the JSON type: a boolean, an integer or a
/// string. A float, `null`, a list or a mapping is not a fact, so the probe engine must turn such
/// output into one of these, or into `unknown`, when it builds a result.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Fact {
    /// A boolean, such as `logged_in: true`.
    Bool(bool),
    /// An integer.
    Integer(i64),
    /// A string, such as `credential_mode: subscription`.
    Text(String),
}

/// What a probe found: `unknown`, or what is known.
///
/// `unknown` is written as the plain string `"unknown"`, and a known value as
/// `{"known": {"version": …, "facts": {…}}}`, with each part left out when it is empty.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProbeValue {
    /// The probe failed, timed out or found nothing it could report.
    Unknown,
    /// The probe found the capability.
    Known {
        /// The version as the probe extracted it, such as `2.47.1`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        version: Option<String>,
        /// Further facts, by name.
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        facts: BTreeMap<String, Fact>,
    },
}

/// A probe's result, as the runner keeps and reports it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProbeResult {
    /// Which probe it is.
    pub namespace: Namespace,
    /// What it found.
    pub value: ProbeValue,
    /// When it was observed, as an RFC 3339 timestamp.
    pub observed_at: String,
}

/// A dotted numeric version, such as `2.45` or `2.47.1`.
///
/// Missing components count as 0, so `2.45` equals `2.45.0`.
#[derive(Clone, Debug)]
pub struct Version(Vec<u64>);

impl Version {
    /// The version that `text` starts with, after any leading whitespace: `2.39.5 (Apple
    /// Git-154)` gives 2.39.5 and `1.87.0-nightly` gives 1.87.0. A pre-release or other tail is
    /// dropped, so `2.45.0-rc1` compares as 2.45.0. `None` when `text` does not start with a
    /// number.
    pub fn extract(text: &str) -> Option<Version> {
        let mut components = Vec::new();
        let mut rest = text.trim_start();
        loop {
            let digits = rest.len() - rest.trim_start_matches(|c: char| c.is_ascii_digit()).len();
            if digits == 0 {
                break;
            }
            components.push(rest[..digits].parse().ok()?);
            rest = &rest[digits..];
            match rest.strip_prefix('.') {
                Some(next) if next.starts_with(|c: char| c.is_ascii_digit()) => rest = next,
                _ => break,
            }
        }
        (!components.is_empty()).then_some(Version(components))
    }

    /// The version `text` is, with nothing before or after it.
    fn parse_exact(text: &str) -> Option<Version> {
        text.split('.')
            .map(|component| {
                if component.is_empty() || !component.bytes().all(|b| b.is_ascii_digit()) {
                    return None;
                }
                component.parse().ok()
            })
            .collect::<Option<Vec<u64>>>()
            .map(Version)
    }

    /// The version's components, as written.
    pub fn components(&self) -> &[u64] {
        &self.0
    }

    fn component(&self, index: usize) -> u64 {
        self.0.get(index).copied().unwrap_or(0)
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (n, component) in self.0.iter().enumerate() {
            if n > 0 {
                f.write_str(".")?;
            }
            write!(f, "{component}")?;
        }
        Ok(())
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        let len = self.0.len().max(other.0.len());
        (0..len)
            .map(|index| self.component(index).cmp(&other.component(index)))
            .find(|ordering| ordering.is_ne())
            .unwrap_or(Ordering::Equal)
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialEq for Version {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other).is_eq()
    }
}

impl Eq for Version {}

/// A comparison operator in a version requirement.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Operator {
    /// `>=`.
    AtLeast,
    /// `>`.
    Above,
    /// `<=`.
    AtMost,
    /// `<`.
    Below,
    /// `=`.
    Exactly,
}

impl Operator {
    /// Every operator, two-character ones first so that a prefix match finds them.
    const ALL: [Operator; 5] = [
        Operator::AtLeast,
        Operator::AtMost,
        Operator::Above,
        Operator::Below,
        Operator::Exactly,
    ];

    /// The operator as written.
    pub fn symbol(self) -> &'static str {
        match self {
            Operator::AtLeast => ">=",
            Operator::Above => ">",
            Operator::AtMost => "<=",
            Operator::Below => "<",
            Operator::Exactly => "=",
        }
    }

    fn holds(self, ordering: Ordering) -> bool {
        match self {
            Operator::AtLeast => ordering.is_ge(),
            Operator::Above => ordering.is_gt(),
            Operator::AtMost => ordering.is_le(),
            Operator::Below => ordering.is_lt(),
            Operator::Exactly => ordering.is_eq(),
        }
    }
}

/// One comparison of a requirement, such as `>= 2.45`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Comparison {
    /// How the version compares.
    pub operator: Operator,
    /// What it is compared with.
    pub version: Version,
}

/// The outcome of a version check.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Check {
    /// The version meets the requirement.
    Satisfied,
    /// The version does not meet the requirement.
    Unsatisfied,
    /// There is no version to check: the result is `unknown`, has no version, or its version
    /// does not start with a number.
    Unknown,
}

/// A requirement that is not well formed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RequirementError {
    /// The requirement as given.
    pub requirement: String,
    /// What is wrong.
    pub message: String,
}

impl fmt::Display for RequirementError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "`{}` is not a version requirement: {}",
            self.requirement, self.message
        )
    }
}

impl std::error::Error for RequirementError {}

/// A version requirement: comparisons joined by commas, all of which must hold (`>= 2.45, < 3`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VersionRequirement(Vec<Comparison>);

impl VersionRequirement {
    /// Parse a requirement. Each comparison is an operator (`>=`, `>`, `<=`, `<` or `=`) and a
    /// dotted numeric version, with optional whitespace around and between them.
    pub fn parse(text: &str) -> Result<VersionRequirement, RequirementError> {
        let error = |message: String| RequirementError {
            requirement: text.to_owned(),
            message,
        };
        if text.trim().is_empty() {
            return Err(error("it is empty".to_owned()));
        }
        let comparisons = text
            .split(',')
            .map(|part| {
                let part = part.trim();
                let (operator, rest) = Operator::ALL
                    .into_iter()
                    .find_map(|operator| {
                        part.strip_prefix(operator.symbol())
                            .map(|rest| (operator, rest))
                    })
                    .ok_or_else(|| {
                        error(format!(
                            "`{part}` does not start with `>=`, `>`, `<=`, `<` or `=`"
                        ))
                    })?;
                let written = rest.trim();
                let version = Version::parse_exact(written)
                    .ok_or_else(|| error(format!("`{written}` is not a dotted numeric version")))?;
                Ok(Comparison { operator, version })
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(VersionRequirement(comparisons))
    }

    /// The requirement's comparisons.
    pub fn comparisons(&self) -> &[Comparison] {
        &self.0
    }

    /// Whether `version` meets every comparison.
    pub fn matches(&self, version: &Version) -> bool {
        self.0
            .iter()
            .all(|comparison| comparison.operator.holds(version.cmp(&comparison.version)))
    }

    /// Check a probe's value: [`Check::Unknown`] when it is `unknown`, has no version, or has a
    /// version that does not start with a number; otherwise whether its version matches.
    pub fn check(&self, value: &ProbeValue) -> Check {
        let ProbeValue::Known {
            version: Some(version),
            ..
        } = value
        else {
            return Check::Unknown;
        };
        match Version::extract(version) {
            None => Check::Unknown,
            Some(version) if self.matches(&version) => Check::Satisfied,
            Some(_) => Check::Unsatisfied,
        }
    }
}

impl fmt::Display for VersionRequirement {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (n, comparison) in self.0.iter().enumerate() {
            if n > 0 {
                f.write_str(", ")?;
            }
            write!(f, "{} {}", comparison.operator.symbol(), comparison.version)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::Version;

    #[test]
    fn extract_takes_the_numeric_prefix() {
        let version = |text: &str| Version::extract(text).map(|version| version.to_string());
        assert_eq!(version("2.39.5 (Apple Git-154)").as_deref(), Some("2.39.5"));
        assert_eq!(version("1.87.0-nightly").as_deref(), Some("1.87.0"));
        assert_eq!(version("  2.45.").as_deref(), Some("2.45"));
        assert_eq!(version("v2.45"), None);
        assert_eq!(version(""), None);
    }

    #[test]
    fn parse_exact_takes_the_whole_string() {
        assert!(Version::parse_exact("2.45").is_some());
        assert!(Version::parse_exact("2.45.").is_none());
        assert!(Version::parse_exact("2.45rc").is_none());
        assert!(Version::parse_exact("2..45").is_none());
        assert!(Version::parse_exact("").is_none());
    }
}
