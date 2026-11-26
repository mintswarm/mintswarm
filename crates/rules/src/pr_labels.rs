//! The pull request labelling rule: which labels a factory PR should carry.
//!
//! Two label families are mirrored on a PR. A component label names each component whose
//! `paths` a changed path matches, unless the path also matches `shared`. A band label,
//! `msw-verdict:<band>`, names the PR's band when one is given. [`changes`] compares those with
//! the PR's current labels and the repository's labels, and returns the labels to add, remove
//! and create. Labels outside the two families are never touched.
//!
//! Label names compare ASCII case-insensitively, as GitHub compares them. The band label's
//! template, colour and description are those `msw-state.py` writes, so the two writers agree.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use mintswarm_domain::field::LabelPattern;
use mintswarm_domain::records::Band;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::exit::Unusable;
use crate::glob;
use crate::plan_decision::envelope::BANDS;
use crate::plan_floor::config::{layout, load};

/// The colour of a component label the rule creates.
pub const COMPONENT_COLOUR: &str = "1d76db";
/// The description of a component label the rule creates.
pub const COMPONENT_DESCRIPTION: &str = "mintswarm component";
/// The placeholder a component label pattern holds once, for the component's name.
pub const COMPONENT_PLACEHOLDER: &str = "{name}";
/// The component label pattern in field mode, and when `tracker.components` names no source.
pub const FIELD_COMPONENT_LABEL: &str = "component:{name}";
/// The band label template, as `msw-state.py` writes it.
pub const VERDICT_LABEL: &str = "msw-verdict:{value}";
/// The placeholder the band label template holds once, for the band's identifier.
pub const VERDICT_PLACEHOLDER: &str = "{value}";
/// The colour of a band label the rule creates, as `msw-state.py` creates it.
pub const VERDICT_COLOUR: &str = "ededed";
/// The description of a band label the rule creates, as `msw-state.py` creates it.
pub const VERDICT_DESCRIPTION: &str = "mintswarm verdict";
/// The prefix of mintswarm's own `msw-<family>:<value>` labels, which are never component labels.
pub const FAMILY_PREFIX: &str = "msw-";
/// The longest label name, in characters.
pub const MAX_NAME: usize = 50;
/// The longest label description, in characters.
pub const MAX_DESCRIPTION: usize = 100;

/// Why the rule cannot give the changes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    /// The config at the base does not load, or its `tracker.components.source` is neither
    /// `labels` nor `field`.
    Config(Unusable),
    /// The component label pattern does not hold `{name}` exactly once with text around it, it
    /// builds band labels or other `msw-<family>:<value>` labels, or labels mode names no
    /// pattern.
    Pattern {
        /// The pattern as configured, empty when absent.
        pattern: String,
    },
    /// The label the component pattern builds for a configured component is a band label or
    /// another `msw-<family>:<value>` label.
    ComponentLabel {
        /// The component's name.
        component: String,
        /// The label built for it.
        label: String,
    },
    /// A label name is longer than [`MAX_NAME`] characters.
    NameTooLong {
        /// The name.
        name: String,
    },
    /// A label description is longer than [`MAX_DESCRIPTION`] characters.
    DescriptionTooLong {
        /// The description.
        description: String,
    },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Config(unusable) => write!(f, "{unusable}"),
            Error::Pattern { pattern } => write!(
                f,
                "config: tracker.components.label {pattern:?} must hold {COMPONENT_PLACEHOLDER} \
                 exactly once, with text around it, and must not build \
                 {FAMILY_PREFIX}<family>:<value> labels"
            ),
            Error::ComponentLabel { component, label } => write!(
                f,
                "config: tracker.components.label builds {label:?} for component {component:?}, \
                 which is a {FAMILY_PREFIX}<family>:<value> label"
            ),
            Error::NameTooLong { name } => {
                write!(f, "label {name:?} is longer than {MAX_NAME} characters")
            }
            Error::DescriptionTooLong { description } => write!(
                f,
                "label description {description:?} is longer than {MAX_DESCRIPTION} characters"
            ),
        }
    }
}

impl std::error::Error for Error {}

impl From<Error> for Unusable {
    fn from(error: Error) -> Self {
        match error {
            Error::Config(unusable) => unusable,
            other => Unusable::new(other.to_string()),
        }
    }
}

/// The rule's view of the repository config.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct View {
    /// `components.<name>.paths` of every component, empty when absent.
    pub paths: BTreeMap<String, Vec<String>>,
    /// `shared`, empty when absent.
    pub shared: Vec<String>,
    /// The component label pattern, holding `{name}` once.
    pub label: String,
}

