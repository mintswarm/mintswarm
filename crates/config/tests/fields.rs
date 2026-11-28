//! `tracker.fields`: each role's entry is checked through its registered descriptor.

#![cfg(test)]

use mintswarm_config::{Error, load_repo_config, load_repo_config_with_roles};
use mintswarm_domain::role::{Preset, ReadPoint, Role, RoleDescriptor, SizeRole, roles};
use mintswarm_domain::scale::{Scale, Size};

/// The sections every config needs, after `tracker`.
const REST: &str = "
components:
  domain: { paths: [\"crates/**\"] }
pipeline:
  plan_approval:
    mode: always
    limits: { files: 10, lines: 500, new_files: 3 }
  landing:
    mode: always
    footprint: { lines_factor: 1.5 }
";

/// The design §4.2 `size` entry, by name and without an id.
const SIZE: &str = "
    size:
      source: field
      field: Effort
      label: \"effort:{option}\"
      default: M
      options:
        XS:  { range: XS }
        S:   { range: S }
        M:   { range: M }
        L:   { range: L }
        XL:  { range: XL }
        2XL: { range: 2XL }
        3XL: { range: 3XL }
        4XL: { range: 4XL }
        Low:    { range: XS-S,   write: false }
        Medium: { range: M-L,    write: false }
        High:   { range: XL-4XL, write: false, reads_as: 2XL }
";

/// A config whose `tracker.fields` holds `entries`.
fn config(entries: &str) -> String {
    format!("tracker:\n  fields:{entries}{REST}")
}

/// The design §4.2 entry with `from` replaced by `to`.
fn size_with(from: &str, to: &str) -> String {
    assert!(SIZE.contains(from), "{from}");
    config(&SIZE.replacen(from, to, 1))
}

fn schema_error(result: Result<impl std::fmt::Debug, Error>) -> Error {
    match result {
        Err(error @ Error::Schema { .. }) => error,
        other => panic!("expected a schema error, got {other:?}"),
    }
}

/// A three-level test scale, standing in for a second role's scale.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Level {
    Low,
    Mid,
    High,
}

impl Scale for Level {
    const ALL: &'static [Self] = &[Self::Low, Self::Mid, Self::High];

    fn name(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Mid => "mid",
            Self::High => "high",
        }
    }
}

struct LevelRole;

impl Role for LevelRole {
    type Scale = Level;
    const NAME: &'static str = "level";
    const READ_POINT: ReadPoint = ReadPoint::WorkHead;
    const PRESETS: &'static [Preset] = &[];
}

fn with_level() -> Vec<&'static dyn RoleDescriptor> {
    let mut registry = roles().to_vec();
    registry.push(&LevelRole);
    registry
}

/// A `level` entry whose options are all `write: false` and leave `high` uncovered.
const LEVEL: &str = "
    level:
      source: labels
      label: \"level:{option}\"
      default: mid
      options:
        lower: { range: low, write: false }
        upper: { range: mid, write: false }
";

#[test]
fn design_size_block_loads_and_reads_github_options() {
    let loaded = load_repo_config(&config(SIZE)).unwrap();
    let map = loaded.field_map::<SizeRole>().expect("size is mapped");
    assert!(!map.is_read_only());
    assert_eq!(map.default(), Size::M);
    let reads = |name: &str| map.option(name).unwrap().reads_as;
    assert_eq!(reads("Low"), Size::S);
    assert_eq!(reads("Medium"), Size::L);
    assert_eq!(reads("High"), Size::Xxl);
    assert_eq!(map.options().len(), 11);
}

#[test]
fn absent_and_off_entries_have_no_map() {
    let absent = load_repo_config(&config(" {}\n")).unwrap();
    assert!(absent.field_map::<SizeRole>().is_none());
    let off = load_repo_config(&config("\n    size: { source: off }\n")).unwrap();
    assert!(off.field_map::<SizeRole>().is_none());
}

#[test]
fn invariant_breaks_fail_with_role_and_option_named() {
    let reversed = schema_error(load_repo_config(&size_with(
        "M:   { range: M }",
        "M:   { range: M-S }",
    )));
    assert_eq!(reversed.path(), "tracker.fields.size.options.M");
    assert!(reversed.message().contains("\"M\""), "{reversed}");

    let overlap = schema_error(load_repo_config(&size_with(
        "XL:  { range: XL }",
        "XL:  { range: L-XL }",
    )));
    assert_eq!(overlap.path(), "tracker.fields.size.options.XL");
    assert!(
        overlap.message().contains("\"L\"") && overlap.message().contains("\"XL\""),
        "{overlap}"
    );

    let uncovered = schema_error(load_repo_config(&size_with(
        "        XL:  { range: XL }\n",
        "",
    )));
    assert_eq!(uncovered.path(), "tracker.fields.size.options");
    assert!(uncovered.message().contains("XL"), "{uncovered}");
}

