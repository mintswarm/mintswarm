//! Writing, reading and checking one role's tracker field, design §4.3, §4.5 and §4.6.
//!
//! A field is referenced by name and its options by name. The ids the tracker hands out are
//! tracked data: a person can recreate a field or an option and get a new id, so an id never
//! decides how a value is read. A changed field id is reported as drift by [`findings`].

use mintswarm_domain::field::{
    FieldFinding, FieldMap, FieldOption, FieldRead, FieldSource, FieldValue,
};
use mintswarm_domain::scale::Scale;

/// The option to write for `value`, design §4.6.
///
/// `None` for a read-only map (R1), whose options are never written. Otherwise the one written
/// option whose range holds `value`; the partition invariant of design §4.1 makes it unique.
pub fn write_option<S: Scale>(map: &FieldMap<S>, value: S) -> Option<&FieldOption<S>> {
    if map.is_read_only() {
        return None;
    }
    map.options()
        .iter()
        .find(|option| option.write && option.range.contains(value))
}

/// How a read of one issue's field is interpreted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReadValue<'m, S: Scale> {
    /// The issue holds no value.
    Unset,
    /// The issue holds a value the map lists.
    Mapped(&'m FieldOption<S>),
    /// The issue holds something the map cannot interpret, with the finding that says why.
    Unreadable(FieldFinding),
}

/// Interpret `read` by `map`, design §4.3 and §4.6.
///
/// A [`FieldRead::Conflict`] is unreadable with `labels_conflict`, and a value whose name the
/// map does not list is unreadable with `option_unmapped`. The value is matched by its option
/// name only; its field and option ids play no part.
pub fn read_value<'m, S: Scale>(map: &'m FieldMap<S>, read: &FieldRead) -> ReadValue<'m, S> {
    match read {
        FieldRead::Unset => ReadValue::Unset,
        FieldRead::Conflict(_) => ReadValue::Unreadable(FieldFinding::LabelsConflict),
        FieldRead::Value(value) => match map.option(&value.name) {
            Some(option) => ReadValue::Mapped(option),
            None => ReadValue::Unreadable(FieldFinding::OptionUnmapped),
        },
    }
}

/// One drift finding between a map and the options the tracker lists.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Drift {
    /// What drifted.
    pub finding: FieldFinding,
    /// The option concerned, by name; `None` for a finding about the whole field.
    pub option: Option<String>,
}

/// Compare `map` with the options the tracker lists for its field, design §4.3.
///
/// Options are compared by name. The findings come in this order:
/// 1. one `field_recreated` if any observed value carries a field id other than the one recorded
///    for the map (field source only; the field is still the one of the mapped name);
/// 2. `option_missing` for each map option, in config order, that no observed value names;
/// 3. `option_unmapped` for each observed value, in observed order, whose name the map does not
///    list.
///
/// A matching set gives no findings. A renamed option shows as the old name missing and the new
/// one unmapped; telling it apart as `option_renamed` needs the recorded option ids, which this
/// function does not have.
pub fn findings<S: Scale>(map: &FieldMap<S>, observed: &[FieldValue]) -> Vec<Drift> {
    let mut drift = Vec::new();
    if let FieldSource::Field { field_id, .. } = map.source() {
        let recreated = observed
            .iter()
            .any(|value| value.field_id.is_some_and(|id| id != *field_id));
        if recreated {
            drift.push(Drift {
                finding: FieldFinding::FieldRecreated,
                option: None,
            });
        }
    }
    for option in map.options() {
        if !observed.iter().any(|value| value.name == option.name) {
            drift.push(Drift {
                finding: FieldFinding::OptionMissing,
                option: Some(option.name.clone()),
            });
        }
    }
    for value in observed {
        if map.option(&value.name).is_none() {
            drift.push(Drift {
                finding: FieldFinding::OptionUnmapped,
                option: Some(value.name.clone()),
            });
        }
    }
    drift
}

/// The scope a fallback covers, design §4.5.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FallbackScope {
    /// The whole role, for example when its field is missing.
    Role,
    /// One option, by name.
    Option(String),
    /// One issue's value.
    Issue,
}

/// The value a consumer uses inside a fallback `scope`, design §4.5: the map's default,
/// whatever the scope.
pub fn fallback<S: Scale>(map: &FieldMap<S>, scope: &FallbackScope) -> S {
    match scope {
        FallbackScope::Role | FallbackScope::Option(_) | FallbackScope::Issue => map.default(),
    }
}
