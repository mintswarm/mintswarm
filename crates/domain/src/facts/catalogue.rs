//! The attribute groups several fact kinds share, and the value sets of the catalogue's keys.
//!
//! Each group is a struct of flat attributes; a kind holds it as a typed field, so every key is
//! a field of exactly one type. Value sets already defined elsewhere in the domain are reused:
//! [`Band`], [`GateVerdict`], [`SteerKind`] and [`SteerVia`] from [`crate::records`],
//! [`OutcomeStatus`] from [`crate::work`] and [`IssueType`](crate::tracker::IssueType) from
//! [`crate::tracker`].

use std::borrow::Cow;

use schemars::{JsonSchema, Schema, SchemaGenerator};
use serde::de::IntoDeserializer;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;

pub use crate::records::{Band, GateVerdict, SteerKind, SteerVia};
pub use crate::work::OutcomeStatus;

/// Declares a closed value set: a unit enum with its serde spelling and an `ALL` list.
macro_rules! value_enum {
    (
        $(#[$meta:meta])*
        $name:ident, $rename:literal {
            $( $(#[$vmeta:meta])* $variant:ident, )+
        }
    ) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
        #[serde(rename_all = $rename)]
        pub enum $name {
            $( $(#[$vmeta])* $variant, )+
        }

        impl $name {
            /// Every value, in the catalogue's order.
            pub const ALL: &'static [$name] = &[$($name::$variant),+];
        }
    };
}

value_enum!(
    /// `mintswarm.provenance`: whether a fact was recorded as it happened.
    ProvenanceKind, "snake_case" {
        Live,
        Reconstructed,
    }
);

value_enum!(
    /// `mintswarm.provenance.source`: what the fact was recorded or reconstructed from.
    ProvenanceSource, "snake_case" {
        Session,
        Tool,
        Runner,
        Plane,
        Transcript,
        Github,
        JobRecord,
    }
);

value_enum!(
    /// `mintswarm.item.outcome`: how an item ended.
    ItemOutcome, "snake_case" {
        Merged,
        Decomposed,
        Closed,
    }
);

value_enum!(
    /// `mintswarm.stage`: the stage a run belongs to.
    Stage, "kebab-case" {
        Analyse,
        Design,
        Decompose,
        AssessDecomposition,
        Plan,
        Assess,
        Implement,
        Gates,
        Review,
        Address,
        Merge,
        Landing,
    }
);

value_enum!(
    /// `mintswarm.harness`: the coding-agent harness a run used.
    Harness, "snake_case" {
        Claude,
        Codex,
    }
);

value_enum!(
    /// `mintswarm.trigger`: what started a run.
    Trigger, "snake_case" {
        Pipeline,
        Human,
        Factory,
        Retry,
    }
);

value_enum!(
    /// `mintswarm.activity.source`: which activity rules classified a tool call.
    ActivitySource, "snake_case" {
        Default,
        Project,
        None,
        Unparsed,
    }
);

value_enum!(
    /// `mintswarm.wait.kind`: what an item waits for.
    WaitKind, "snake_case" {
        Blocked,
        ToStart,
        Runner,
        Human,
    }
);

value_enum!(
    /// `mintswarm.judge`: the judge that scored confidence.
    Judge, "snake_case" {
        Plan,
        Landing,
        Decomposition,
    }
);

value_enum!(
    /// `mintswarm.decision.kind`: what a decision decides.
    DecisionKind, "snake_case" {
        DesignApproval,
        PlanApproval,
        DecompositionApproval,
        LandingApproval,
        QuestionRound,
        RoundCap,
    }
);

value_enum!(
    /// `mintswarm.decision.verdict`: what a decision came to.
    DecisionVerdict, "snake_case" {
        Approved,
        ChangesRequested,
        Rejected,
        Answered,
        Continue,
        Accept,
    }
);

value_enum!(
    /// `mintswarm.decision.where`: where a decision is taken.
    DecisionWhere, "snake_case" {
        Session,
        Github,
    }
);

value_enum!(
    /// `mintswarm.actor.kind`: who acted.
    ActorKind, "snake_case" {
        Agent,
        Human,
        Judge,
        Rule,
        Delegated,
    }
);

value_enum!(
    /// `mintswarm.intervention.kind`: how a person intervened.
    InterventionKind, "snake_case" {
        AssumptionOverruled,
        Steer,
        HumanReview,
    }
);

value_enum!(
    /// `mintswarm.link.kind`: what a link means.
    LinkKind, "snake_case" {
        BlockedBy,
        SubIssue,
        Trigger,
        Parent,
    }
);

value_enum!(
    /// `mintswarm.link.op`: whether a link was added or removed.
    LinkOp, "snake_case" {
        Added,
        Removed,
    }
);

value_enum!(
    /// `mintswarm.pr.authored_by`: who wrote a PR.
    PrAuthor, "snake_case" {
        Factory,
        Human,
    }
);

value_enum!(
    /// `gen_ai.operation.name` of a stage run.
    InvokeAgent, "snake_case" {
        InvokeAgent,
    }
);

value_enum!(
    /// `gen_ai.operation.name` of a turn.
    Chat, "snake_case" {
        Chat,
    }
);

value_enum!(
    /// `gen_ai.operation.name` of a tool call.
    ExecuteTool, "snake_case" {
        ExecuteTool,
    }
);

/// `mintswarm.outcome.status`: a stage's [`OutcomeStatus`], or `unknown` when it was not
/// recorded.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RunOutcome {
    Known(OutcomeStatus),
    Unknown,
}

impl RunOutcome {
    /// The spelling of [`RunOutcome::Unknown`].
    pub const UNKNOWN: &'static str = "unknown";

    /// Every value, in the catalogue's order.
    pub const ALL: &'static [RunOutcome] = &[
        RunOutcome::Known(OutcomeStatus::Complete),
        RunOutcome::Known(OutcomeStatus::NeedsInput),
        RunOutcome::Known(OutcomeStatus::Failed),
        RunOutcome::Unknown,
    ];
}

impl Serialize for RunOutcome {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            RunOutcome::Known(status) => status.serialize(serializer),
            RunOutcome::Unknown => serializer.serialize_str(Self::UNKNOWN),
        }
    }
}