/// The rule's view of the config `bytes` at the base.
///
/// The pattern is `tracker.components.label` when `tracker.components.source` is `labels`, and
/// [`FIELD_COMPONENT_LABEL`] when it is `field` or absent.
pub fn view(bytes: &[u8]) -> Result<View, Error> {
    let config = load(bytes).map_err(Error::Config)?;
    let value = config.value();
    let (paths, shared) = layout(value);
    let block = value.get("tracker").and_then(|t| t.get("components"));
    let label = match block.and_then(|b| b.get("source")).and_then(Value::as_str) {
        None | Some("field") => FIELD_COMPONENT_LABEL.to_owned(),
        Some("labels") => block
            .and_then(|b| b.get("label"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        Some(_) => {
            return Err(Error::Config(Unusable::new(
                "config: tracker.components.source must be labels or field",
            )));
        }
    };
    component_pattern(&label, &paths)?;
    Ok(View {
        paths,
        shared,
        label,
    })
}

/// `label` split at its one `{name}`; an error unless it holds `{name}` exactly once with some
/// text around it, since a bare `{name}` would match every label, and unless the labels it
/// builds, for any name and for each component in `paths`, are neither band labels nor other
/// `msw-<family>:<value>` labels, which the rule never reads as component labels.
fn component_pattern<'l>(
    label: &'l str,
    paths: &BTreeMap<String, Vec<String>>,
) -> Result<LabelPattern<'l>, Error> {
    let verdict = verdict_pattern()?;
    let own = |built: &str| verdict.option_of(built).is_some() || is_family(built);
    let pattern = LabelPattern::with_placeholder(label, COMPONENT_PLACEHOLDER)
        .filter(|pattern| !(pattern.prefix().is_empty() && pattern.suffix().is_empty()))
        .filter(|pattern| !own(&pattern.label("x")))
        .ok_or_else(|| Error::Pattern {
            pattern: label.to_owned(),
        })?;
    for component in paths.keys() {
        let built = pattern.label(component);
        if own(&built) {
            return Err(Error::ComponentLabel {
                component: component.clone(),
                label: built,
            });
        }
    }
    Ok(pattern)
}

/// [`VERDICT_LABEL`] split at its one [`VERDICT_PLACEHOLDER`].
fn verdict_pattern() -> Result<LabelPattern<'static>, Error> {
    LabelPattern::with_placeholder(VERDICT_LABEL, VERDICT_PLACEHOLDER).ok_or_else(|| {
        Error::Pattern {
            pattern: VERDICT_LABEL.to_owned(),
        }
    })
}

/// A label to create on the repository.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Create {
    /// The label's name.
    pub name: String,
    /// The label's colour, six hexadecimal digits without `#`.
    #[serde(rename = "color")]
    pub colour: String,
    /// The label's description.
    pub description: String,
}

impl Create {
    /// A label to create; an error when the name is longer than [`MAX_NAME`] characters or the
    /// description longer than [`MAX_DESCRIPTION`].
    pub fn new(
        name: impl Into<String>,
        colour: impl Into<String>,
        description: impl Into<String>,
    ) -> Result<Self, Error> {
        let name = check_name(name.into())?;
        let description = description.into();
        if description.chars().count() > MAX_DESCRIPTION {
            return Err(Error::DescriptionTooLong { description });
        }
        Ok(Create {
            name,
            colour: colour.into(),
            description,
        })
    }
}

fn check_name(name: String) -> Result<String, Error> {
    if name.chars().count() > MAX_NAME {
        return Err(Error::NameTooLong { name });
    }
    Ok(name)
}

/// The labels to add to a PR, remove from it and create on the repository, each sorted in byte
/// order and free of duplicates.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Changes {
    /// Labels to add to the PR.
    pub add: Vec<String>,
    /// Labels to remove from the PR.
    pub remove: Vec<String>,
    /// Labels to create on the repository before adding them, sorted by name.
    pub create: Vec<Create>,
}

/// The band's identifier, such as `likely-safe`.
fn band_id(band: Band) -> &'static str {
    BANDS
        .iter()
        .find(|entry| entry.1 == band)
        .map_or("", |entry| entry.0)
}

/// Whether `label` is one of mintswarm's own `msw-<family>:<value>` labels, ignoring ASCII case.
fn is_family(label: &str) -> bool {
    label
        .get(..FAMILY_PREFIX.len())
        .is_some_and(|head| head.eq_ignore_ascii_case(FAMILY_PREFIX))
        && label[FAMILY_PREFIX.len()..].contains(':')
}

