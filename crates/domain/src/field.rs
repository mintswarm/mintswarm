//! Mapping a canonical scale onto one tracker field, design §4.
//!
//! A [`FieldMap`] is built from a raw [`FieldMapSpec`] and enforces the invariants of design
//! §4.1. The tracker sees only a non-generic [`FieldRef`], so its adapter never needs the scale.

use std::collections::HashSet;
use std::fmt;
use std::future::Future;

use serde::{Deserialize, Serialize};

use crate::scale::{Scale, ScaleRange, ScaleRangeError};

/// Where a role's value lives on the tracker. A role whose source is `off` has no map at all.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "source", rename_all = "snake_case")]
pub enum FieldSource {
    /// An organisation issue field, single select.
    Field {
        /// The field's name.
        name: String,
        /// The field's id, recorded at setup.
        field_id: u64,
    },
    /// One label per option.
    Labels {
        /// The label pattern, for example `effort:{option}`.
        label: String,
    },
}

/// The field a tracker reads or writes: its source and the option names the map knows.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FieldRef {
    pub source: FieldSource,
    /// The map's option names, in config order.
    pub options: Vec<String>,
}

/// A role's source as the config states it, before validation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SourceSpec {
    Field { name: String, field_id: i64 },
    Labels { label: String },
}

/// One option entry as the config states it, before validation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OptionSpec {
    /// The tracker option's name.
    pub name: String,
    /// `lo-hi` or one value, by the scale's names.
    pub range: String,
    /// Whether the plane writes this option.
    pub write: bool,
    /// The value a person's choice stands for; the range's upper bound if `None`.
    pub reads_as: Option<String>,
}

/// A role's field mapping as the config states it, before validation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FieldMapSpec {
    pub source: SourceSpec,
    /// The role's default and fallback value, by name.
    pub default: String,
    pub options: Vec<OptionSpec>,
}

/// A validated option entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FieldOption<S: Scale> {
    pub name: String,
    pub range: ScaleRange<S>,
    pub write: bool,
    pub reads_as: S,
}

/// A validated mapping of the scale `S` onto one tracker field.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FieldMap<S: Scale> {
    source: FieldSource,
    default: S,
    options: Vec<FieldOption<S>>,
    read_only: bool,
}

/// Why a [`FieldMapSpec`] breaks an invariant of design §4.1.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FieldMapError {
    /// `field_id` is zero or negative.
    FieldIdNotPositive { field_id: i64 },
    /// `default` is not a value of the scale.
    UnknownDefault { default: String },
    /// The map has no options.
    NoOptions,
    /// Two options share a name.
    DuplicateOption { option: String },
    /// An option's range does not parse.
    BadRange {
        option: String,
        range: String,
        reason: ScaleRangeError,
    },
    /// An option's `reads_as` is not a value of the scale.
    UnknownReadsAs { option: String, reads_as: String },
    /// An option's `reads_as` lies outside its range.
    ReadsAsOutsideRange {
        option: String,
        reads_as: String,
        range: String,
    },
    /// Two written options both cover a value.
    Overlap {
        value: String,
        first: String,
        second: String,
    },
    /// No written option covers a value.
    Uncovered { value: String },
}

impl fmt::Display for FieldMapError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::FieldIdNotPositive { field_id } => {
                write!(f, "field_id {field_id} is not a positive integer")
            }
            Self::UnknownDefault { default } => {
                write!(f, "default {default:?} is not a value of the scale")
            }
            Self::NoOptions => f.write_str("the map has no options"),
            Self::DuplicateOption { option } => write!(f, "option {option:?} is repeated"),
            Self::BadRange {
                option,
                range,
                reason,
            } => write!(f, "option {option:?}: range {range:?}: {reason}"),
            Self::UnknownReadsAs { option, reads_as } => write!(
                f,
                "option {option:?}: reads_as {reads_as:?} is not a value of the scale"
            ),
            Self::ReadsAsOutsideRange {
                option,
                reads_as,
                range,
            } => write!(
                f,
                "option {option:?}: reads_as {reads_as} lies outside its range {range}"
            ),
            Self::Overlap {
                value,
                first,
                second,
            } => write!(
                f,
                "written options {first:?} and {second:?} both cover {value}"
            ),
            Self::Uncovered { value } => write!(f, "no written option covers {value}"),
        }
    }
}

