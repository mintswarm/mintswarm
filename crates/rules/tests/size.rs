//! The lines floor and the effective size.

use mintswarm_domain::field::{
    FieldFinding, FieldMap, FieldMapSpec, FieldRead, FieldValue, OptionSpec, SourceSpec,
};
use mintswarm_domain::scale::Size;
use mintswarm_rules::field::{FallbackScope, ReadValue, fallback, read_value};
use mintswarm_rules::size::{
    EffectiveSize, Fallback, Judged, JudgingStage, Reading, SizeSource, classify, effective_size,
    floor,
};

fn preset() -> FieldMap<Size> {
    let option = |name: &str, range: &str| OptionSpec {
        name: name.into(),
        range: range.into(),
        write: true,
        reads_as: None,
    };
    FieldMap::new(FieldMapSpec {
        source: SourceSpec::Field {
            name: "Effort".into(),
        },
        default: "M".into(),
        options: vec![
            option("Low", "XS-S"),
            option("Medium", "M-L"),
            option("High", "XL-4XL"),
        ],
    })
    .expect("the preset map is valid")
}

fn read(name: &str) -> FieldRead {
    FieldRead::Value(FieldValue {
        field_id: Some(7),
        option_id: None,
        name: name.into(),
    })
}

fn plan(size: Size) -> Option<Judged> {
    Some(Judged {
        stage: JudgingStage::Plan,
        size,
    })
}

fn analyse(size: Size) -> Option<Judged> {
    Some(Judged {
        stage: JudgingStage::Analyse,
        size,
    })
}

fn off(map: &FieldMap<Size>) -> Fallback {
    Fallback {
        active: false,
        value: fallback(map, &FallbackScope::Issue),
    }
}

fn on(map: &FieldMap<Size>) -> Fallback {
    Fallback {
        active: true,
        value: fallback(map, &FallbackScope::Role),
    }
}

#[test]
fn floor_follows_the_band_boundaries() {
    assert_eq!(floor(0), Size::Xs);
    assert_eq!(floor(25), Size::Xs);
    assert_eq!(floor(26), Size::S);
    assert_eq!(floor(75), Size::S);
    assert_eq!(floor(76), Size::M);
    assert_eq!(floor(7000), Size::Xxxl);
    assert_eq!(floor(7001), Size::Xxxxl);
}

#[test]
fn the_floor_raises_a_judged_plan_size_and_both_are_returned() {
    let map = preset();
    let got = effective_size(
        plan(Size::S),
        Some(floor(300)),
        &ReadValue::Unset,
        None,
        off(&map),
    );
    assert_eq!(
        got,
        EffectiveSize {
            size: Size::L,
            source: SizeSource::Plan,
            judged: Some(Size::S),
            floor: Some(Size::L),
        }
    );
    assert_eq!(got.source.to_string(), "plan");
}

#[test]
fn a_judged_size_above_the_floor_stands() {
    let map = preset();
    let got = effective_size(
        plan(Size::Xl),
        Some(floor(30)),
        &ReadValue::Unset,
        None,
        off(&map),
    );
    assert_eq!((got.size, got.source), (Size::Xl, SizeSource::Plan));
    let got = effective_size(analyse(Size::S), None, &ReadValue::Unset, None, off(&map));
    assert_eq!((got.size, got.source), (Size::S, SizeSource::Analyse));
}

#[test]
fn a_persons_option_keeps_a_stage_size_inside_it_and_reads_as_otherwise() {
    let map = preset();
    let medium = read_value(&map, &read("Medium"));
    let inside = effective_size(plan(Size::L), None, &medium, Some("High"), off(&map));
    assert_eq!((inside.size, inside.source), (Size::L, SizeSource::Person));
    let outside = effective_size(plan(Size::Xs), None, &medium, Some("Low"), off(&map));
    assert_eq!(
        (outside.size, outside.source),
        (Size::L, SizeSource::Person)
    );
    assert_eq!(outside.judged, Some(Size::Xs));
    let unjudged = effective_size(None, None, &medium, None, off(&map));
    assert_eq!(
        (unjudged.size, unjudged.source),
        (Size::L, SizeSource::Person)
    );
}

#[test]
fn a_read_equal_to_the_last_option_written_is_an_echo() {
    let map = preset();
    let low = read_value(&map, &read("Low"));
    let option = map.option("Low").unwrap();
    assert_eq!(classify(&low, Some("Low")), Reading::Echo(option));
    assert_eq!(classify(&low, Some("Medium")), Reading::Person(option));
    assert_eq!(classify(&low, None), Reading::Person(option));
    let got = effective_size(
        analyse(Size::Xs),
        None,
        &ReadValue::Mapped(option),
        Some("Low"),
        off(&map),
    );
    assert_eq!((got.size, got.source), (Size::Xs, SizeSource::Analyse));
}

#[test]
fn fallback_gives_the_default_or_unreadable() {
    let map = preset();
    let nothing = effective_size(plan(Size::Xl), None, &ReadValue::Unset, None, on(&map));
    assert_eq!(
        (nothing.size, nothing.source),
        (Size::M, SizeSource::Default)
    );
    let high = read_value(&map, &read("High"));
    let person = effective_size(plan(Size::Xl), None, &high, None, on(&map));
    assert_eq!(
        (person.size, person.source),
        (Size::M, SizeSource::Unreadable)
    );
    let conflict = ReadValue::Unreadable(FieldFinding::LabelsConflict);
    let got = effective_size(plan(Size::Xl), None, &conflict, None, on(&map));
    assert_eq!((got.size, got.source), (Size::M, SizeSource::Unreadable));
    let got = effective_size(plan(Size::Xl), None, &conflict, None, off(&map));
    assert_eq!((got.size, got.source), (Size::M, SizeSource::Unreadable));
}

#[test]
fn no_judged_size_and_no_read_gives_the_default() {
    let map = preset();
    let got = effective_size(None, None, &ReadValue::Unset, None, off(&map));
    assert_eq!(
        got,
        EffectiveSize {
            size: Size::M,
            source: SizeSource::Default,
            judged: None,
            floor: None,
        }
    );
    let names: Vec<_> = [
        SizeSource::Analyse,
        SizeSource::Plan,
        SizeSource::Person,
        SizeSource::Default,
        SizeSource::Unreadable,
    ]
    .iter()
    .map(|source| source.as_str())
    .collect();
    assert_eq!(
        names,
        ["analyse", "plan", "person", "default", "unreadable"]
    );
}
