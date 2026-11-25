//! The comments `decision.py` prints: the auto-approval comment and the shadow record, with
//! their rows, as `comment`, `shadow_comment` and their helpers write them.

use mintswarm_domain::records::{
    Decision, DecisionRecord, FloorLayer, Json, JudgementLayer, LayerStatus, Verdict,
};

use super::{AUTHORITY_TRIGGERS, code_list, span, span_of};
use crate::json;
use crate::plan_decision::envelope::band_name;
use crate::plan_decision::hit_rules;
use crate::plan_floor::footprint::{Dependency, Footprint};
use crate::plan_floor::rules::{Package, get, parse_screen};
use crate::plan_floor::{RULES, mode_name};

fn is_usable(layer: &FloorLayer) -> bool {
    layer.status == LayerStatus::Usable
}

/// The floor's result in a row: clean with the rule count, the hit rules, or `unusable`.
pub fn floor_line(layer: &FloorLayer) -> String {
    if !is_usable(layer) {
        return "unusable".to_owned();
    }
    if layer.verdict == Some(Verdict::Clean) {
        format!("clean, {} of {} rules", layer.rules.len(), RULES.len())
    } else {
        format!("hit on {}", code_list(hit_rules(layer)))
    }
}

/// ` · requires the maintainer: ` and the authority triggers the layer lists, in code spans; empty
/// when it lists none.
pub(crate) fn requires_maintainer(layer: &JudgementLayer) -> String {
    let listed = code_list(
        layer
            .hard_triggers
            .iter()
            .map(String::as_str)
            .filter(|t| AUTHORITY_TRIGGERS.contains(t)),
    );
    if listed.is_empty() {
        String::new()
    } else {
        format!(" · requires the maintainer: {listed}")
    }
}

/// The band and score, then ` · requires the maintainer: …` for the authority triggers listed,
/// the threshold and the reason; or `unusable` with the threshold.
pub fn confidence_line(layer: &JudgementLayer, min_confidence: u32) -> String {
    let (LayerStatus::Usable, Some(band), Some(confidence)) =
        (layer.status, layer.band, layer.confidence)
    else {
        return format!("unusable (min_confidence {min_confidence})");
    };
    let requires = requires_maintainer(layer);
    let reason = layer.reasons.first().map_or("", String::as_str);
    format!(
        "`{} · {confidence}`{requires} (min_confidence {min_confidence}), {}",
        band_name(band),
        span(reason)
    )
}

/// The Confidence, Hard triggers and Doubt rows; `suffix` follows the Confidence row.
pub fn judgement_rows(layer: &JudgementLayer, min_confidence: u32, suffix: &str) -> Vec<String> {
    let mut rows = vec![format!(
        "- **Confidence:** {}{suffix}",
        confidence_line(layer, min_confidence)
    )];
    if layer.status != LayerStatus::Usable {
        return rows;
    }
    let triggers = code_list(
        layer
            .hard_triggers
            .iter()
            .map(String::as_str)
            .filter(|t| !AUTHORITY_TRIGGERS.contains(t)),
    );
    let triggers = if triggers.is_empty() {
        "none".to_owned()
    } else {
        triggers
    };
    rows.push(format!("- **Hard triggers:** {triggers}"));
    for doubt in &layer.doubts {
        rows.push(format!(
            "- **Doubt:** {}; if wrong: {}; resolved by: {}",
            span(&doubt.statement),
            span(&doubt.consequence),
            span(&doubt.resolution)
        ));
    }
    rows
}

/// A value of the screen record as a code span: `unknown` when absent or null, JSON's words for
/// booleans.
pub fn evidence(value: Option<&Json>) -> String {
    match value {
        None | Some(Json::Null) => "`unknown`".to_owned(),
        Some(Json::Bool(true)) => "`true`".to_owned(),
        Some(Json::Bool(false)) => "`false`".to_owned(),
        Some(other) => span_of(other),
    }
}

/// `container[key]` when `container` is an object, else none.
fn field<'j>(container: Option<&'j Json>, key: &str) -> Option<&'j Json> {
    match container {
        Some(Json::Object(entries)) => get(entries, key),
        _ => None,
    }
}

