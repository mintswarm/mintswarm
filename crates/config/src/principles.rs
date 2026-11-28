//! `principles` in the repository config: the principle packs the repository uses, how each is
//! pinned, and its overrides of single principles.
//!
//! The section follows `schemas/settings.schema.yaml` in mintswarm/tenets, where the format is
//! defined while it settles. What needs the pack itself is not checked here: that an override
//! lowering a principle's declared strength gives a reason, that an overridden principle exists,
//! and that a parameter set here is not one the pack derives from the repository.

use std::collections::BTreeMap;

use serde_json::{Map, Value};

use crate::schema::{kind, known, mapping, string};
use crate::{Error, Path};

/// A principle's strength, which sets what breaking it costs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Strength {
    /// A blocker finding in review and a hard trigger at landing.
    Must,
    /// A major finding; the PR says why it departs.
    Should,
    /// A preference: a minor finding at most.
    May,
}

impl Strength {
    const NAMES: [(&'static str, Strength); 3] = [
        ("must", Strength::Must),
        ("should", Strength::Should),
        ("may", Strength::May),
    ];
}

/// Where a pack comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PackSource {
    /// An archive or repository outside this repository, read-only, with the pin it is used at.
    External {
        /// The `https://`, `git+https://` or `git+ssh://` URL.
        url: String,
        /// The tag or hash it is pinned to.
        pin: Pin,
    },
    /// A directory of this repository, read at the job's base commit.
    Local {
        /// The directory, relative to the repository root.
        path: String,
    },
}

/// How an external pack is pinned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pin {
    /// A release tag, resolved to a hash and locked when the settings change lands.
    Tag(String),
    /// A commit or a `sha256:` archive digest.
    Hash(String),
}

/// One principle's override.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Override {
    /// The principle applies at this strength; `reason` is required when it is lower than the
    /// declared one, which the principles check verifies against the pack.
    Strength {
        /// The strength it applies at.
        strength: Strength,
        /// Why, if given.
        reason: Option<String>,
    },
    /// The principle is not applied at all.
    Skip {
        /// Why.
        reason: String,
    },
}

/// One entry of `principles.sources`.
#[derive(Debug, Clone, PartialEq)]
pub struct PrincipleSource {
    /// The qualified pack name, `@<org>/<repo>:<pack>`.
    pub pack: String,
    /// Where it comes from.
    pub source: PackSource,
    /// Whether the pack applies; `true` unless set.
    pub enabled: bool,
    /// How strongly the pack weighs against others, from 0 to 1, if set.
    pub bias: Option<f64>,
    /// Values for the pack's parameters that the repository records nowhere else.
    pub parameters: Map<String, Value>,
    /// Overrides, keyed by the principle's slug within the pack.
    pub overrides: BTreeMap<String, Override>,
}

/// Validate `principles`, absent meaning no packs.
pub(crate) fn check_principles(
    value: Option<&Value>,
    path: &Path,
) -> Result<Vec<PrincipleSource>, Error> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let map = mapping(value, path)?;
    known(map, path, &["sources"])?;
    let Some(sources) = map.get("sources") else {
        return Ok(Vec::new());
    };
    let path = path.key("sources");
    let list = sources
        .as_array()
        .ok_or_else(|| Error::schema(&path, format!("must be a list, not {}", kind(sources))))?;
    let mut checked: Vec<PrincipleSource> = Vec::with_capacity(list.len());
    for (index, entry) in list.iter().enumerate() {
        let source = check_source(entry, &path.index(index))?;
        if checked.iter().any(|earlier| earlier.pack == source.pack) {
            return Err(Error::schema(
                &path.index(index).key("pack"),
                format!("{} is listed twice", source.pack),
            ));
        }
        checked.push(source);
    }
    Ok(checked)
}

