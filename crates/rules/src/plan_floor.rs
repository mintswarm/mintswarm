//! The plan floor over a bound input bundle: the pure part of
//! `.mintswarm/dogfood/plan-approval/floor.py`.
//!
//! [`floor`] takes a [`Bundle`], what `floor.py` reads from git and from files, each item held to
//! its sha256, and gives the `floor.json` record with the verdict and each rule's outcome that
//! `floor.py` gives, or [`Unusable`] where `floor.py` exits 2. A reason that carries a parser's
//! error is the floor's own wording, never the parser's. It reads nothing itself: extracting the
//! bundle from git and writing the verdict belong to the caller.
//!
//! - [`footprint`]: the footprint parser and the `footprint` rule's reasons;
//! - [`config`]: the floor's view of the config at the base, with the gate words;
//! - [`rules`]: the rules that read the footprint, with the dependency screen;
//! - [`documents`]: the three document rules.
//!
//! A footprint that is not strict JSON is reported from [`crate::json`]'s structured error, the
//! same reason `floor.py` computes from the decoder's position, its own hooks and its own scan
//! for the first container nested deeper than [`crate::json::MAX_DEPTH`], before which alone it
//! decodes, so both report the first error in the text.

pub mod config;
pub mod documents;
pub mod footprint;
pub mod rules;

use mintswarm_domain::records::{
    BaseRef, Bound, FloorRecord, Json, Mode, RuleResult, Verdict as RuleVerdict,
};

use crate::binding::digest;
use crate::exit::{Outcome, Unusable, Verdict};
use crate::json::{self, Style};

/// The config's path, which the verdict names.
pub const CONFIG: &str = ".mintswarm/config.yaml";

/// The rules that read the footprint, not evaluated after a `footprint` hit: `floor.py`'s
/// `FOOTPRINT_RULES`.
pub const FOOTPRINT_RULES: std::ops::Range<usize> = 1..13;

/// The rules, in the order they are evaluated and reported.
pub const RULES: [&str; 16] = [
    "footprint",
    "require-human-paths",
    "require-human-components",
    "component-scope",
    "no-component",
    "limit-files",
    "limit-lines",
    "limit-new-files",
    "dependency-screen",
    "external-steps",
    "gate-changes",
    "tests-none",
    "overridden-assumptions",
    "blocking-questions",
    "acceptance-criteria",
    "first-of-kind",
];

/// One file of the bundle: its path, its bytes and the sha256 they are held to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    /// The path, as the verdict names it.
    pub path: String,
    /// The bytes.
    pub bytes: Vec<u8>,
    /// The sha256 of the bytes in lowercase hex, as the extractor bound them.
    pub sha256: String,
}

impl Item {
    /// The file at `path` with `bytes`, bound to their digest.
    pub fn new(path: impl Into<String>, bytes: Vec<u8>) -> Self {
        let sha256 = digest(&bytes);
        Item {
            path: path.into(),
            bytes,
            sha256,
        }
    }
}

/// The history from the base: the raw output of
/// `git -c log.showSignature=false log -z --format=%s <commit> --`, held to its sha256.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Subjects {
    /// The bytes, subjects separated by NUL.
    pub bytes: Vec<u8>,
    /// Their sha256 in lowercase hex.
    pub sha256: String,
}

impl Subjects {
    /// The history `bytes`, bound to their digest.
    pub fn new(bytes: Vec<u8>) -> Self {
        let sha256 = digest(&bytes);
        Subjects { bytes, sha256 }
    }

    /// The subjects, decoded as Python's `errors="replace"` decodes them, the empty ones
    /// dropped.
    pub fn decode(&self) -> Vec<String> {
        String::from_utf8_lossy(&self.bytes)
            .split('\0')
            .filter(|subject| !subject.is_empty())
            .map(str::to_owned)
            .collect()
    }
}

/// What the plan floor reads, each content item bound by its sha256.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bundle {
    /// `.mintswarm/config.yaml` at the base commit.
    pub config: Item,
    /// The subjects of every commit reachable from the base commit.
    pub subjects: Subjects,
    /// The base ref as given and the commit it resolves to.
    pub base: BaseRef,
    /// The issue's components, in the issue's order.
    pub components: Vec<String>,
    /// The plan.
    pub plan: Item,
    /// The issue document, when given.
    pub issue_document: Option<Item>,
    /// The analysis, when given.
    pub analysis: Option<Item>,
    /// The screen record, when given.
    pub screen: Option<Item>,
    /// The result record beside the analysis, when there is one; read, and its binding checked,
    /// only with the analysis.
    pub analysis_record: Option<Item>,
    /// The result record beside the plan, when there is one.
    pub plan_record: Option<Item>,
}