impl<'de> Deserialize<'de> for RunOutcome {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        if text == Self::UNKNOWN {
            return Ok(RunOutcome::Unknown);
        }
        OutcomeStatus::deserialize(IntoDeserializer::<D::Error>::into_deserializer(text))
            .map(RunOutcome::Known)
    }
}

impl JsonSchema for RunOutcome {
    fn inline_schema() -> bool {
        true
    }

    fn schema_name() -> Cow<'static, str> {
        "RunOutcome".into()
    }

    fn json_schema(generator: &mut SchemaGenerator) -> Schema {
        let mut schema = OutcomeStatus::json_schema(generator);
        if let Some(Value::Array(values)) = schema.get_mut("enum") {
            values.push(Value::from(Self::UNKNOWN));
        }
        schema
    }
}

attributes! {
    /// Where a record comes from; every record carries it.
    #[derive(Eq)]
    pub struct Provenance {
        groups {}
        fields {
            required kind: ProvenanceKind = "mintswarm.provenance",
            required source: ProvenanceSource = "mintswarm.provenance.source",
            /// What a reconstructed record could not recover, for example `start_unrecorded`.
            optional gaps: Vec<String> = "mintswarm.provenance.gaps",
        }
    }
}

attributes! {
    /// The run a stage-run record is about.
    #[derive(Eq)]
    pub struct Run {
        groups {}
        fields {
            required stage: Stage = "mintswarm.stage",
            required harness: Harness = "mintswarm.harness",
            optional effort: String = "mintswarm.effort",
            required round: u32 = "mintswarm.round",
            /// The subagent's agent id while dogfooding, the attempt id in the product.
            required id: String = "mintswarm.run.id",
            required trigger: Trigger = "mintswarm.trigger",
        }
    }
}

attributes! {
    /// The model a run or turn asked for.
    #[derive(Eq)]
    pub struct Model {
        groups {}
        fields {
            required provider: String = "gen_ai.provider.name",
            required request_model: String = "gen_ai.request.model",
        }
    }
}

attributes! {
    /// Token counts by class; the only type holding token classes.
    #[derive(Copy, Default, Eq)]
    pub struct Tokens {
        groups {}
        fields {
            /// All input, cached or not, as the semantic conventions define it.
            required input: u64 = "gen_ai.usage.input_tokens",
            required output: u64 = "gen_ai.usage.output_tokens",
            required cache_read: u64 = "gen_ai.usage.cache_read.input_tokens",
            required cache_creation: u64 = "gen_ai.usage.cache_creation.input_tokens",
        }
    }
}

attributes! {
    /// What a stage run returned.
    #[derive(Copy, Default, Eq)]
    pub struct StageResult {
        groups {}
        fields {
            optional findings_blocker: u32 = "mintswarm.result.findings.blocker",
            optional findings_major: u32 = "mintswarm.result.findings.major",
            optional findings_minor: u32 = "mintswarm.result.findings.minor",
            optional findings_nit: u32 = "mintswarm.result.findings.nit",
            optional gate_verdict: GateVerdict = "mintswarm.result.gate_verdict",
            optional lines_added: u64 = "mintswarm.result.lines_added",
            optional lines_removed: u64 = "mintswarm.result.lines_removed",
            optional questions: u32 = "mintswarm.result.questions",
            optional must_decide: u32 = "mintswarm.result.must_decide",
            optional assumptions: u32 = "mintswarm.result.assumptions",
        }
    }
}

attributes! {
    /// What a wait is for.
    #[derive(Copy, Eq)]
    pub struct Wait {
        groups {}
        fields {
            required kind: WaitKind = "mintswarm.wait.kind",
        }
    }
}

attributes! {
    /// Who acted.
    #[derive(Eq)]
    pub struct Actor {
        groups {}
        fields {
            required kind: ActorKind = "mintswarm.actor.kind",
            /// The actor's login.
            optional id: String = "mintswarm.actor.id",
        }
    }
}

attributes! {
    /// The decision a request or a verdict is about.
    #[derive(Eq)]
    pub struct Decision {
        groups {}
        fields {
            required kind: DecisionKind = "mintswarm.decision.kind",
            required required: bool = "mintswarm.decision.required",
            required place: DecisionWhere = "mintswarm.decision.where",
            optional url: String = "mintswarm.decision.url",
        }
    }
}

attributes! {
    /// What a link means.
    #[derive(Copy, Eq)]
    pub struct Linked {
        groups {}
        fields {
            required kind: LinkKind = "mintswarm.link.kind",
        }
    }
}

attributes! {
    /// The PR a record is about.
    #[derive(Copy, Eq)]
    pub struct Pr {
        groups {}
        fields {
            required number: u64 = "mintswarm.pr.number",
            required authored_by: PrAuthor = "mintswarm.pr.authored_by",
        }
    }
}

/// Host utilisation, as fractions from 0 to 1; sent as telemetry, not as a fact.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HostUtilisation {
    #[serde(rename = "system.cpu.utilization")]
    pub cpu: f64,
    #[serde(rename = "system.memory.utilization")]
    pub memory: f64,
    #[serde(rename = "system.filesystem.utilization")]
    pub filesystem: f64,
}