fn check_source(value: &Value, path: &Path) -> Result<PrincipleSource, Error> {
    let map = mapping(value, path)?;
    known(
        map,
        path,
        &[
            "pack",
            "from",
            "tag",
            "hash",
            "enabled",
            "bias",
            "parameters",
            "principles",
        ],
    )?;
    let pack_path = path.key("pack");
    let pack = string(required(map, "pack", path)?, &pack_path)?;
    if !is_pack_name(pack) {
        return Err(Error::schema(
            &pack_path,
            format!("must be @<org>/<repo>:<pack>, not {pack}"),
        ));
    }
    let from_path = path.key("from");
    let from = string(required(map, "from", path)?, &from_path)?;
    let tag = optional_nonempty(map, "tag", path)?;
    let hash = optional_nonempty(map, "hash", path)?;
    let source = if let Some(local) = from.strip_prefix("repo:") {
        if local.is_empty() || local.starts_with('/') || local.split('/').any(|part| part == "..") {
            return Err(Error::schema(
                &from_path,
                "must name a directory inside the repository after repo:",
            ));
        }
        if tag.is_some() || hash.is_some() {
            return Err(Error::schema(
                path,
                "a repo: pack is read at the job's base commit and takes no tag or hash",
            ));
        }
        PackSource::Local {
            path: local.to_owned(),
        }
    } else if ["https://", "git+https://", "git+ssh://"]
        .iter()
        .any(|scheme| from.starts_with(scheme))
    {
        let pin = match (tag, hash) {
            (Some(tag), None) => Pin::Tag(tag.to_owned()),
            (None, Some(hash)) if is_hash(hash) => Pin::Hash(hash.to_owned()),
            (None, Some(hash)) => {
                return Err(Error::schema(
                    &path.key("hash"),
                    format!("must be a 40- or 64-digit hex commit or sha256:<64 hex>, not {hash}"),
                ));
            }
            _ => {
                return Err(Error::schema(
                    path,
                    "an external pack is pinned by exactly one of tag and hash",
                ));
            }
        };
        PackSource::External {
            url: from.to_owned(),
            pin,
        }
    } else {
        return Err(Error::schema(
            &from_path,
            format!("must start with https://, git+https://, git+ssh:// or repo:, not {from}"),
        ));
    };
    let enabled = match map.get("enabled") {
        None => true,
        Some(Value::Bool(enabled)) => *enabled,
        Some(other) => {
            return Err(Error::schema(
                &path.key("enabled"),
                format!("must be a bool, not {}", kind(other)),
            ));
        }
    };
    let bias = match map.get("bias") {
        None => None,
        Some(value) => match value.as_f64() {
            Some(bias) if (0.0..=1.0).contains(&bias) => Some(bias),
            _ => {
                return Err(Error::schema(
                    &path.key("bias"),
                    format!("must be a number from 0 to 1, not {value}"),
                ));
            }
        },
    };
    let parameters = match map.get("parameters") {
        None => Map::new(),
        Some(value) => check_parameters(value, &path.key("parameters"))?,
    };
    let overrides = match map.get("principles") {
        None => BTreeMap::new(),
        Some(value) => check_overrides(value, &path.key("principles"))?,
    };
    Ok(PrincipleSource {
        pack: pack.to_owned(),
        source,
        enabled,
        bias,
        parameters,
        overrides,
    })
}

fn check_parameters(value: &Value, path: &Path) -> Result<Map<String, Value>, Error> {
    let map = mapping(value, path)?;
    for (name, value) in map {
        let entry = path.key(name);
        if !is_identifier(name, b'_') {
            return Err(Error::schema(
                &entry,
                "a parameter name is lower-case letters, digits and _, starting with a letter",
            ));
        }
        if !matches!(value, Value::String(_) | Value::Bool(_))
            && value.as_i64().is_none()
            && value.as_u64().is_none()
        {
            return Err(Error::schema(
                &entry,
                format!(
                    "must be a string, an integer or a bool, not {}",
                    kind(value)
                ),
            ));
        }
    }
    Ok(map.clone())
}

fn check_overrides(value: &Value, path: &Path) -> Result<BTreeMap<String, Override>, Error> {
    let map = mapping(value, path)?;
    let mut overrides = BTreeMap::new();
    for (slug, value) in map {
        let entry = path.key(slug);
        if !is_identifier(slug, b'-') {
            return Err(Error::schema(
                &entry,
                "a principle slug is lower-case letters, digits and -, starting with a letter",
            ));
        }
        let fields = mapping(value, &entry)?;
        known(fields, &entry, &["strength", "skip", "reason"])?;
        let reason = optional_nonempty(fields, "reason", &entry)?.map(str::to_owned);
        let skip = match fields.get("skip") {
            None => false,
            Some(Value::Bool(true)) => true,
            Some(other) => {
                return Err(Error::schema(
                    &entry.key("skip"),
                    format!("must be true when given, not {other}"),
                ));
            }
        };
        let strength = match fields.get("strength") {
            None => None,
            Some(value) => Some(
                value
                    .as_str()
                    .and_then(|name| {
                        Strength::NAMES
                            .iter()
                            .find(|(known, _)| *known == name)
                            .map(|(_, strength)| *strength)
                    })
                    .ok_or_else(|| {
                        Error::schema(
                            &entry.key("strength"),
                            format!("must be must, should or may, not {value}"),
                        )
                    })?,
            ),
        };
        let checked = match (skip, strength, reason) {
            (true, Some(_), _) => {
                return Err(Error::schema(
                    &entry,
                    "a skipped principle takes no strength",
                ));
            }
            (true, None, Some(reason)) => Override::Skip { reason },
            (true, None, None) => {
                return Err(Error::schema(
                    &entry.key("reason"),
                    "is missing: skipping a principle needs a reason",
                ));
            }
            (false, Some(strength), reason) => Override::Strength { strength, reason },
            (false, None, _) => {
                return Err(Error::schema(&entry, "an override sets a strength or skip"));
            }
        };
        overrides.insert(slug.clone(), checked);
    }
    Ok(overrides)
}