impl Bundle {
    /// Each content item with what it is, in a fixed order.
    fn items(&self) -> Vec<(&'static str, &str, &[u8], &str)> {
        let mut items = vec![
            (
                "config",
                self.config.path.as_str(),
                self.config.bytes.as_slice(),
                self.config.sha256.as_str(),
            ),
            (
                "history",
                "of the base",
                self.subjects.bytes.as_slice(),
                self.subjects.sha256.as_str(),
            ),
        ];
        for (what, item) in [
            ("plan", Some(&self.plan)),
            ("issue document", self.issue_document.as_ref()),
            ("analysis", self.analysis.as_ref()),
            ("screen record", self.screen.as_ref()),
            (
                "result record",
                self.analysis.as_ref().and(self.analysis_record.as_ref()),
            ),
            ("result record", self.plan_record.as_ref()),
        ] {
            if let Some(item) = item {
                items.push((
                    what,
                    item.path.as_str(),
                    item.bytes.as_slice(),
                    item.sha256.as_str(),
                ));
            }
        }
        items
    }
}

/// The verdict the floor reaches.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Floor {
    record: FloorRecord,
}

impl Floor {
    /// The verdict record, as `floor.json` holds it.
    pub fn record(&self) -> &FloorRecord {
        &self.record
    }

    /// The outcome: [`Verdict::Hit`] when any rule is hit, else [`Verdict::Clean`].
    pub fn outcome(&self) -> Outcome {
        Ok(match self.record.verdict {
            RuleVerdict::Hit => Verdict::Hit,
            RuleVerdict::Clean => Verdict::Clean,
        })
    }

    /// The text of `floor.json`, as `json.dumps(verdict, indent=2, ensure_ascii=False) + "\n"`
    /// writes it.
    pub fn text(&self) -> String {
        json::to_file_text(&record_json(&self.record), Style::DISK)
    }
}

/// A rule's result: a hit when it has reasons.
pub(crate) fn rule(name: &str, reasons: Vec<String>) -> RuleResult {
    RuleResult {
        rule: name.to_owned(),
        result: if reasons.is_empty() {
            RuleVerdict::Clean
        } else {
            RuleVerdict::Hit
        },
        reasons,
    }
}

fn string(s: &str) -> Json {
    Json::String(s.to_owned())
}

fn bound_json(bound: Option<&Bound>) -> Json {
    bound.map_or(Json::Null, |bound| {
        Json::Object(vec![
            ("path".into(), string(&bound.path)),
            ("sha256".into(), string(&bound.sha256)),
        ])
    })
}

pub(crate) fn verdict_name(verdict: RuleVerdict) -> &'static str {
    match verdict {
        RuleVerdict::Hit => "hit",
        RuleVerdict::Clean => "clean",
    }
}

pub(crate) fn mode_name(mode: Mode) -> &'static str {
    match mode {
        Mode::Always => "always",
        Mode::Shadow => "shadow",
        Mode::Judged => "judged",
        Mode::Never => "never",
    }
}

/// The record as a JSON tree in `floor.py`'s key order.
fn record_json(record: &FloorRecord) -> Json {
    let rules = record
        .rules
        .iter()
        .map(|r| {
            Json::Object(vec![
                ("rule".into(), string(&r.rule)),
                ("result".into(), string(verdict_name(r.result))),
                (
                    "reasons".into(),
                    Json::Array(r.reasons.iter().map(|s| string(s)).collect()),
                ),
            ])
        })
        .collect();
    Json::Object(vec![
        ("tool".into(), string(&record.tool)),
        ("version".into(), Json::Int(i64::from(record.version))),
        ("plan".into(), bound_json(Some(&record.plan))),
        (
            "issue_document".into(),
            bound_json(record.issue_document.as_ref()),
        ),
        ("analysis".into(), bound_json(record.analysis.as_ref())),
        ("screen".into(), bound_json(record.screen.as_ref())),
        (
            "components".into(),
            Json::Array(record.components.iter().map(|s| string(s)).collect()),
        ),
        (
            "base".into(),
            Json::Object(vec![
                ("ref".into(), string(&record.base.reference)),
                ("commit".into(), string(&record.base.commit.0)),
            ]),
        ),
        ("config".into(), string(&record.config)),
        ("mode".into(), string(mode_name(record.mode))),
        ("verdict".into(), string(verdict_name(record.verdict))),
        ("rules".into(), Json::Array(rules)),
        (
            "not_evaluated".into(),
            Json::Array(record.not_evaluated.iter().map(|s| string(s)).collect()),
        ),
    ])
}

