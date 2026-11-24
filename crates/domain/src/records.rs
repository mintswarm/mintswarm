//! The records the pipeline's tools write, field for field and in the Python tools' key order.
//!
//! Every struct rejects unknown fields. A key the Python always writes, even as `null`, is always
//! serialised; a key it only sometimes writes is skipped when absent.

use std::fmt;

use serde::de::{self, Deserializer, MapAccess, SeqAccess, Visitor};
use serde::ser::{SerializeMap, SerializeSeq, Serializer};
use serde::{Deserialize, Serialize};

use crate::ids::{IssueNumber, PrNumber, RevId};

/// A JSON value that keeps its object keys in order and its integers apart from its floats.
///
/// Used where a record embeds a value the Python leaves open, and to compare records by structure.
/// An integer is an `Int` when it fits in `i64`, and a `UInt` only above `i64::MAX`, so each
/// integer has one form.
#[derive(Clone, Debug, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    Int(i64),
    UInt(u64),
    Float(f64),
    String(String),
    Array(Vec<Json>),
    Object(Vec<(String, Json)>),
}

impl Serialize for Json {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Json::Null => serializer.serialize_unit(),
            Json::Bool(b) => serializer.serialize_bool(*b),
            Json::Int(i) => serializer.serialize_i64(*i),
            Json::UInt(u) => serializer.serialize_u64(*u),
            Json::Float(f) => serializer.serialize_f64(*f),
            Json::String(s) => serializer.serialize_str(s),
            Json::Array(items) => {
                let mut seq = serializer.serialize_seq(Some(items.len()))?;
                for item in items {
                    seq.serialize_element(item)?;
                }
                seq.end()
            }
            Json::Object(entries) => {
                let mut map = serializer.serialize_map(Some(entries.len()))?;
                for (key, value) in entries {
                    map.serialize_entry(key, value)?;
                }
                map.end()
            }
        }
    }
}

struct JsonVisitor;

impl<'de> Visitor<'de> for JsonVisitor {
    type Value = Json;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a JSON value")
    }

    fn visit_unit<E: de::Error>(self) -> Result<Json, E> {
        Ok(Json::Null)
    }

    fn visit_none<E: de::Error>(self) -> Result<Json, E> {
        Ok(Json::Null)
    }

    fn visit_some<D: Deserializer<'de>>(self, d: D) -> Result<Json, D::Error> {
        Json::deserialize(d)
    }

    fn visit_bool<E: de::Error>(self, b: bool) -> Result<Json, E> {
        Ok(Json::Bool(b))
    }

    fn visit_i64<E: de::Error>(self, i: i64) -> Result<Json, E> {
        Ok(Json::Int(i))
    }

    fn visit_u64<E: de::Error>(self, u: u64) -> Result<Json, E> {
        Ok(i64::try_from(u).map_or(Json::UInt(u), Json::Int))
    }

    fn visit_f64<E: de::Error>(self, f: f64) -> Result<Json, E> {
        Ok(Json::Float(f))
    }

    fn visit_str<E: de::Error>(self, s: &str) -> Result<Json, E> {
        Ok(Json::String(s.to_owned()))
    }

    fn visit_string<E: de::Error>(self, s: String) -> Result<Json, E> {
        Ok(Json::String(s))
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Json, A::Error> {
        let mut items = Vec::new();
        while let Some(item) = seq.next_element()? {
            items.push(item);
        }
        Ok(Json::Array(items))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Json, A::Error> {
        let mut entries = Vec::new();
        while let Some(entry) = map.next_entry()? {
            entries.push(entry);
        }
        Ok(Json::Object(entries))
    }
}

impl<'de> Deserialize<'de> for Json {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Json, D::Error> {
        deserializer.deserialize_any(JsonVisitor)
    }
}

/// A file bound by its sha256.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Bound {
    pub path: String,
    pub sha256: String,
}