/// The package's disqualifiers, each with its advisory ids or the name it imitates, or `none`.
fn disqualifier_text(package: &Package<'_>) -> String {
    let problems = get(package.fields, "problems");
    let mut parts = Vec::new();
    for item in &package.disqualifiers {
        let mut text = format!("`{item}`");
        if *item == "advisory" {
            let ids: Vec<String> = match field(problems, "advisories") {
                Some(Json::Array(advisories)) => advisories
                    .iter()
                    .filter_map(|a| field(Some(a), "id"))
                    .filter(|id| **id != Json::Null)
                    .map(span_of)
                    .collect(),
                _ => Vec::new(),
            };
            if !ids.is_empty() {
                text.push_str(&format!(" ({})", ids.join(", ")));
            }
        } else if *item == "typosquat"
            && let Some(of) = field(problems, "typosquat_of").filter(|v| **v != Json::Null)
        {
            text.push_str(&format!(" of {}", span_of(of)));
        }
        parts.push(text);
    }
    if parts.is_empty() {
        "none".to_owned()
    } else {
        parts.join(", ")
    }
}

/// What a Package row shows for one declared package.
enum Screened<'p, 'j> {
    /// The screen record's entry for it.
    Entry(&'p Package<'j>),
    /// Why there is none.
    Missing(&'static str),
}

/// The Package row of one declared package: its screen results, or why there are none.
fn package_row(dependency: &Dependency, package: Screened<'_, '_>) -> String {
    let head = format!(
        "- **Package:** {} in {}",
        span(&dependency.name),
        span(&dependency.manifest)
    );
    let package = match package {
        Screened::Missing(why) => return format!("{head}: {why}"),
        Screened::Entry(package) => package,
    };
    let mut status = format!(
        "`{}`",
        if package.unscreened {
            "unscreened"
        } else {
            "screened"
        }
    );
    if package.unscreened {
        status.push_str(&format!(" ({})", span(package.reason)));
    }
    let entry = |key: &str| get(package.fields, key);
    let dependencies = entry("dependencies");
    let author = entry("author");
    let establishment = entry("establishment");
    format!(
        "{head}, version {}; status {status}; disqualifiers: {}; dependencies {} direct, {} \
         transitive; publisher {}, publisher_changed {}; age_days {}, releases {}",
        evidence(entry("version")),
        disqualifier_text(package),
        evidence(field(dependencies, "direct")),
        evidence(field(dependencies, "transitive")),
        evidence(field(author, "publisher")),
        evidence(field(author, "publisher_changed")),
        evidence(field(establishment, "age_days")),
        evidence(field(establishment, "releases")),
    )
}

/// One Package row per declared package of `footprint` (none when it is unreadable), from the
/// screen record's text `screen`, or `None` when no record is given: `package_rows`.
pub fn package_rows(screen: Option<&str>, footprint: Option<&Footprint>) -> Vec<String> {
    let Some(declared) = footprint
        .map(|f| &f.new_dependencies)
        .filter(|d| !d.is_empty())
    else {
        return Vec::new();
    };
    let Some(text) = screen else {
        return declared
            .iter()
            .map(|dep| package_row(dep, Screened::Missing("no screen record (`--screen`)")))
            .collect();
    };
    let record = json::read(text).ok();
    let packages = record.as_ref().map(parse_screen);
    let Some(Ok(mut listed)) = packages else {
        return declared
            .iter()
            .map(|dep| package_row(dep, Screened::Missing("the screen record is not usable")))
            .collect();
    };
    let mut rows = Vec::with_capacity(declared.len());
    for dep in declared {
        let at = listed
            .iter()
            .position(|p| (p.name, p.manifest) == (dep.name.as_str(), dep.manifest.as_str()));
        match at {
            None => rows.push(package_row(
                dep,
                Screened::Missing("not in the screen record"),
            )),
            Some(at) => {
                let package = listed.remove(at);
                rows.push(package_row(dep, Screened::Entry(&package)));
            }
        }
    }
    rows
}

/// The auto-approval comment: `comment`.
pub fn comment(record: &DecisionRecord, packages: &[String]) -> String {
    let floor = &record.layers.floor;
    let judgement = &record.layers.judgement;
    let mut lines = vec![
        "### Plan approved without a human".to_owned(),
        String::new(),
        "mintswarm's plan-approval decision approved the plan for this issue without a human."
            .to_owned(),
        String::new(),
        format!(
            "- **Mode:** `{}`, base commit `{}`",
            mode_name(record.mode),
            record.subject.base.commit.0
        ),
        format!("- **Floor:** {}", floor_line(floor)),
    ];
    lines.extend(packages.iter().cloned());
    lines.extend(judgement_rows(judgement, record.min_confidence, ""));
    lines.push(format!(
        "- **Assessment:** {}, judged by {} with {}",
        span(&judgement.artifact),
        span(&judgement.harness),
        span(&judgement.model)
    ));
    lines.push(String::new());
    lines.push(
        "**To veto:** a trusted author comments on this issue with `veto` as the first line. \
         Until implement finishes, a veto cancels the job and returns the issue to awaiting plan \
         approval; after that, it is a rejection at PR approval."
            .to_owned(),
    );
    lines.push(String::new());
    lines.push("PR approval stays with a human in every mode.".to_owned());
    lines.join("\n") + "\n"
}

/// The footprint in brief: `footprint_brief`.
pub fn footprint_brief(footprint: Option<&Footprint>) -> String {
    match footprint {
        None => "not readable (a `footprint` hit)".to_owned(),
        Some(f) => format!(
            "{} files ({} new), about {} lines, {} declared packages",
            f.files.len(),
            f.new_files.len(),
            f.estimated_lines,
            f.new_dependencies.len()
        ),
    }
}

/// The shadow-record comment: `shadow_comment`.
pub fn shadow_comment(
    record: &DecisionRecord,
    footprint: Option<&Footprint>,
    packages: &[String],
) -> String {
    let floor = &record.layers.floor;
    let judgement = &record.layers.judgement;
    let subject = &record.subject;
    let configured = if judgement.as_configured {
        "as configured".to_owned()
    } else {
        format!(
            "not as configured: expected {} with {}",
            span(judgement.expected.harness.as_deref().unwrap_or("None")),
            span(judgement.expected.model.as_deref().unwrap_or("None"))
        )
    };
    let assess = format!(
        "{} with {}, {configured}; artifact {}",
        span(&judgement.harness),
        span(&judgement.model),
        span(&judgement.artifact)
    );
    let floor_detail = if is_usable(floor) {
        let clean: Vec<&str> = floor
            .rules
            .iter()
            .filter(|r| r.result == Verdict::Clean)
            .map(|r| r.rule.as_str())
            .collect();
        let clean = if clean.is_empty() {
            "none".to_owned()
        } else {
            code_list(clean)
        };
        let code = if floor.verdict == Some(Verdict::Clean) {
            0
        } else {
            1
        };
        let mut detail = format!(
            "{} (`floor.py` exit {code}); clean: {clean}",
            floor_line(floor)
        );
        if !floor.not_evaluated.is_empty() {
            detail.push_str(&format!(
                "; not evaluated: {}",
                code_list(floor.not_evaluated.iter().map(String::as_str))
            ));
        }
        detail
    } else {
        format!("unusable: {}", floor.problems.join("; "))
    };
    let suffix = if judgement.status == LayerStatus::Usable {
        String::new()
    } else {
        format!(": {}", judgement.problems.join("; "))
    };
    let decision = |d| match d {
        Decision::Auto => "auto",
        Decision::Human => "human",
    };
    let mut lines = vec![
        "### Shadow record: plan approval".to_owned(),
        String::new(),
        "Mode `shadow`: this plan goes to the maintainer. Below is what plan approval judged, for \
         comparison with the maintainer's decision."
            .to_owned(),
        String::new(),
        format!(
            "- **Plan:** {}, sha256 `{}`; footprint: {}",
            span(&subject.plan.path),
            subject.plan.sha256,
            footprint_brief(footprint)
        ),
        format!("- **Assess:** stage `assess`, {assess}"),
    ];
    lines.extend(judgement_rows(judgement, record.min_confidence, &suffix));
    lines.push(format!(
        "- **Floor:** at base commit `{}` (`--base` {}): {floor_detail}",
        subject.base.commit.0,
        span(&subject.base.reference)
    ));
    lines.extend(packages.iter().cloned());
    lines.push(format!(
        "- **Would-be decision:** `{}` (`judged_decision`)",
        decision(record.judged_decision)
    ));
    lines.push(format!("- **Decision:** `{}`", decision(record.decision)));
    lines.push("- **Maintainer decision:** pending".to_owned());
    lines.join("\n") + "\n"
}
