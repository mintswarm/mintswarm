//! The rules that read the footprint: `evaluate`, `parse_screen` and `screen_reasons` in
//! `.mintswarm/dogfood/plan-approval/floor.py`.

use mintswarm_domain::records::{Json, RuleResult, Verdict};

use super::config::View;
use super::footprint::Footprint;
use super::{CONFIG, rule};
use crate::glob::matching;
use crate::json;
use crate::py::{flat, lower};

/// The component whose paths `tests-none` allows.
const DOCS: &str = "docs";
const SCREEN_TOOL: &str = "dependency-screen";
const SCREEN_MODE: &str = "plan";
const DISQUALIFIERS: [&str; 4] = ["advisory", "yanked", "deprecated", "typosquat"];

pub(crate) fn get<'j>(entries: &'j [(String, Json)], key: &str) -> Option<&'j Json> {
    entries.iter().find(|(k, _)| k == key).map(|(_, v)| v)
}

fn get_str<'j>(entries: &'j [(String, Json)], key: &str) -> Option<&'j str> {
    match get(entries, key) {
        Some(Json::String(s)) => Some(s),
        _ => None,
    }
}

/// One package of a usable screen record.
pub(crate) struct Package<'j> {
    pub name: &'j str,
    pub manifest: &'j str,
    pub reason: &'j str,
    pub unscreened: bool,
    pub version: Option<&'j str>,
    pub disqualifiers: Vec<&'j str>,
    /// The package's object, for the fields the Package rows show.
    pub fields: &'j [(String, Json)],
}

/// The packages of the screen record `record`, as `parse_screen` reads it, or the
/// `dependency-screen` rule's fixed reason when it is not a record that rule reads.
pub(crate) fn parse_screen(record: &Json) -> Result<Vec<Package<'_>>, &'static str> {
    let Json::Object(entries) = record else {
        return Err("the screen record is not a JSON object");
    };
    if get_str(entries, "tool") != Some(SCREEN_TOOL) {
        return Err("the screen record's tool is not dependency-screen");
    }
    if get(entries, "version") != Some(&Json::Int(1)) {
        return Err("the screen record's version is not 1");
    }
    if get_str(entries, "mode") != Some(SCREEN_MODE) {
        return Err("the screen record's mode is not plan");
    }
    let Some(Json::Array(packages)) = get(entries, "packages") else {
        return Err("the screen record's packages is not a list");
    };
    let mut out = Vec::with_capacity(packages.len());
    for package in packages {
        let fields = match package {
            Json::Object(fields) => Some(fields),
            _ => None,
        };
        let text = |key: &str| fields.and_then(|fields| get_str(fields, key));
        let (Some(fields), Some(name), Some(manifest), Some(reason)) =
            (fields, text("name"), text("manifest"), text("reason"))
        else {
            return Err("a package of the screen record has no string name, manifest or reason");
        };
        let unscreened = match get_str(fields, "status") {
            Some("screened") => false,
            Some("unscreened") => true,
            _ => {
                return Err(
                    "a package of the screen record has a status other than screened or unscreened",
                );
            }
        };
        let mut disqualifiers = Vec::new();
        let usable = match get(fields, "disqualifiers") {
            Some(Json::Array(items)) => items.iter().all(|item| match item {
                Json::String(s) if DISQUALIFIERS.contains(&s.as_str()) => {
                    let fresh = !disqualifiers.contains(&s.as_str());
                    disqualifiers.push(s.as_str());
                    fresh
                }
                _ => false,
            }),
            _ => false,
        };
        if !usable {
            return Err(
                "a package of the screen record has disqualifiers that are not unique values of \
                 advisory, yanked, deprecated, typosquat",
            );
        }
        out.push(Package {
            name,
            manifest,
            reason,
            unscreened,
            version: get_str(fields, "version"),
            disqualifiers,
            fields,
        });
    }
    Ok(out)
}

/// The `dependency-screen` rule's reasons; `screen` is the screen record's text, or `None` when
/// none is given.
fn screen_reasons(footprint: &Footprint, screen: Option<&str>) -> Vec<String> {
    let declared: Vec<(&str, &str)> = footprint
        .new_dependencies
        .iter()
        .map(|d| (d.name.as_str(), d.manifest.as_str()))
        .collect();
    let Some(text) = screen else {
        return match declared.len() {
            0 => Vec::new(),
            1 => vec!["1 declared package and no screen record (--screen)".into()],
            count => vec![format!(
                "{count} declared packages and no screen record (--screen)"
            )],
        };
    };
    let record = match json::read(text) {
        Ok(record) => record,
        Err(_) => return vec!["the screen record is not strict JSON".into()],
    };
    let packages = match parse_screen(&record) {
        Ok(packages) => packages,
        Err(reason) => return vec![reason.into()],
    };
    let mut reasons = Vec::new();
    let mut remaining = declared;
    for package in packages {
        let label = format!("{} ({})", flat(package.name), flat(package.manifest));
        let Some(at) = remaining
            .iter()
            .position(|pair| *pair == (package.name, package.manifest))
        else {
            reasons.push(format!(
                "the screen record lists {label}, which the footprint does not declare: unbound"
            ));
            continue;
        };
        remaining.remove(at);
        if package.unscreened {
            let reason = flat(package.reason);
            let reason = if reason.is_empty() {
                "no reason given".to_owned()
            } else {
                reason
            };
            reasons.push(format!("{label} is unscreened: {reason}"));
        }
        let version = package
            .version
            .map_or_else(|| "of an unknown version".to_owned(), flat);
        for disqualifier in package.disqualifiers {
            reasons.push(format!(
                "{} {version} ({}) is disqualified: {disqualifier}",
                flat(package.name),
                flat(package.manifest)
            ));
        }
    }
    for (name, manifest) in remaining {
        reasons.push(format!(
            "{} ({}) is declared but not screened: the screen record does not list it",
            flat(name),
            flat(manifest)
        ));
    }
    reasons
}

