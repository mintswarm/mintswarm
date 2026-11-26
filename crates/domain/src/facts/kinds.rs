//! The attributes of each fact kind, design §4.3: its groups, held as typed fields, then its own
//! keys. [`super::FactKind`] gives each kind's OTLP signal, name and natural key.

use super::catalogue::{
    self, ActivitySource, Actor, Band, Chat, Decision, DecisionVerdict, ExecuteTool,
    InterventionKind, InvokeAgent, ItemOutcome, Judge, LinkOp, Linked, Model, Pr, Provenance, Run,
    RunOutcome, StageResult, SteerKind, SteerVia, Tokens,
};
use super::ids::{FactId, ItemKey};

attributes! {
    /// The item's root span.
    pub struct ItemRoot {
        groups {
            provenance: Provenance,
        }
        fields {
            required outcome: ItemOutcome = "mintswarm.item.outcome",
            /// The number of the PR that landed the item.
            optional landed_by: u64 = "mintswarm.item.landed_by",
        }
    }
}

attributes! {
    /// A stage run has started.
    pub struct StageRunStarted {
        groups {
            provenance: Provenance,
            run: Run,
        }
        fields {}
    }
}

attributes! {
    /// A stage run.
    pub struct StageRun {
        groups {
            provenance: Provenance,
            run: Run,
            model: Model,
            result: StageResult,
        }
        fields {
            required operation: InvokeAgent = "gen_ai.operation.name",
            /// `msw-<stage>`.
            required agent: String = "gen_ai.agent.name",
            /// Unix nanoseconds: when the run was queued.
            optional queued_at: u64 = "mintswarm.run.queued_at",
            required outcome: RunOutcome = "mintswarm.outcome.status",
        }
    }
}

attributes! {
    /// One model turn.
    pub struct Turn {
        groups {
            provenance: Provenance,
            model: Model,
            tokens: Tokens,
        }
        fields {
            required operation: Chat = "gen_ai.operation.name",
            optional response_model: String = "gen_ai.response.model",
            /// The model's message id.
            required response_id: String = "gen_ai.response.id",
        }
    }
}

attributes! {
    /// One tool call.
    pub struct ToolCall {
        groups {
            provenance: Provenance,
        }
        fields {
            required operation: ExecuteTool = "gen_ai.operation.name",
            required tool: String = "gen_ai.tool.name",
            required call_id: String = "gen_ai.tool.call.id",
            /// The activity the rules classified the call as; an open set.
            required activity: String = "mintswarm.activity",
            required activity_source: ActivitySource = "mintswarm.activity.source",
            /// The shell program only, never the command line.
            optional program: String = "mintswarm.tool.program",
            optional mcp_server: String = "mintswarm.mcp.server",
        }
    }
}

attributes! {
    /// A named phase of a run.
    pub struct Phase {
        groups {
            provenance: Provenance,
        }
        fields {
            required name: String = "mintswarm.phase",
        }
    }
}

attributes! {
    /// A wait has started.
    pub struct WaitStarted {
        groups {
            provenance: Provenance,
            wait: catalogue::Wait,
        }
        fields {}
    }
}

attributes! {
    /// A wait.
    pub struct Wait {
        groups {
            provenance: Provenance,
            wait: catalogue::Wait,
        }
        fields {}
    }
}

attributes! {
    /// A judge's confidence in a run's output.
    pub struct ConfidenceScored {
        groups {
            provenance: Provenance,
        }
        fields {
            required judge: Judge = "mintswarm.judge",
            required value: f64 = "mintswarm.confidence.value",
            required band: Band = "mintswarm.confidence.band",
            required threshold: f64 = "mintswarm.confidence.threshold",
            optional hard_triggers: Vec<String> = "mintswarm.confidence.hard_triggers",
        }
    }
}

attributes! {
    /// A decision was requested.
    pub struct DecisionRequested {
        groups {
            provenance: Provenance,
            decision: Decision,
        }
        fields {}
    }
}

