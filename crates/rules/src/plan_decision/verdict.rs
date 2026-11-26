//! The floor layer: the floor verdict as `verdict_problems` and `floor_layer` in
//! `.mintswarm/dogfood/plan-approval/decision.py` read it, held to the rules the floor gives for
//! the same bundle.

use mintswarm_domain::records::{FloorLayer, Json, LayerStatus, RuleResult, Verdict};

use super::Input;
use super::envelope::read_keeping_surrogates;
use crate::binding::digest;
use crate::plan_floor::{FOOTPRINT_RULES, RULES, verdict_name};

/// The floor verdict's path, as the layer names it.
pub const VERDICT: &str = ".mintswarm/job/outputs/floor.json";

/// The verdict's keys, in `floor.py`'s order.
const FLOOR_KEYS: [&str; 13] = [
    "tool",
    "version",
    "plan",
    "issue_document",
    "analysis",
    "screen",
    "components",
    "base",
    "config",
    "mode",
    "verdict",
    "rules",
    "not_evaluated",
];
const FLOOR_TOOL: &str = "mintswarm-plan-floor";
const FLOOR_VERSION: i64 = 4;

/// What the verdict is held to: the digests of the plan and the documents the decision was given,
/// the base commit, the components and the mode at the base.
pub(crate) struct Binding<'a> {
    pub plan: &'a str,
    pub issue_document: &'a str,
    pub analysis: &'a str,
    pub screen: Option<&'a str>,
    pub commit: &'a str,
    pub components: &'a [String],
    pub mode: &'a str,
}

fn get<'j>(value: &'j Json, key: &str) -> Option<&'j Json> {
    match value {
        Json::Object(entries) => entries.iter().find(|(k, _)| k == key).map(|(_, v)| v),
        _ => None,
    }
}

fn is(value: Option<&Json>, s: &str) -> bool {
    matches!(value, Some(Json::String(v)) if v == s)
}

/// `bound`: an object whose `sha256` is `sha`.
fn bound(value: &Json, sha: &str) -> bool {
    is(get(value, "sha256"), sha)
}

/// `is_rule`: exactly `rule`, `result` and `reasons`, in order, with string reasons exactly
/// when the result is `hit`.
fn is_rule(item: &Json) -> bool {
    let Json::Object(entries) = item else {
        return false;
    };
    let [(k1, rule), (k2, result), (k3, reasons)] = entries.as_slice() else {
        return false;
    };
    if (k1.as_str(), k2.as_str(), k3.as_str()) != ("rule", "result", "reasons") {
        return false;
    }
    let hit = match result {
        Json::String(r) if r == "hit" => true,
        Json::String(r) if r == "clean" => false,
        _ => return false,
    };
    let Json::Array(reasons) = reasons else {
        return false;
    };
    matches!(rule, Json::String(_))
        && reasons.iter().all(|r| matches!(r, Json::String(_)))
        && hit != reasons.is_empty()
}