impl std::error::Error for FieldMapError {}

impl<S: Scale> FieldMap<S> {
    /// Validate `spec` against the invariants of design §4.1, including R1's exemption: a role
    /// whose options are all `write: false` is read-only and needs no partition.
    pub fn new(spec: FieldMapSpec) -> Result<Self, FieldMapError> {
        let source = match spec.source {
            SourceSpec::Field { name, field_id } => {
                let id = u64::try_from(field_id)
                    .ok()
                    .filter(|id| *id > 0)
                    .ok_or(FieldMapError::FieldIdNotPositive { field_id })?;
                FieldSource::Field { name, field_id: id }
            }
            SourceSpec::Labels { label } => FieldSource::Labels { label },
        };
        let default = S::parse(&spec.default).ok_or_else(|| FieldMapError::UnknownDefault {
            default: spec.default.clone(),
        })?;
        if spec.options.is_empty() {
            return Err(FieldMapError::NoOptions);
        }
        let mut seen = HashSet::new();
        for option in &spec.options {
            if !seen.insert(option.name.as_str()) {
                return Err(FieldMapError::DuplicateOption {
                    option: option.name.clone(),
                });
            }
        }
        let options = spec
            .options
            .into_iter()
            .map(Self::option_from)
            .collect::<Result<Vec<_>, _>>()?;
        let read_only = options.iter().all(|option| !option.write);
        if !read_only {
            Self::check_partition(&options)?;
        }
        Ok(Self {
            source,
            default,
            options,
            read_only,
        })
    }

    fn option_from(spec: OptionSpec) -> Result<FieldOption<S>, FieldMapError> {
        let range = ScaleRange::parse(&spec.range).map_err(|reason| FieldMapError::BadRange {
            option: spec.name.clone(),
            range: spec.range.clone(),
            reason,
        })?;
        let reads_as = match &spec.reads_as {
            None => range.hi(),
            Some(text) => S::parse(text).ok_or_else(|| FieldMapError::UnknownReadsAs {
                option: spec.name.clone(),
                reads_as: text.clone(),
            })?,
        };
        if !range.contains(reads_as) {
            return Err(FieldMapError::ReadsAsOutsideRange {
                option: spec.name,
                reads_as: reads_as.name().to_owned(),
                range: range.to_string(),
            });
        }
        Ok(FieldOption {
            name: spec.name,
            range,
            write: spec.write,
            reads_as,
        })
    }

    /// Each value of the scale must be covered by exactly one written option.
    fn check_partition(options: &[FieldOption<S>]) -> Result<(), FieldMapError> {
        for &value in S::ALL {
            let mut covering = options
                .iter()
                .filter(|option| option.write && option.range.contains(value));
            match (covering.next(), covering.next()) {
                (None, _) => {
                    return Err(FieldMapError::Uncovered {
                        value: value.name().to_owned(),
                    });
                }
                (Some(first), Some(second)) => {
                    return Err(FieldMapError::Overlap {
                        value: value.name().to_owned(),
                        first: first.name.clone(),
                        second: second.name.clone(),
                    });
                }
                (Some(_), None) => {}
            }
        }
        Ok(())
    }

    /// Where the role's value lives.
    pub fn source(&self) -> &FieldSource {
        &self.source
    }

    /// The role's default, which is also its fallback value.
    pub fn default(&self) -> S {
        self.default
    }

    /// The option entries, in config order.
    pub fn options(&self) -> &[FieldOption<S>] {
        &self.options
    }

    /// Whether every option is `write: false`, so the role is only ever read.
    pub fn is_read_only(&self) -> bool {
        self.read_only
    }

    /// The option with this name, if any.
    pub fn option(&self, name: &str) -> Option<&FieldOption<S>> {
        self.options.iter().find(|option| option.name == name)
    }

    /// The non-generic reference the tracker reads and writes through.
    pub fn field_ref(&self) -> FieldRef {
        FieldRef {
            source: self.source.clone(),
            options: self
                .options
                .iter()
                .map(|option| option.name.clone())
                .collect(),
        }
    }
}