attributes! {
    /// A decision was taken.
    pub struct DecisionTaken {
        groups {
            provenance: Provenance,
            actor: Actor,
            decision: Decision,
        }
        fields {
            required verdict: DecisionVerdict = "mintswarm.decision.verdict",
            optional answers_recommended: u32 = "mintswarm.decision.answers.recommended",
            optional answers_other_option: u32 = "mintswarm.decision.answers.other_option",
            optional answers_free_text: u32 = "mintswarm.decision.answers.free_text",
            optional must_decide: u32 = "mintswarm.decision.must_decide",
            optional assumptions_confirmed: u32 = "mintswarm.decision.assumptions.confirmed",
            optional assumptions_overruled: u32 = "mintswarm.decision.assumptions.overruled",
            optional assumptions_silent: u32 = "mintswarm.decision.assumptions.silent",
            /// The fact id of the judge's `confidence_scored` for the same item.
            optional confidence_fact: FactId = "mintswarm.decision.confidence_fact",
        }
    }
}

attributes! {
    /// A person intervened.
    pub struct Intervention {
        groups {
            provenance: Provenance,
            actor: Actor,
        }
        fields {
            required kind: InterventionKind = "mintswarm.intervention.kind",
            /// Where a steer was given; only for a steer.
            optional steer_kind: SteerKind = "mintswarm.steer.kind",
            /// Why it counts as a steer; only for a steer.
            optional steer_via: SteerVia = "mintswarm.steer.via",
            optional url: String = "mintswarm.intervention.url",
        }
    }
}

attributes! {
    /// The item was picked up.
    pub struct PickedUp {
        groups {
            provenance: Provenance,
        }
        fields {}
    }
}

attributes! {
    /// The item's state changed.
    pub struct StateChanged {
        groups {
            provenance: Provenance,
        }
        fields {
            /// The state left, as the tracker's state options spell it.
            required from: String = "mintswarm.state.from",
            /// The state entered, as the tracker's state options spell it.
            required to: String = "mintswarm.state.to",
        }
    }
}

attributes! {
    /// A link from the item was added or removed.
    pub struct LinkChanged {
        groups {
            provenance: Provenance,
            linked: Linked,
        }
        fields {
            required target: ItemKey = "mintswarm.link.target",
            required op: LinkOp = "mintswarm.link.op",
        }
    }
}

attributes! {
    /// A PR was opened.
    pub struct PrOpened {
        groups {
            provenance: Provenance,
            pr: Pr,
        }
        fields {}
    }
}

attributes! {
    /// A PR was merged.
    pub struct PrMerged {
        groups {
            provenance: Provenance,
            pr: Pr,
        }
        fields {}
    }
}

attributes! {
    /// A PR was reverted.
    pub struct PrReverted {
        groups {
            provenance: Provenance,
            pr: Pr,
        }
        fields {
            /// The number of the reverting PR.
            required revert_pr: u64 = "mintswarm.revert.pr",
        }
    }
}