#[test]
fn read_only_role_loads_with_uncovered_values() {
    let loaded = load_repo_config_with_roles(&config(LEVEL), &with_level()).unwrap();
    let map = loaded.field_map::<LevelRole>().expect("level is mapped");
    assert!(map.is_read_only());
    assert_eq!(map.default(), Level::Mid);
    assert_eq!(map.option("upper").unwrap().reads_as, Level::Mid);
}

#[test]
fn registered_roles_report_their_read_points() {
    let registry = with_level();
    let point = |name: &str| {
        registry
            .iter()
            .find(|role| role.name() == name)
            .map(|role| role.read_point())
    };
    assert_eq!(point("size"), Some(ReadPoint::PinnedBase));
    assert_eq!(point("level"), Some(ReadPoint::WorkHead));
}

#[test]
fn labels_mode_needs_the_option_placeholder() {
    let missing = schema_error(load_repo_config(&size_with(
        "source: field\n      field: Effort\n      label: \"effort:{option}\"",
        "source: labels\n      label: \"effort\"",
    )));
    assert_eq!(missing.path(), "tracker.fields.size.label");

    let absent = schema_error(load_repo_config(&size_with(
        "source: field\n      field: Effort\n      label: \"effort:{option}\"",
        "source: labels",
    )));
    assert_eq!(absent.path(), "tracker.fields.size.label");

    let labels = size_with("source: field\n      field: Effort", "source: labels");
    let map = load_repo_config(&labels)
        .unwrap()
        .field_map::<SizeRole>()
        .unwrap();
    assert_eq!(
        map.field_ref().source,
        mintswarm_domain::field::FieldSource::Labels {
            label: "effort:{option}".into()
        }
    );
}

#[test]
fn a_second_scale_loads_beside_size_without_loader_change() {
    let text = config(&format!("{SIZE}{LEVEL}"));
    let loaded = load_repo_config_with_roles(&text, &with_level()).unwrap();
    assert!(loaded.field_map::<SizeRole>().is_some());
    assert!(loaded.field_map::<LevelRole>().is_some());

    let unregistered = schema_error(load_repo_config(&text));
    assert_eq!(unregistered.path(), "tracker.fields.level");

    let priority = schema_error(load_repo_config(&config(
        "\n    priority: { source: off }\n",
    )));
    assert_eq!(priority.path(), "tracker.fields.priority");
    assert!(priority.message().contains("size"), "{priority}");
}

#[test]
fn fields_are_named_never_given_by_id() {
    let with_id = schema_error(load_repo_config(&size_with(
        "field: Effort\n",
        "field: Effort\n      field_id: 50124379\n",
    )));
    assert_eq!(with_id.path(), "tracker.fields.size.field_id");
    assert_eq!(with_id.message(), "unknown key");

    let unnamed = schema_error(load_repo_config(&size_with(
        "field: Effort\n",
        "field: \" \"\n",
    )));
    assert_eq!(unnamed.path(), "tracker.fields.size.field");
}

#[test]
fn entry_keys_and_types_are_checked() {
    let cases = [
        (
            size_with("source: field", "source: project"),
            "tracker.fields.size.source",
        ),
        (
            size_with("default: M", "default: 3"),
            "tracker.fields.size.default",
        ),
        (
            size_with("default: M", "default: medium"),
            "tracker.fields.size.default",
        ),
        (
            size_with("{ range: XS }", "{ range: XS, write: yes please }"),
            "tracker.fields.size.options.XS.write",
        ),
        (
            size_with("{ range: XS }", "{ size: XS }"),
            "tracker.fields.size.options.XS.size",
        ),
        (
            size_with("{ range: XS }", "{ write: true }"),
            "tracker.fields.size.options.XS.range",
        ),
        (
            config("\n    size: { source: off, default: [M] }\n"),
            "tracker.fields.size.default",
        ),
        (
            config("\n    size: { source: field, field: Effort, default: M, options: {} }\n"),
            "tracker.fields.size.options",
        ),
        (
            config("\n    size: { source: field, field: Effort, options: {} }\n"),
            "tracker.fields.size.default",
        ),
        (config(" [size]\n"), "tracker.fields"),
    ];
    for (text, path) in cases {
        let error = schema_error(load_repo_config(&text));
        assert_eq!(error.path(), path, "{error}");
    }
}