/// A ref as given, and the commit it resolved to.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BaseRef {
    #[serde(rename = "ref")]
    pub reference: String,
    pub commit: RevId,
}

/// The result of a floor rule, and of a floor as a whole.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Verdict {
    Hit,
    Clean,
}

/// One floor rule's result.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuleResult {
    pub rule: String,
    pub result: Verdict,
    pub reasons: Vec<String>,
}

/// An approval mode, as `pipeline.plan_approval.mode` and `pipeline.landing.mode` name it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Always,
    Shadow,
    Judged,
    Never,
}

/// The plan floor's verdict, `plan-approval/floor.py`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FloorRecord {
    pub tool: String,
    pub version: u32,
    pub plan: Bound,
    pub issue_document: Option<Bound>,
    pub analysis: Option<Bound>,
    pub component: String,
    pub base: BaseRef,
    pub config: String,
    pub mode: Mode,
    pub verdict: Verdict,
    pub rules: Vec<RuleResult>,
    pub not_evaluated: Vec<String>,
}

impl FloorRecord {
    pub const TOOL: &'static str = "mintswarm-plan-floor";
    pub const VERSION: u32 = 2;
}

/// Whether an approval goes ahead on its own or waits for a human.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Decision {
    Auto,
    Human,
}

/// Whether a decision layer's input could be used.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LayerStatus {
    Usable,
    Unusable,
}

/// An assessment's confidence band.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Band {
    Safe,
    LikelySafe,
    NeedsReview,
    Uncertain,
    Blocked,
}

/// What a plan decision decides about.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Subject {
    pub issue: IssueNumber,
    pub component: String,
    pub plan: Bound,
    pub base: BaseRef,
}

/// The floor verdict as the decision read it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FloorLayer {
    pub status: LayerStatus,
    pub problems: Vec<String>,
    pub path: String,
    pub sha256: Option<String>,
    pub verdict: Option<Verdict>,
    pub rules: Vec<RuleResult>,
    pub not_evaluated: Vec<String>,
}

/// The assess harness and model the base config names.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExpectedJudge {
    pub harness: Option<String>,
    pub model: Option<String>,
}

/// A doubt an assessment raises.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Doubt {
    pub statement: String,
    pub consequence: String,
    pub resolution: String,
}

/// The assessment as the decision read it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JudgementLayer {
    pub status: LayerStatus,
    pub problems: Vec<String>,
    pub stage: String,
    pub path: String,
    pub sha256: Option<String>,
    pub artifact: String,
    pub harness: String,
    pub model: String,
    pub expected: ExpectedJudge,
    pub as_configured: bool,
    pub confidence: Option<u32>,
    pub band: Option<Band>,
    pub hard_triggers: Vec<String>,
    pub doubts: Vec<Doubt>,
    pub reasons: Vec<String>,
}

/// The two layers of a plan decision.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Layers {
    pub floor: FloorLayer,
    pub judgement: JudgementLayer,
}

/// The plan approval decision, `plan-approval/decision.py`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DecisionRecord {
    pub tool: String,
    pub version: u32,
    pub approval: String,
    pub subject: Subject,
    pub mode: Mode,
    pub min_confidence: u32,
    pub layers: Layers,
    pub judged_decision: Decision,
    pub decision: Decision,
    pub reasons: Vec<String>,
}

impl DecisionRecord {
    pub const TOOL: &'static str = "mintswarm-approval-decision";
    pub const VERSION: u32 = 2;
}

/// The kind of a kept round.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RoundKind {
    Gates,
    Review,
}

/// A file of a kept round.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoundFile {
    pub name: String,
    pub sha256: String,
}

/// A kept gates or review round, `landing/keep-round.py`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoundRecord {
    pub tool: String,
    pub version: u32,
    pub kind: RoundKind,
    pub round: u32,
    pub head: RevId,
    pub kept_at: String,
    pub files: Vec<RoundFile>,
}

