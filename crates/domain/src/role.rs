//! Roles: the uses a tracker field is put to, design §4.1 and §4.2.
//!
//! A role fixes, in code, its scale, the commit its config entry is read at (R2) and the presets
//! setup offers. The config loader finds roles through a registry of [`RoleDescriptor`]s, so
//! adding a role needs only a descriptor and no change to the loader.

use crate::field::{FieldMap, FieldMapError, FieldMapSpec, OptionSpec};
use crate::scale::{Scale, Size};

/// The commit a role's config entry is read at, design §4.2 (R2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ReadPoint {
    /// The job's pinned base, so that a job's stages and its field reads use one mapping.
    PinnedBase,
    /// The default branch's head, as ingest rules are read.
    DefaultHead,
}

/// One option of a [`Preset`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PresetOption {
    /// The tracker option's name.
    pub name: &'static str,
    /// `lo-hi` or one value, by the scale's names.
    pub range: &'static str,
    /// Whether the plane writes this option.
    pub write: bool,
    /// The value a person's choice stands for; the range's upper bound if `None`.
    pub reads_as: Option<&'static str>,
}

/// A set of options setup can offer for a role.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Preset {
    /// The preset's name.
    pub name: &'static str,
    /// The role's default value, by name.
    pub default: &'static str,
    /// The options, in order.
    pub options: &'static [PresetOption],
}

impl Preset {
    /// The options as config option entries.
    pub fn option_specs(&self) -> Vec<OptionSpec> {
        self.options
            .iter()
            .map(|option| OptionSpec {
                name: option.name.to_owned(),
                range: option.range.to_owned(),
                write: option.write,
                reads_as: option.reads_as.map(str::to_owned),
            })
            .collect()
    }
}

/// A role, typed by its scale.
pub trait Role: Sync + 'static {
    /// The scale the role's values are on.
    type Scale: Scale;
    /// The role's name, its key under `tracker.fields`.
    const NAME: &'static str;
    /// The commit the role's config entry is read at.
    const READ_POINT: ReadPoint;
    /// The presets setup offers for the role.
    const PRESETS: &'static [Preset];
}

/// A role as the config loader sees it, without its scale's type.
pub trait RoleDescriptor: Sync {
    /// The role's name, its key under `tracker.fields`.
    fn name(&self) -> &'static str;
    /// The commit the role's config entry is read at.
    fn read_point(&self) -> ReadPoint;
    /// The presets setup offers for the role.
    fn presets(&self) -> &'static [Preset];
    /// Check `spec` against the invariants of design §4.1 on the role's scale.
    fn check(&self, spec: FieldMapSpec) -> Result<(), FieldMapError>;
}

impl<R: Role> RoleDescriptor for R {
    fn name(&self) -> &'static str {
        R::NAME
    }

    fn read_point(&self) -> ReadPoint {
        R::READ_POINT
    }

    fn presets(&self) -> &'static [Preset] {
        R::PRESETS
    }

    fn check(&self, spec: FieldMapSpec) -> Result<(), FieldMapError> {
        FieldMap::<R::Scale>::new(spec).map(drop)
    }
}

/// The `size` role: the canonical size of a piece of work, read at the job's pinned base.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SizeRole;

const fn written(name: &'static str, range: &'static str) -> PresetOption {
    PresetOption {
        name,
        range,
        write: true,
        reads_as: None,
    }
}

/// The `size` presets: the canonical sizes, and GitHub's default Low, Medium and High (B9).
const SIZE_PRESETS: &[Preset] = &[
    Preset {
        name: "canonical",
        default: "M",
        options: &[
            written("XS", "XS"),
            written("S", "S"),
            written("M", "M"),
            written("L", "L"),
            written("XL", "XL"),
            written("2XL", "2XL"),
            written("3XL", "3XL"),
            written("4XL", "4XL"),
        ],
    },
    Preset {
        name: "github",
        default: "M",
        options: &[
            written("Low", "XS-S"),
            written("Medium", "M-L"),
            PresetOption {
                reads_as: Some("2XL"),
                ..written("High", "XL-4XL")
            },
        ],
    },
];

impl Role for SizeRole {
    type Scale = Size;
    const NAME: &'static str = "size";
    const READ_POINT: ReadPoint = ReadPoint::PinnedBase;
    const PRESETS: &'static [Preset] = SIZE_PRESETS;
}

/// The registered roles.
pub fn roles() -> &'static [&'static dyn RoleDescriptor] {
    &[&SizeRole]
}
