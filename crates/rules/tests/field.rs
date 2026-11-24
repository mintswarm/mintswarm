//! Writing, reading and checking a role's tracker field.

use mintswarm_domain::field::{
    FieldFinding, FieldMap, FieldMapSpec, FieldRead, FieldValue, OptionSpec, SourceSpec,
};
use mintswarm_domain::scale::{Scale, Size};
use mintswarm_rules::field::{
    Drift, FallbackScope, ReadValue, fallback, findings, read_value, write_option,
};

const FIELD_ID: u64 = 7;

fn option(name: &str, range: &str, write: bool) -> OptionSpec {
    OptionSpec {
        name: name.into(),
        range: range.into(),
        write,
        reads_as: None,
    }
}

fn field_spec(options: Vec<OptionSpec>) -> FieldMapSpec {
    FieldMapSpec {
        source: SourceSpec::Field {
            name: "Effort".into(),
        },
        default: "M".into(),
        options,
    }
}

fn labels_spec(options: Vec<OptionSpec>) -> FieldMapSpec {
    FieldMapSpec {
        source: SourceSpec::Labels {
            label: "effort:{option}".into(),
        },
        default: "M".into(),
        options,
    }
}

/// One written option per size.
fn canonical() -> FieldMap<Size> {
    let options = Size::ALL
        .iter()
        .map(|size| option(size.name(), size.name(), true))
        .collect();
    FieldMap::new(field_spec(options)).expect("the canonical map is valid")
}

fn preset(write: bool) -> FieldMap<Size> {
    FieldMap::new(field_spec(vec![
        option("Low", "XS-S", write),
        option("Medium", "M-L", write),
        option("High", "XL-4XL", write),
    ]))
    .expect("the preset map is valid")
}

fn value(field_id: Option<u64>, option_id: u64, name: &str) -> FieldValue {
    FieldValue {
        field_id,
        option_id: Some(option_id),
        name: name.into(),
    }
}

fn written(map: &FieldMap<Size>, size: Size) -> Option<&str> {
    write_option(map, size).map(|option| option.name.as_str())
}

#[test]
fn write_option_picks_the_written_option_covering_the_value() {
    let map = canonical();
    for &size in Size::ALL {
        assert_eq!(written(&map, size), Some(size.name()));
    }
    let map = preset(true);
    assert_eq!(written(&map, Size::Xs), Some("Low"));
    assert_eq!(written(&map, Size::S), Some("Low"));
    assert_eq!(written(&map, Size::M), Some("Medium"));
    assert_eq!(written(&map, Size::L), Some("Medium"));
    assert_eq!(written(&map, Size::Xl), Some("High"));
    assert_eq!(written(&map, Size::Xxxxl), Some("High"));
}

#[test]
fn write_option_is_none_for_every_value_of_a_read_only_map() {
    let map = preset(false);
    assert!(map.is_read_only());
    for &size in Size::ALL {
        assert_eq!(write_option(&map, size), None, "{size}");
    }
}

#[test]
fn read_value_interprets_each_kind_of_read() {
    let map = preset(true);
    assert_eq!(read_value(&map, &FieldRead::Unset), ReadValue::Unset);
    let medium = FieldRead::Value(value(Some(7), 2, "Medium"));
    assert_eq!(
        read_value(&map, &medium),
        ReadValue::Mapped(map.option("Medium").unwrap())
    );
    let unknown = FieldRead::Value(value(Some(7), 9, "Huge"));
    assert_eq!(
        read_value(&map, &unknown),
        ReadValue::Unreadable(FieldFinding::OptionUnmapped)
    );
}

#[test]
fn read_value_matches_by_name_whatever_the_ids() {
    let map = preset(true);
    let recreated = FieldRead::Value(value(Some(99), 42, "Low"));
    assert_eq!(
        read_value(&map, &recreated),
        ReadValue::Mapped(map.option("Low").unwrap())
    );
}

#[test]
fn read_value_of_a_conflict_is_unreadable_and_fallback_is_the_default() {
    let map = FieldMap::<Size>::new(labels_spec(vec![
        option("Low", "XS-S", true),
        option("Medium", "M-L", true),
        option("High", "XL-4XL", true),
    ]))
    .unwrap();
    let conflict = FieldRead::Conflict(vec![value(None, 1, "Low"), value(None, 3, "High")]);
    assert_eq!(
        read_value(&map, &conflict),
        ReadValue::Unreadable(FieldFinding::LabelsConflict)
    );
    for scope in [
        FallbackScope::Role,
        FallbackScope::Option("Medium".into()),
        FallbackScope::Issue,
    ] {
        assert_eq!(fallback(&map, &scope), Size::M, "{scope:?}");
    }
}

#[test]
fn findings_is_empty_for_a_matching_set() {
    let map = preset(true);
    let observed = [
        value(Some(7), 1, "Low"),
        value(Some(7), 2, "Medium"),
        value(Some(7), 3, "High"),
    ];
    assert_eq!(findings(&map, Some(FIELD_ID), &observed), []);
}

#[test]
fn findings_reports_a_renamed_option_as_missing_then_unmapped() {
    let map = preset(true);
    let observed = [
        value(Some(7), 1, "Low"),
        value(Some(7), 12, "Mid"),
        value(Some(7), 3, "High"),
    ];
    assert_eq!(
        findings(&map, Some(FIELD_ID), &observed),
        [
            Drift {
                finding: FieldFinding::OptionMissing,
                option: Some("Medium".into()),
            },
            Drift {
                finding: FieldFinding::OptionUnmapped,
                option: Some("Mid".into()),
            },
        ]
    );
}

#[test]
fn findings_reports_a_recreated_field_once_and_skips_it_for_labels() {
    let map = preset(true);
    let observed = [
        value(Some(8), 1, "Low"),
        value(Some(8), 2, "Medium"),
        value(Some(8), 3, "High"),
    ];
    assert_eq!(
        findings(&map, Some(FIELD_ID), &observed),
        [Drift {
            finding: FieldFinding::FieldRecreated,
            option: None,
        }]
    );
    let labels = FieldMap::<Size>::new(labels_spec(vec![option("All", "XS-4XL", true)])).unwrap();
    assert_eq!(findings(&labels, None, &[value(None, 1, "All")]), []);
}
