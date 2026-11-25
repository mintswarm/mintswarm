//! The plan-approval decision over a bound input bundle: the pure part of
//! `.mintswarm/dogfood/plan-approval/decision.py`.
//!
//! [`decide`] takes the floor's [`Bundle`], the floor verdict's and the assessment's bytes, and
//! who judged; it recomputes the floor from the bundle, reads both layers as `decision.py` reads
//! them, applies the mode, and gives the `decision.json` record, its text, the outcome and the
//! comment `decision.py` prints. It reads and writes nothing itself.
//!
//! - [`envelope`]: the judgement layer, the assessment's envelope;
//! - [`verdict`]: the floor layer, the floor verdict held to the bundle.
//!
//! Where the floor words a rule's reason itself (a parser's error, see [`crate::plan_floor`]), a
//! verdict `floor.py` wrote with its parser's wording reads here as one whose rules differ.

pub mod envelope;
pub mod verdict;

use mintswarm_domain::ids::IssueNumber;
use mintswarm_domain::records::{
    Decision, DecisionRecord, FloorLayer, JudgementLayer, LayerStatus, Layers, Mode, Subject,
    Verdict as RuleVerdict,
};

use crate::binding::{bind, digest};
use crate::exit::{Outcome, Unusable, Verdict};
use crate::json::{self, Style};
use crate::plan_floor::{self, Bundle, RULES, mode_name};
use crate::render::AUTHORITY_TRIGGERS;
use crate::render::comment::{self, package_rows};

/// A file the decision reads: its bytes, or why there are none.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Input<'a> {
    /// The file does not exist.
    Missing,
    /// The file exists and cannot be read.
    Unreadable,
    /// The file's bytes.
    Bytes(&'a [u8]),
}

/// What the decision reads besides the floor's bundle, which must hold the issue document and
/// the analysis.
#[derive(Clone, Copy, Debug)]
pub struct Inputs<'a> {
    /// The floor's bundle: the config and history at the base, the plan and the documents.
    pub bundle: &'a Bundle,
    /// The issue's number.
    pub issue: u64,
    /// `.mintswarm/job/outputs/floor.json`.
    pub verdict: Input<'a>,
    /// The assessment's path, as the record names it.
    pub assessment_path: &'a str,
    /// The assessment.
    pub assessment: Input<'a>,
    /// Where the assessment was published.
    pub artifact: &'a str,
    /// The harness assess ran on.
    pub harness: &'a str,
    /// The model assess ran on.
    pub model: &'a str,
}

/// The decision reached.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Decided {
    record: DecisionRecord,
    text: String,
    comment: Option<String>,
}

impl Decided {
    /// The record, as `decision.json` holds it.
    pub fn record(&self) -> &DecisionRecord {
        &self.record
    }

    /// The text of `decision.json`, as
    /// `json.dumps(record, indent=2, ensure_ascii=False) + "\n"` writes it.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// The comment `decision.py` prints: the auto-approval comment for `auto`, the shadow record
    /// under `shadow`, otherwise none.
    pub fn comment(&self) -> Option<&str> {
        self.comment.as_deref()
    }

    /// The outcome: [`Verdict::Clean`] (exit 0) for `auto`, [`Verdict::Hit`] (exit 1) for
    /// `human`.
    pub fn outcome(&self) -> Outcome {
        Ok(match self.record.decision {
            Decision::Auto => Verdict::Clean,
            Decision::Human => Verdict::Hit,
        })
    }
}

fn mode_reason(mode: Mode) -> &'static str {
    match mode {
        Mode::Always => "mode always: every plan goes to a human",
        Mode::Shadow => {
            "mode shadow: every plan goes to a human; judged_decision is recorded for comparison"
        }
        Mode::Judged => "mode judged: the decision is judged_decision",
        Mode::Never => "mode never: every plan is approved without a human",
    }
}

fn decision_name(decision: Decision) -> &'static str {
    match decision {
        Decision::Auto => "auto",
        Decision::Human => "human",
    }
}

/// The hit rules of a usable floor layer.
pub(crate) fn hit_rules(layer: &FloorLayer) -> Vec<&str> {
    layer
        .rules
        .iter()
        .filter(|r| r.result == RuleVerdict::Hit)
        .map(|r| r.rule.as_str())
        .collect()
}