/// What one wanted label is: its name as built and how to create it.
struct Wanted {
    name: String,
    colour: &'static str,
    description: &'static str,
}

/// The label changes for a PR whose diff touches `paths`, with the optional `band`, the PR's
/// current `pr_labels` and the repository's `repo_labels`.
///
/// - A path that matches `shared` touches no component; any other path touches every component
///   whose `paths` it matches. A path in no component is not an error.
/// - A PR label the component pattern matches is kept when it names a touched component,
///   ignoring ASCII case, and removed otherwise.
/// - With a band, band labels naming another band are removed; without one, band labels are left
///   alone. Band labels and other `msw-<family>:<value>` labels are never read as component
///   labels, whatever the pattern.
/// - Of kept labels that differ only in case, one stays: the name as built when present, else the
///   first in byte order.
/// - A touched component's label and the band label are added unless a kept label names them.
/// - `create` holds the added labels the repository lacks, ignoring ASCII case.
pub fn changes<P, L, R>(
    view: &View,
    paths: &[P],
    band: Option<Band>,
    pr_labels: &[L],
    repo_labels: &[R],
) -> Result<Changes, Error>
where
    P: AsRef<str>,
    L: AsRef<str>,
    R: AsRef<str>,
{
    let component = component_pattern(&view.label, &view.paths)?;
    let verdict = verdict_pattern()?;

    let mut touched = BTreeSet::new();
    for path in paths.iter().map(AsRef::as_ref) {
        if !glob::matching(&view.shared, path, false).is_empty() {
            continue;
        }
        for (name, globs) in &view.paths {
            if !glob::matching(globs, path, false).is_empty() {
                touched.insert(name.as_str());
            }
        }
    }
    let band = band.map(band_id);

    // The wanted labels, keyed by their ASCII-lowercase names.
    let mut wanted: BTreeMap<String, Wanted> = BTreeMap::new();
    for name in &touched {
        let name = component.label(name);
        wanted.insert(
            name.to_ascii_lowercase(),
            Wanted {
                name,
                colour: COMPONENT_COLOUR,
                description: COMPONENT_DESCRIPTION,
            },
        );
    }
    if let Some(band) = band {
        let name = verdict.label(band);
        wanted.insert(
            name.to_ascii_lowercase(),
            Wanted {
                name,
                colour: VERDICT_COLOUR,
                description: VERDICT_DESCRIPTION,
            },
        );
    }

    let present: BTreeSet<&str> = pr_labels.iter().map(AsRef::as_ref).collect();
    let mut remove = BTreeSet::new();
    // The kept labels, grouped by their ASCII-lowercase names, each group in byte order.
    let mut kept: BTreeMap<String, Vec<&str>> = BTreeMap::new();
    for &label in &present {
        // The band template goes first and mintswarm's own families are never component
        // labels, so a pattern such as `msw-{name}` cannot reach them.
        let keep = if let Some(value) = verdict.option_of(label) {
            match band {
                Some(band) => band.eq_ignore_ascii_case(value),
                None => continue,
            }
        } else if is_family(label) {
            continue;
        } else if let Some(name) = component.option_of(label) {
            touched.iter().any(|t| t.eq_ignore_ascii_case(name))
        } else {
            continue;
        };
        if keep {
            kept.entry(label.to_ascii_lowercase())
                .or_default()
                .push(label);
        } else {
            remove.insert(label);
        }
    }
    for (key, group) in &kept {
        let built = wanted.get(key).map(|w| w.name.as_str());
        let stays = built
            .filter(|built| group.contains(built))
            .or_else(|| group.first().copied());
        remove.extend(group.iter().copied().filter(|label| Some(*label) != stays));
    }

    let repo: BTreeSet<String> = repo_labels
        .iter()
        .map(|label| label.as_ref().to_ascii_lowercase())
        .collect();
    let mut add = BTreeSet::new();
    let mut create = Vec::new();
    for (key, want) in wanted {
        if kept.contains_key(&key) {
            continue;
        }
        let name = check_name(want.name)?;
        if !repo.contains(&key) {
            create.push(Create::new(name.clone(), want.colour, want.description)?);
        }
        add.insert(name);
    }
    create.sort();
    create.dedup();

    Ok(Changes {
        add: add.into_iter().collect(),
        remove: remove.into_iter().map(str::to_owned).collect(),
        create,
    })
}