/// An option value as the tracker holds it.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FieldValue {
    /// The field's id; `None` in labels mode.
    pub field_id: Option<u64>,
    /// The option's id, where the tracker has one.
    pub option_id: Option<u64>,
    /// The option's name.
    pub name: String,
}

/// What a read of a field on one issue found.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FieldRead {
    /// The issue has no value.
    Unset,
    /// The issue has one value.
    Value(FieldValue),
    /// The issue carries two or more of the role's labels (labels mode only, R3).
    Conflict(Vec<FieldValue>),
}

/// A drift finding, design §4.3.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FieldFinding {
    FieldMissing,
    FieldRecreated,
    OptionMissing,
    OptionRenamed,
    OptionUnmapped,
    LabelsConflict,
    WriteRejected,
}

impl FieldFinding {
    /// The finding's name, as serde spells it.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::FieldMissing => "field_missing",
            Self::FieldRecreated => "field_recreated",
            Self::OptionMissing => "option_missing",
            Self::OptionRenamed => "option_renamed",
            Self::OptionUnmapped => "option_unmapped",
            Self::LabelsConflict => "labels_conflict",
            Self::WriteRejected => "write_rejected",
        }
    }
}

impl fmt::Display for FieldFinding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Why a field read or write failed. Callers act on it under design §4.5.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FieldError {
    /// A drift-class refusal.
    Field(FieldFinding),
    /// A transport, rate-limit or server error; not drift.
    Transport(String),
}

impl fmt::Display for FieldError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Field(finding) => write!(f, "field error: {finding}"),
            Self::Transport(detail) => write!(f, "transport error: {detail}"),
        }
    }
}

impl std::error::Error for FieldError {}

/// Lists a field's options. Only the operator's admin commands use it, never the plane.
pub trait FieldCatalogue: Send + Sync {
    fn options(
        &self,
        field: &FieldRef,
    ) -> impl Future<Output = Result<Vec<FieldValue>, FieldError>> + Send;
}

#[cfg(test)]
mod tests {
    use super::{FieldMap, FieldMapError, FieldMapSpec, FieldSource, OptionSpec, SourceSpec};
    use crate::scale::Size;

    fn option(name: &str, range: &str) -> OptionSpec {
        OptionSpec {
            name: name.into(),
            range: range.into(),
            write: true,
            reads_as: None,
        }
    }

    fn labels(options: Vec<OptionSpec>) -> FieldMapSpec {
        FieldMapSpec {
            source: SourceSpec::Labels {
                label: "effort:{option}".into(),
            },
            default: "M".into(),
            options,
        }
    }

    #[test]
    fn spans_partition_and_read_as_their_upper_bound() {
        let map = FieldMap::<Size>::new(labels(vec![
            option("Low", "XS-S"),
            option("Medium", "M-L"),
            option("High", "XL-4XL"),
        ]))
        .unwrap();
        assert_eq!(map.option("Medium").unwrap().reads_as, Size::L);
        assert_eq!(map.default(), Size::M);
        let field = map.field_ref();
        assert_eq!(field.options, ["Low", "Medium", "High"]);
        assert!(matches!(field.source, FieldSource::Labels { .. }));
    }

    #[test]
    fn rejects_the_spec_errors_outside_the_invariants() {
        let check = |options, want: fn(&FieldMapError) -> bool| {
            let err = FieldMap::<Size>::new(labels(options)).unwrap_err();
            assert!(want(&err), "{err}");
        };
        check(vec![], |e| matches!(e, FieldMapError::NoOptions));
        check(vec![option("A", "XS-4XL"), option("A", "M")], |e| {
            matches!(e, FieldMapError::DuplicateOption { .. })
        });
        check(vec![option("A", "")], |e| {
            matches!(e, FieldMapError::BadRange { .. })
        });
        let mut odd = option("A", "XS-4XL");
        odd.reads_as = Some("5XL".into());
        check(vec![odd], |e| {
            matches!(e, FieldMapError::UnknownReadsAs { .. })
        });
    }
}
