//! Roles: their read points, presets and the registry.

use mintswarm_domain::field::{FieldMap, FieldMapSpec, SourceSpec};
use mintswarm_domain::role::{ReadPoint, Role, RoleDescriptor, SizeRole, roles};
use mintswarm_domain::scale::Size;

fn preset_map(name: &str) -> FieldMap<Size> {
    let preset = SizeRole::PRESETS
        .iter()
        .find(|preset| preset.name == name)
        .unwrap_or_else(|| panic!("no {name} preset"));
    let spec = FieldMapSpec {
        source: SourceSpec::Field {
            name: "Effort".into(),
        },
        default: preset.default.into(),
        options: preset.option_specs(),
    };
    FieldMap::new(spec).unwrap_or_else(|err| panic!("{name}: {err}"))
}

#[test]
fn every_registered_role_reports_its_read_point() {
    let registered = roles();
    assert!(!registered.is_empty());
    for role in registered {
        let point = role.read_point();
        assert!(matches!(
            point,
            ReadPoint::PinnedBase | ReadPoint::DefaultHead
        ));
        assert!(!role.presets().is_empty(), "{}", role.name());
    }
    let size = registered
        .iter()
        .find(|role| role.name() == "size")
        .expect("size is registered");
    assert_eq!(size.read_point(), ReadPoint::PinnedBase);
    assert_eq!(SizeRole::READ_POINT, ReadPoint::PinnedBase);
    assert_eq!(SizeRole.name(), SizeRole::NAME);
}

#[test]
fn canonical_preset_writes_each_size() {
    let map = preset_map("canonical");
    assert!(!map.is_read_only());
    assert_eq!(map.default(), Size::M);
    assert_eq!(map.options().len(), 8);
    assert!(map.options().iter().all(|option| option.write));
}

#[test]
fn github_preset_reads_low_medium_high_as_s_l_2xl() {
    let map = preset_map("github");
    assert!(!map.is_read_only());
    let reads = |name: &str| map.option(name).unwrap().reads_as;
    assert_eq!(reads("Low"), Size::S);
    assert_eq!(reads("Medium"), Size::L);
    assert_eq!(reads("High"), Size::Xxl);
}

#[test]
fn descriptor_check_applies_the_role_scale() {
    let preset = SizeRole::PRESETS[0];
    let mut options = preset.option_specs();
    options.pop();
    let spec = FieldMapSpec {
        source: SourceSpec::Field {
            name: "Effort".into(),
        },
        default: "M".into(),
        options,
    };
    assert!(SizeRole.check(spec).is_err());
}