/// The rules from `require-human-paths` to `overridden-assumptions`, in order, as `evaluate`
/// gives them; `screen` is the screen record's text, or `None` when none is given.
pub fn evaluate(
    footprint: &Footprint,
    components: &[String],
    view: &View,
    screen: Option<&str>,
) -> Vec<RuleResult> {
    let declared = footprint.declared();
    let no_paths: Vec<String> = Vec::new();
    let paths_of = |name: &str| view.paths.get(name).unwrap_or(&no_paths);
    let mut results = Vec::new();

    let mut reasons = Vec::new();
    for path in &declared {
        for glob in matching(&view.rh_paths, path, true) {
            reasons.push(format!("{path} matches require_human.paths {glob}"));
        }
    }
    results.push(rule("require-human-paths", reasons));

    let mut reasons = Vec::new();
    for component in components {
        if view.rh_components.contains(component) {
            reasons.push(format!(
                "the issue's component {component} is in require_human.components"
            ));
        }
    }
    for path in &declared {
        for name in &view.rh_components {
            for glob in matching(paths_of(name), path, true) {
                reasons.push(format!(
                    "{path} is in component {name} ({glob}), which requires a human"
                ));
            }
        }
    }
    results.push(rule("require-human-components", reasons));

    let in_shared = |path: &str| !matching(&view.shared, path, false).is_empty();
    let outside = match components {
        [one] => format!("component {one}"),
        _ => format!("components {}", components.join(", ")),
    };
    let reasons = declared
        .iter()
        .filter(|path| {
            components
                .iter()
                .all(|c| matching(paths_of(c), path, false).is_empty())
                && !in_shared(path)
        })
        .map(|path| format!("{path} is outside {outside} and shared"))
        .collect();
    results.push(rule("component-scope", reasons));

    let reasons = declared
        .iter()
        .filter(|path| {
            !in_shared(path)
                && !view
                    .paths
                    .values()
                    .any(|globs| !matching(globs, path, false).is_empty())
        })
        .map(|path| format!("{path} is in no component and not in shared"))
        .collect();
    results.push(rule("no-component", reasons));

    let split = footprint
        .files
        .iter()
        .any(|path| !matching(&view.non_production, path, false).is_empty());
    let (lines_label, lines) = if split {
        ("production_lines", footprint.production_lines)
    } else {
        ("lines", footprint.estimated_lines)
    };
    for (name, key, label, value) in [
        (
            "limit-files",
            "files",
            "files",
            footprint.files.len() as u64,
        ),
        ("limit-lines", "lines", lines_label, lines),
        (
            "limit-new-files",
            "new_files",
            "new_files",
            footprint.new_files.len() as u64,
        ),
    ] {
        let (limit, source) = view.limit(key);
        let source = source.map_or_else(String::new, |s| format!(" ({s})"));
        let reasons = if value > limit {
            vec![format!(
                "{label} is {value}, over the limit of {limit}{source}"
            )]
        } else {
            Vec::new()
        };
        results.push(rule(name, reasons));
    }

    results.push(rule("dependency-screen", screen_reasons(footprint, screen)));

    let reasons = footprint
        .external_steps
        .iter()
        .map(|step| format!("external step: {step}"))
        .collect();
    results.push(rule("external-steps", reasons));

    let mut reasons: Vec<String> = footprint
        .gate_changes
        .iter()
        .map(|change| format!("declared gate change: {change}"))
        .collect();
    let config = lower(CONFIG);
    for path in &declared {
        let lowered = lower(path);
        if lowered == config {
            reasons.push(format!("{path} holds the gate definitions (gates.run)"));
        }
        for (name, word) in &view.gate_words {
            if lowered == lower(word) {
                reasons.push(format!("{path} is named by gate {name}"));
            }
        }
    }
    results.push(rule("gate-changes", reasons));

    let mut reasons = Vec::new();
    if view.tests.as_deref() == Some("none") {
        let docs = paths_of(DOCS);
        for path in &declared {
            if matching(docs, path, false).is_empty() {
                reasons.push(format!(
                    "gates.tests is none and {path} is outside component {DOCS}"
                ));
            }
        }
    }
    results.push(rule("tests-none", reasons));

    let reasons = footprint
        .overridden_assumptions
        .iter()
        .map(|item| {
            format!(
                "overridden assumption: {} ({})",
                item.assumption, item.reason
            )
        })
        .collect();
    results.push(rule("overridden-assumptions", reasons));
    results
}

/// Whether any of `results` is a hit.
pub fn any_hit(results: &[RuleResult]) -> bool {
    results.iter().any(|r| r.result == Verdict::Hit)
}