/// The problems `verdict_problems` finds with `verdict`, in its order.
fn verdict_problems(verdict: &Json, binding: &Binding<'_>) -> Vec<String> {
    let Json::Object(entries) = verdict else {
        return vec!["the floor verdict is not a JSON object".to_owned()];
    };
    if entries.iter().map(|(k, _)| k.as_str()).ne(FLOOR_KEYS) {
        return vec!["the floor verdict's keys are not floor.py's, in order".to_owned()];
    }
    let field = |index: usize| &entries[index].1;
    let mut problems: Vec<&str> = Vec::new();
    if !is(Some(field(0)), FLOOR_TOOL) || *field(1) != Json::Int(FLOOR_VERSION) {
        problems.push("the floor verdict's tool or version is not floor.py's");
    }
    if !bound(field(2), binding.plan) {
        problems.push("the floor verdict's plan.sha256 does not match the plan");
    }
    if !bound(field(3), binding.issue_document) {
        problems.push("the floor verdict's issue_document does not match --issue-document");
    }
    if !bound(field(4), binding.analysis) {
        problems.push("the floor verdict's analysis does not match --analysis");
    }
    let screen = match binding.screen {
        None => *field(5) == Json::Null,
        Some(sha) => bound(field(5), sha),
    };
    if !screen {
        problems.push("the floor verdict's screen does not match --screen");
    }
    if !is(get(field(7), "commit"), binding.commit) {
        problems.push("the floor verdict's base.commit does not match the base");
    }
    let components = match field(6) {
        Json::Array(names) => {
            names.len() == binding.components.len()
                && names
                    .iter()
                    .zip(binding.components)
                    .all(|(name, given)| is(Some(name), given))
        }
        _ => false,
    };
    if !components {
        problems.push("the floor verdict's components do not match --components");
    }
    if !is(Some(field(9)), binding.mode) {
        problems.push("the floor verdict's mode does not match the base config");
    }
    let mut problems: Vec<String> = problems.into_iter().map(str::to_owned).collect();
    let rules = match field(11) {
        Json::Array(rules) if rules.iter().all(is_rule) => rules,
        _ => {
            problems.push("the floor verdict's rules are not well formed".to_owned());
            return problems;
        }
    };
    let skipped: Vec<&str> = match field(12) {
        Json::Array(items) if items.iter().all(|s| matches!(s, Json::String(_))) => items
            .iter()
            .filter_map(|s| match s {
                Json::String(s) => Some(s.as_str()),
                _ => None,
            })
            .collect(),
        _ => {
            problems.push(
                "the floor verdict's not_evaluated is not a list of rule identifiers".to_owned(),
            );
            return problems;
        }
    };
    let names: Vec<&str> = rules
        .iter()
        .filter_map(|rule| match get(rule, "rule") {
            Some(Json::String(name)) => Some(name.as_str()),
            _ => None,
        })
        .collect();
    let footprint_rules = &RULES[FOOTPRINT_RULES];
    let expected: Vec<&str> = RULES
        .iter()
        .copied()
        .filter(|name| !skipped.contains(name))
        .collect();
    if !(skipped.is_empty() || skipped == footprint_rules) || names != expected {
        problems.push(
            "the floor verdict's rule identifiers are not floor.py's, in order, with none or only \
             the footprint rules not evaluated"
                .to_owned(),
        );
        return problems;
    }
    let result = |rule: &Json| is(get(rule, "result"), "hit");
    if !skipped.is_empty() && !rules.first().is_some_and(result) {
        problems.push("the floor verdict skips rules without a footprint hit".to_owned());
    }
    let hit = rules.iter().any(result);
    if !is(Some(field(10)), if hit { "hit" } else { "clean" }) {
        problems.push("the floor verdict's verdict does not agree with its rules".to_owned());
    }
    problems
}

/// A rule as the verdict holds it.
fn rule_json(rule: &RuleResult) -> Json {
    Json::Object(vec![
        ("rule".into(), Json::String(rule.rule.clone())),
        (
            "result".into(),
            Json::String(verdict_name(rule.result).to_owned()),
        ),
        (
            "reasons".into(),
            Json::Array(rule.reasons.iter().cloned().map(Json::String).collect()),
        ),
    ])
}

/// The floor layer for the verdict `input`, held to `binding` and to the `rules` and
/// `not_evaluated` the floor gives for the same bundle: `floor_layer`.
pub(crate) fn floor_layer(
    input: Input<'_>,
    binding: &Binding<'_>,
    rules: &[RuleResult],
    not_evaluated: &[String],
) -> FloorLayer {
    let mut layer = FloorLayer {
        status: LayerStatus::Unusable,
        problems: Vec::new(),
        path: VERDICT.to_owned(),
        sha256: None,
        verdict: None,
        rules: Vec::new(),
        not_evaluated: Vec::new(),
    };
    let bytes = match input {
        Input::Missing => {
            layer.problems = vec!["the floor verdict is missing".to_owned()];
            return layer;
        }
        Input::Unreadable => {
            layer.problems = vec!["the floor verdict cannot be read".to_owned()];
            return layer;
        }
        Input::Bytes(bytes) => bytes,
    };
    layer.sha256 = Some(digest(bytes));
    let verdict = std::str::from_utf8(bytes)
        .ok()
        .and_then(|text| read_keeping_surrogates(text).ok());
    let Some((verdict, _)) = verdict else {
        layer.problems = vec!["the floor verdict is not strict JSON".to_owned()];
        return layer;
    };
    let mut problems = verdict_problems(&verdict, binding);
    if problems.is_empty() {
        let same_rules =
            get(&verdict, "rules") == Some(&Json::Array(rules.iter().map(rule_json).collect()));
        let same_skipped = get(&verdict, "not_evaluated")
            == Some(&Json::Array(
                not_evaluated.iter().cloned().map(Json::String).collect(),
            ));
        if !(same_rules && same_skipped) {
            problems = vec![
                "the floor verdict's rules differ from what floor.py gives for this plan and base"
                    .to_owned(),
            ];
        }
    }
    if !problems.is_empty() {
        layer.problems = problems;
        return layer;
    }
    let hit = rules.iter().any(|rule| rule.result == Verdict::Hit);
    layer.status = LayerStatus::Usable;
    layer.verdict = Some(if hit { Verdict::Hit } else { Verdict::Clean });
    layer.rules = rules.to_vec();
    layer.not_evaluated = not_evaluated.to_vec();
    layer
}