/// `judged_decision`, the decision under `mode` and the reasons, as `decide` gives them.
fn judge(
    mode: Mode,
    min_confidence: u64,
    floor: &FloorLayer,
    judgement: &JudgementLayer,
) -> (Decision, Decision, Vec<String>) {
    let usable = floor.status == LayerStatus::Usable;
    let clean = usable && floor.verdict == Some(RuleVerdict::Clean);
    let confidence = u64::from(judgement.confidence.unwrap_or(0));
    let judged_usable = judgement.status == LayerStatus::Usable;
    let confident =
        judged_usable && judgement.hard_triggers.is_empty() && confidence >= min_confidence;
    let judged = if clean && confident && judgement.as_configured {
        Decision::Auto
    } else {
        Decision::Human
    };
    let decision = match mode {
        Mode::Always | Mode::Shadow => Decision::Human,
        Mode::Never => Decision::Auto,
        Mode::Judged => judged,
    };
    let mut reasons = vec![mode_reason(mode).to_owned()];
    reasons.push(if !usable {
        "floor: unusable".to_owned()
    } else if clean {
        format!(
            "floor: clean, {} of {} rules",
            floor.rules.len(),
            RULES.len()
        )
    } else {
        format!("floor: hit on {}", hit_rules(floor).join(", "))
    });
    if judged_usable {
        let band = judgement.band.map_or("", |band| {
            envelope::BANDS
                .iter()
                .find(|b| b.1 == band)
                .map_or("", |b| b.0)
        });
        reasons.push(format!("judgement: confidence {confidence} (band {band})"));
        let (authority, unusable): (Vec<&str>, Vec<&str>) = judgement
            .hard_triggers
            .iter()
            .map(String::as_str)
            .partition(|t| AUTHORITY_TRIGGERS.contains(t));
        if !unusable.is_empty() {
            reasons.push(format!("judgement: hard trigger {}", unusable.join(", ")));
        }
        if !authority.is_empty() {
            reasons.push(format!(
                "judgement: requires the maintainer: {}",
                authority.join(", ")
            ));
        }
        if judgement.hard_triggers.is_empty() && confidence < min_confidence {
            reasons.push(format!(
                "judgement: confidence below min_confidence {min_confidence}"
            ));
        }
    } else {
        reasons.push("judgement: unusable".to_owned());
    }
    if !judgement.as_configured {
        reasons.push("judgement: harness or model not as configured".to_owned());
    }
    reasons.push(format!("judged_decision: {}", decision_name(judged)));
    (judged, decision, reasons)
}

/// The decision `decision.py` reaches on `inputs`, or why they cannot be used.
///
/// The bundle is unusable input when the floor cannot use it (see [`plan_floor::floor`]), when
/// it lacks the issue document or the analysis, which the decision requires, or when the config's
/// `min_confidence` does not fit the record. A missing or unusable verdict or assessment is not:
/// it makes that layer unusable.
pub fn decide(inputs: &Inputs<'_>) -> Result<Decided, Unusable> {
    let bundle = inputs.bundle;
    let (Some(issue_document), Some(analysis)) = (&bundle.issue_document, &bundle.analysis) else {
        return Err(Unusable::new(
            "the decision needs the issue document and the analysis",
        ));
    };
    let evaluation = plan_floor::evaluate(bundle)?;
    let view = &evaluation.view;
    let min_confidence = u32::try_from(view.min_confidence).map_err(|_| {
        Unusable::new("config: pipeline.plan_approval.min_confidence is out of range")
    })?;
    let plan_sha = digest(&bundle.plan.bytes);
    let mode = mode_name(view.mode);
    let binding = verdict::Binding {
        plan: &plan_sha,
        issue_document: &issue_document.sha256,
        analysis: &analysis.sha256,
        screen: bundle.screen.as_ref().map(|s| s.sha256.as_str()),
        commit: &bundle.base.commit.0,
        components: &bundle.components,
        mode,
    };
    let floor = verdict::floor_layer(
        inputs.verdict,
        &binding,
        &evaluation.rules,
        &evaluation.not_evaluated,
    );
    let judgement = envelope::judgement_layer(
        inputs.assessment,
        inputs.assessment_path,
        inputs.artifact,
        inputs.harness,
        inputs.model,
        &view.expected_judge,
    );
    let (judged, decision, reasons) = judge(view.mode, view.min_confidence, &floor, &judgement);
    let record = DecisionRecord {
        tool: DecisionRecord::TOOL.to_owned(),
        version: DecisionRecord::VERSION,
        approval: "plan".to_owned(),
        subject: Subject {
            issue: IssueNumber(inputs.issue),
            components: bundle.components.clone(),
            plan: bind(bundle.plan.path.clone(), &bundle.plan.bytes),
            base: bundle.base.clone(),
        },
        mode: view.mode,
        min_confidence,
        layers: Layers { floor, judgement },
        judged_decision: judged,
        decision,
        reasons,
    };
    let text = json::to_json(&record)
        .map(|tree| json::to_file_text(&tree, Style::DISK))
        .map_err(|_| Unusable::new("the decision record cannot be written as JSON"))?;
    let screen = bundle
        .screen
        .as_ref()
        .map(|item| std::str::from_utf8(&item.bytes).unwrap_or_default());
    let footprint = evaluation.footprint.as_ref();
    let packages = package_rows(screen, footprint);
    let comment = if decision == Decision::Auto {
        Some(comment::comment(&record, &packages))
    } else if view.mode == Mode::Shadow {
        Some(comment::shadow_comment(&record, footprint, &packages))
    } else {
        None
    };
    Ok(Decided {
        record,
        text,
        comment,
    })
}