impl RoundRecord {
    pub const TOOL: &'static str = "mintswarm-keep-round";
    pub const VERSION: u32 = 2;
}

/// Where a check comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CheckSource {
    CheckRun,
    Status,
}

/// A check run or commit status on the PR's head.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Check {
    pub name: String,
    pub conclusion: Option<String>,
    pub source: CheckSource,
    pub app: Option<String>,
}

/// A gates round's verdict.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GateVerdict {
    Passed,
    Failed,
}

/// A kept gates round as the facts read it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GateRound {
    pub round: u32,
    pub commit: RevId,
    pub current: bool,
    pub verdict: GateVerdict,
    /// Names of the failed tests; `null` for a test with no string name.
    pub failed_tests: Vec<Option<String>>,
    pub reason: Option<String>,
    pub files: Vec<RoundFile>,
}

/// A review round's verdict.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewVerdict {
    Pass,
    ChangesRequired,
}

/// Review findings counted by severity.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FindingCounts {
    pub blocker: u32,
    pub major: u32,
    pub minor: u32,
    pub nit: u32,
}

/// A kept review round as the facts read it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewRound {
    pub round: u32,
    pub commit: RevId,
    pub current: bool,
    pub verdict: ReviewVerdict,
    pub findings: FindingCounts,
    pub files: Vec<RoundFile>,
}

/// Where a steer was given.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SteerKind {
    IssueComment,
    PrComment,
    Review,
    ReviewComment,
}

/// Why an item counts as a steer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SteerVia {
    Account,
    Marker,
}

/// A steer given after the approval.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Steer {
    pub kind: SteerKind,
    pub author: String,
    pub time: String,
    pub edited_at: Option<String>,
    pub url: String,
    pub via: SteerVia,
}

/// A record file bound by its sha256, with the record as read.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BoundRecord {
    pub path: String,
    pub sha256: String,
    pub record: Json,
}

/// An assessment bound by its sha256, with its envelope's figures.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssessmentRef {
    pub path: String,
    pub sha256: String,
    pub confidence: Option<u32>,
    pub band: Option<Band>,
}

/// The plan approval the landing facts start from.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Approval {
    pub approver: String,
    pub approved_at: String,
    pub approval_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision: Option<BoundRecord>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kickoff_floor: Option<BoundRecord>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assessment: Option<AssessmentRef>,
}

/// The landing facts, `landing/collect-facts.py`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FactsRecord {
    pub tool: String,
    pub version: u32,
    pub pr: PrNumber,
    pub issue: IssueNumber,
    pub head: RevId,
    pub base: RevId,
    pub fetched_at: String,
    pub checks: Vec<Check>,
    pub gate_rounds: Vec<GateRound>,
    pub review_rounds: Vec<ReviewRound>,
    pub steers: Vec<Steer>,
    pub review_requests: Vec<String>,
    pub approval: Approval,
}

impl FactsRecord {
    pub const TOOL: &'static str = "mintswarm-landing-facts";
    pub const VERSION: u32 = 3;
}

/// The facts file a landing floor read.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FactsBound {
    pub path: String,
    pub sha256: String,
    pub checks: Vec<Check>,
}

/// The landing floor's verdict, `landing/floor.py`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LandingFloorRecord {
    pub tool: String,
    pub version: u32,
    pub plan: Bound,
    pub facts: FactsBound,
    pub component: String,
    pub base: BaseRef,
    pub head: BaseRef,
    pub merge_base: RevId,
    pub config: String,
    pub mode: Mode,
    pub verdict: Verdict,
    pub rules: Vec<RuleResult>,
    pub not_evaluated: Vec<String>,
    /// The diff's overruns of the plan's line limits, total then production; a doubt for the
    /// landing decision, never a rule hit. Empty when `footprint` is a hit.
    pub evidence: Vec<String>,
}

impl LandingFloorRecord {
    pub const TOOL: &'static str = "mintswarm-landing-floor";
    pub const VERSION: u32 = 2;
}