fn text<'b>(what: &str, item: &'b Item) -> Result<&'b str, Unusable> {
    std::str::from_utf8(&item.bytes).map_err(|error| {
        Unusable::new(format!(
            "cannot read the {what} {}: it is not UTF-8 at byte {}",
            item.path,
            error.valid_up_to()
        ))
    })
}

fn optional_text<'b>(what: &str, item: Option<&'b Item>) -> Result<Option<&'b str>, Unusable> {
    item.map(|item| text(what, item)).transpose()
}

fn bound(item: &Item) -> Bound {
    Bound {
        path: item.path.clone(),
        sha256: item.sha256.clone(),
    }
}

/// What the floor concludes on a bundle: the config's view, the rules evaluated with their
/// reasons, the rules not evaluated and the footprint, `None` after a `footprint` hit; what
/// `floor.py`'s `conclude` returns, which `decision.py` reuses.
#[derive(Clone, Debug)]
pub(crate) struct Evaluation {
    /// The config's view for the bundle's components.
    pub view: config::View,
    /// The rules evaluated, in [`RULES`] order.
    pub rules: Vec<RuleResult>,
    /// The rules not evaluated, in [`RULES`] order.
    pub not_evaluated: Vec<String>,
    /// The footprint, when it parses.
    pub footprint: Option<footprint::Footprint>,
}

/// The floor's rules on `bundle`, or why it cannot be used; see [`floor`].
pub(crate) fn evaluate(bundle: &Bundle) -> Result<Evaluation, Unusable> {
    for (what, path, bytes, sha256) in bundle.items() {
        if digest(bytes) != sha256 {
            return Err(Unusable::new(format!(
                "the {what} {path} does not match its sha256 binding {sha256}"
            )));
        }
    }
    let plan = text("plan", &bundle.plan)?;
    let view = config::view(&bundle.config.bytes, &bundle.components)?;
    let issue = optional_text("issue document", bundle.issue_document.as_ref())?;
    let analysis = optional_text("analysis", bundle.analysis.as_ref())?;
    let screen = optional_text("screen record", bundle.screen.as_ref())?;
    let analysis_record = match analysis {
        Some(_) => optional_text("result record", bundle.analysis_record.as_ref())?,
        None => None,
    };
    let plan_record = optional_text("result record", bundle.plan_record.as_ref())?;
    let subjects = bundle.subjects.decode();
    let subjects: Vec<&str> = subjects.iter().map(String::as_str).collect();

    let (mut results, footprint) = match footprint::parse(plan) {
        Err(error) => (
            vec![rule("footprint", vec![error.message().to_owned()])],
            None,
        ),
        Ok(parsed) => {
            let mut results = vec![rule("footprint", Vec::new())];
            results.extend(rules::evaluate(&parsed, &bundle.components, &view, screen));
            (results, Some(parsed))
        }
    };
    let documents = documents::Documents {
        plan,
        issue,
        analysis,
        analysis_record,
        plan_record,
    };
    results.extend(documents::evaluate(
        &documents,
        &bundle.components,
        &subjects,
    ));
    let not_evaluated = RULES
        .iter()
        .filter(|name| !results.iter().any(|r| r.rule == **name))
        .map(|name| (*name).to_owned())
        .collect();
    Ok(Evaluation {
        view,
        rules: results,
        not_evaluated,
        footprint,
    })
}

/// The verdict `floor.py` reaches on the inputs `bundle` holds, or why they cannot be used.
///
/// Every item's binding is checked first: an item whose bytes do not have the sha256 it holds is
/// unusable input. Then, as `floor.py` reads them: the plan, the documents and the result records
/// must be UTF-8; the config must load and define each of the bundle's components. The rules run in
/// [`RULES`] order; after a `footprint` hit, the rules that read the footprint are not evaluated.
pub fn floor(bundle: &Bundle) -> Result<Floor, Unusable> {
    let evaluation = evaluate(bundle)?;
    let verdict = if rules::any_hit(&evaluation.rules) {
        RuleVerdict::Hit
    } else {
        RuleVerdict::Clean
    };
    Ok(Floor {
        record: FloorRecord {
            tool: FloorRecord::TOOL.to_owned(),
            version: FloorRecord::VERSION,
            plan: bound(&bundle.plan),
            issue_document: bundle.issue_document.as_ref().map(bound),
            analysis: bundle.analysis.as_ref().map(bound),
            screen: bundle.screen.as_ref().map(bound),
            components: bundle.components.clone(),
            base: bundle.base.clone(),
            config: CONFIG.to_owned(),
            mode: evaluation.view.mode,
            verdict,
            rules: evaluation.rules,
            not_evaluated: evaluation.not_evaluated,
        },
    })
}