fn required<'a>(map: &'a Map<String, Value>, key: &str, path: &Path) -> Result<&'a Value, Error> {
    map.get(key)
        .ok_or_else(|| Error::schema(&path.key(key), "is missing"))
}

fn optional_nonempty<'a>(
    map: &'a Map<String, Value>,
    key: &str,
    path: &Path,
) -> Result<Option<&'a str>, Error> {
    match map.get(key) {
        None => Ok(None),
        Some(value) => match string(value, &path.key(key))? {
            "" => Err(Error::schema(&path.key(key), "must not be empty")),
            text => Ok(Some(text)),
        },
    }
}

/// `[a-z][a-z0-9<extra>]*`.
fn is_identifier(text: &str, extra: u8) -> bool {
    let mut bytes = text.bytes();
    bytes.next().is_some_and(|first| first.is_ascii_lowercase())
        && bytes.all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == extra)
}

/// `@<org>/<repo>:<pack>`: org `[a-z0-9][a-z0-9-]*`, repo `[a-z0-9][a-z0-9._-]*`, pack a slug.
fn is_pack_name(text: &str) -> bool {
    let Some(rest) = text.strip_prefix('@') else {
        return false;
    };
    let Some((org, rest)) = rest.split_once('/') else {
        return false;
    };
    let Some((repo, pack)) = rest.split_once(':') else {
        return false;
    };
    let name = |part: &str, extra: &[u8]| {
        let mut bytes = part.bytes();
        bytes
            .next()
            .is_some_and(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
            && bytes.all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || extra.contains(&b))
    };
    name(org, b"-") && name(repo, b"._-") && is_identifier(pack, b'-')
}

/// A 40- or 64-digit lower-case hex commit, or `sha256:` and 64 lower-case hex digits.
fn is_hash(text: &str) -> bool {
    let hex = |part: &str| {
        part.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    };
    match text.strip_prefix("sha256:") {
        Some(digest) => digest.len() == 64 && hex(digest),
        None => (text.len() == 40 || text.len() == 64) && hex(text),
    }
}

#[cfg(test)]
mod tests {
    use super::{is_hash, is_identifier, is_pack_name};

    #[test]
    fn pack_names() {
        assert!(is_pack_name("@mintswarm/tenets:rust"));
        assert!(is_pack_name("@acme-co/my.packs_2:misra-c"));
        assert!(!is_pack_name("mintswarm/tenets:rust"));
        assert!(!is_pack_name("@mintswarm/tenets"));
        assert!(!is_pack_name("@Mintswarm/tenets:rust"));
        assert!(!is_pack_name("@mintswarm/tenets:rust/x"));
        assert!(!is_pack_name("@/tenets:rust"));
    }

    #[test]
    fn hashes() {
        assert!(is_hash(&"a".repeat(40)));
        assert!(is_hash(&"0".repeat(64)));
        assert!(is_hash(&format!("sha256:{}", "f".repeat(64))));
        assert!(!is_hash(&"a".repeat(41)));
        assert!(!is_hash(&"A".repeat(40)));
        assert!(!is_hash(&format!("sha256:{}", "f".repeat(40))));
    }

    #[test]
    fn identifiers() {
        assert!(is_identifier("no-unwrap-outside-tests", b'-'));
        assert!(is_identifier("max_width", b'_'));
        assert!(!is_identifier("1st", b'-'));
        assert!(!is_identifier("", b'-'));
        assert!(!is_identifier("max_width", b'-'));
    }
}