attributes! {
    /// The item was linked as a regression of another.
    pub struct RegressionLinked {
        groups {
            provenance: Provenance,
        }
        fields {
            /// The item that regressed.
            required of: ItemKey = "mintswarm.regression.of",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::facts::FlatAttributes;
    use crate::facts::catalogue::{Harness, ProvenanceKind, ProvenanceSource, Stage, Trigger};
    use schemars::{SchemaGenerator, generate::SchemaSettings};
    use serde_json::json;

    fn live() -> Provenance {
        Provenance {
            kind: ProvenanceKind::Live,
            source: ProvenanceSource::Runner,
            gaps: None,
        }
    }

    fn turn() -> Turn {
        Turn {
            provenance: live(),
            model: Model {
                provider: "anthropic".into(),
                request_model: "claude-opus-4".into(),
            },
            tokens: Tokens {
                input: 100,
                output: 20,
                cache_read: 60,
                cache_creation: 10,
            },
            operation: Chat::Chat,
            response_model: None,
            response_id: "msg_01".into(),
        }
    }

    #[test]
    fn turn_round_trips_with_flat_token_keys() {
        let value = serde_json::to_value(turn()).unwrap();
        assert_eq!(
            value,
            json!({
                "mintswarm.provenance": "live",
                "mintswarm.provenance.source": "runner",
                "gen_ai.provider.name": "anthropic",
                "gen_ai.request.model": "claude-opus-4",
                "gen_ai.usage.input_tokens": 100,
                "gen_ai.usage.output_tokens": 20,
                "gen_ai.usage.cache_read.input_tokens": 60,
                "gen_ai.usage.cache_creation.input_tokens": 10,
                "gen_ai.operation.name": "chat",
                "gen_ai.response.id": "msg_01",
            })
        );
        let back: Turn = serde_json::from_value(value).unwrap();
        assert_eq!(back, turn());
        assert_eq!(back.tokens.cache_read, 60);
    }

    #[test]
    fn serialises_groups_then_own_keys_in_order() {
        let text = serde_json::to_string(&turn()).unwrap();
        let provider = text.find("gen_ai.provider.name").unwrap();
        let input = text.find("gen_ai.usage.input_tokens").unwrap();
        let operation = text.find("gen_ai.operation.name").unwrap();
        assert!(text.find("mintswarm.provenance").unwrap() < provider);
        assert!(provider < input && input < operation);
    }

    #[test]
    fn unknown_attribute_key_fails() {
        let mut value = serde_json::to_value(turn()).unwrap();
        value["gen_ai.usage.reasoning_tokens"] = json!(5);
        assert!(serde_json::from_value::<Turn>(value).is_err());
    }

    #[test]
    fn missing_required_group_key_fails() {
        let mut value = serde_json::to_value(turn()).unwrap();
        value
            .as_object_mut()
            .unwrap()
            .remove("gen_ai.usage.output_tokens");
        assert!(serde_json::from_value::<Turn>(value).is_err());
    }

    #[test]
    fn null_optional_value_fails() {
        let mut value = serde_json::to_value(turn()).unwrap();
        value["gen_ai.response.model"] = json!(null);
        assert!(serde_json::from_value::<Turn>(value).is_err());
    }

    #[test]
    fn optional_keys_round_trip() {
        let started = StageRunStarted {
            provenance: Provenance {
                gaps: Some(vec!["round_unknown".into()]),
                ..live()
            },
            run: Run {
                stage: Stage::AssessDecomposition,
                harness: Harness::Codex,
                effort: Some("high".into()),
                round: 2,
                id: "run-1".into(),
                trigger: Trigger::Retry,
            },
        };
        let value = serde_json::to_value(&started).unwrap();
        assert_eq!(value["mintswarm.provenance.gaps"], json!(["round_unknown"]));
        assert_eq!(value["mintswarm.stage"], json!("assess-decomposition"));
        assert_eq!(value["mintswarm.effort"], json!("high"));
        assert_eq!(
            serde_json::from_value::<StageRunStarted>(value).unwrap(),
            started
        );
    }

    #[test]
    fn merged_schema_is_closed() {
        let mut generator = SchemaGenerator::new(SchemaSettings::default().with(|s| {
            s.inline_subschemas = true;
        }));
        let schema = <Turn as schemars::JsonSchema>::json_schema(&mut generator).to_value();
        assert_eq!(schema["additionalProperties"], json!(false));
        assert_eq!(schema["type"], json!("object"));
        let properties = schema["properties"].as_object().unwrap();
        for key in [
            "mintswarm.provenance",
            "gen_ai.request.model",
            "gen_ai.usage.cache_creation.input_tokens",
            "gen_ai.response.model",
        ] {
            assert!(properties.contains_key(key), "{key}");
        }
        let required: Vec<&str> = schema["required"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert!(required.contains(&"gen_ai.usage.input_tokens"));
        assert!(!required.contains(&"gen_ai.response.model"));
        assert!(!required.contains(&"mintswarm.provenance.gaps"));
        assert_eq!(Turn::KEYS.len(), 3);
    }
}
