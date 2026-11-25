//! Contract tests: the record types against the Python tools' output, the plane types through
//! serde_json, the code host's lack of a merge, and the ten steps.

use std::convert::Infallible;
use std::fmt::Debug;
use std::future::Future;
use std::path::PathBuf;
use std::pin::pin;
use std::task::{Context, Poll, Waker};

use mintswarm_domain::ids::{
    AttemptId, DirectiveId, IssueNumber, JobId, LeaseEpoch, RevId, RunnerId, Seq, WorkerId,
};
use mintswarm_domain::plane::{
    Assignment, AttemptPhase, Credential, CredentialBinding, CredentialPurpose, CredentialRequest,
    Directive, DirectiveDisposition, DirectiveKind, DirectiveReceipt, Fence, HarnessInfo,
    Heartbeat, HeldAttempt, HelloAck, JobEvent, JobOutcome, Lease, PlaneLink, ProbeSummary, Record,
    RecordKind, RunnerInfo, RunnerMode, StepStatus, Work,
};
use mintswarm_domain::records::{
    Bound, DecisionRecord, FactsRecord, FloorRecord, Json, LandingFloorRecord, RoundRecord,
};
use mintswarm_domain::work::{
    Cost, InputFile, Limit, OutcomeStatus, StageJob, StageOutcome, StepJob, StepKind, Tokens, Usage,
};
use mintswarm_domain::{
    Question, QuestionKind, QuestionOption, QuestionShape, Recommendation, Reference, ReferenceKind,
};
use serde::Serialize;
use serde::de::DeserializeOwned;

const TOOLS: [&str; 5] = [
    FloorRecord::TOOL,
    DecisionRecord::TOOL,
    RoundRecord::TOOL,
    FactsRecord::TOOL,
    LandingFloorRecord::TOOL,
];

/// The line re-serialised through `T`, as an order-preserving tree.
fn through<T: Serialize + DeserializeOwned>(line: &str, version: u32, want: u32) -> Json {
    assert_eq!(version, want, "record version");
    let record: T = serde_json::from_str(line).expect("the record deserialises strictly");
    let text = serde_json::to_string(&record).expect("the record serialises");
    serde_json::from_str(&text).expect("the re-serialisation is JSON")
}

fn field<'a>(tree: &'a Json, key: &str) -> &'a Json {
    match tree {
        Json::Object(entries) => entries
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v)
            .expect("the record has the field"),
        _ => panic!("the record is not an object"),
    }
}

/// `records.jsonl`, or `MSW_RECORD_FIXTURES`, relative to the workspace root.
fn fixtures() -> PathBuf {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    match std::env::var_os("MSW_RECORD_FIXTURES") {
        Some(path) => manifest.join("../..").join(path),
        None => manifest.join("tests/fixtures/records.jsonl"),
    }
}

#[test]
fn records_match_python_fixtures() {
    let path = fixtures();
    let text = std::fs::read_to_string(&path).expect("the fixtures are readable");
    let mut seen = Vec::new();
    for (number, line) in text.lines().enumerate() {
        let tree: Json = serde_json::from_str(line).expect("the fixture is JSON");
        let Json::String(tool) = field(&tree, "tool").clone() else {
            panic!("line {}: tool is not a string", number + 1);
        };
        let Json::Int(version) = *field(&tree, "version") else {
            panic!("line {}: version is not an integer", number + 1);
        };
        let version = u32::try_from(version).expect("version fits");
        let again = match tool.as_str() {
            FloorRecord::TOOL => through::<FloorRecord>(line, version, FloorRecord::VERSION),
            DecisionRecord::TOOL => {
                through::<DecisionRecord>(line, version, DecisionRecord::VERSION)
            }
            RoundRecord::TOOL => through::<RoundRecord>(line, version, RoundRecord::VERSION),
            FactsRecord::TOOL => through::<FactsRecord>(line, version, FactsRecord::VERSION),
            LandingFloorRecord::TOOL => {
                through::<LandingFloorRecord>(line, version, LandingFloorRecord::VERSION)
            }
            other => panic!("line {}: unknown tool {other}", number + 1),
        };
        assert_eq!(
            again,
            tree,
            "line {} of {}: {tool} does not round-trip key for key",
            number + 1,
            path.display()
        );
        seen.push(tool);
    }
    for tool in TOOLS {
        assert!(seen.iter().any(|t| t == tool), "no fixture for {tool}");
    }
}

fn round_trip<T>(value: T)
where
    T: Serialize + DeserializeOwned + PartialEq + Debug + Send + Sync + 'static,
{
    let text = serde_json::to_string(&value).expect("the value serialises");
    let back: T = serde_json::from_str(&text).expect("the value deserialises");
    assert_eq!(back, value, "{text}");
}

fn rev(c: char) -> RevId {
    RevId(c.to_string().repeat(40))
}

fn stage_job() -> StageJob {
    StageJob {
        job: JobId("job-164".into()),
        attempt: AttemptId("att-1".into()),
        issue: IssueNumber(164),
        components: vec!["domain".into()],
        stage: "implement".into(),
        harness: "claude".into(),
        model: "opus".into(),
        effort: Some("high".into()),
        base: rev('a'),
        instructions: rev('b'),
        artefact: "handoff".into(),
        inputs: vec![InputFile {
            path: "issue.md".into(),
            sha256: "0".repeat(64),
        }],
    }
}

fn step_job() -> StepJob {
    StepJob {
        job: JobId("job-164".into()),
        attempt: AttemptId("att-2".into()),
        step: StepKind::KeepRound,
        base: rev('a'),
        head: Some(rev('c')),
        inputs: vec![],
    }
}

#[test]
fn plane_types_round_trip() {
    round_trip(RunnerInfo {
        runner: RunnerId("r1".into()),
        version: "0.1.0".into(),
        host: "host".into(),
        harnesses: vec![HarnessInfo {
            id: "claude".into(),
            version: "2.0".into(),
        }],
        probes: vec![ProbeSummary {
            id: "git".into(),
            passed: true,
        }],
        account: Some("work".into()),
        workers: 4,
        repository: "mintswarm/mintswarm".into(),
        mode: RunnerMode::Observer,
        features: vec!["directive_receipts".into()],
        step_kinds: vec![StepKind::Gates, StepKind::KeepRound],
        protocol_minor: 2,
        held_attempts: vec![held_attempt(AttemptPhase::Started)],
    });
    for phase in [AttemptPhase::Assigned, AttemptPhase::Started] {
        round_trip(phase);
        round_trip(held_attempt(phase));
    }
    for mode in [RunnerMode::Worker, RunnerMode::Observer] {
        round_trip(mode);
    }
    round_trip(LeaseEpoch(u64::MAX));
    round_trip(DirectiveId("dir-1".into()));
    round_trip(HelloAck {
        runner: RunnerId("r1".into()),
        acked: Seq(7),
    });
    round_trip(WorkerId("w1".into()));
    let lease = Lease {
        epoch: LeaseEpoch(3),
        expires_at: "2026-11-23T12:00:00Z".into(),
    };
    round_trip(Assignment {
        work: Work::Stage(stage_job()),
        lease: lease.clone(),
    });
    round_trip(lease.clone());
    round_trip(Assignment {
        work: Work::Step(step_job()),
        lease,
    });
    round_trip(Heartbeat {
        runner: RunnerId("r1".into()),
        workers_free: 1,
        workers_total: 4,
        load: 0.5,
        throttled_until: None,
        active: vec![AttemptId("att-1".into())],
    });
    let attempt = AttemptId("att-1".into());
    for kind in [
        DirectiveKind::Cancel {
            attempt: attempt.clone(),
            lease_epoch: LeaseEpoch(3),
        },
        DirectiveKind::Interrupt {
            attempt: attempt.clone(),
            lease_epoch: LeaseEpoch(3),
            message: Some("stop".into()),
        },
        DirectiveKind::Deliver {
            attempt: attempt.clone(),
            lease_epoch: LeaseEpoch(3),
            message: "a steer".into(),
        },
        DirectiveKind::Drain,
        DirectiveKind::UpgradeRequired,
    ] {
        round_trip(Directive {
            id: DirectiveId("dir-1".into()),
            kind,
        });
    }
    let fence = Fence {
        attempt: attempt.clone(),
        lease_epoch: LeaseEpoch(3),
    };
    round_trip(fence.clone());
    for disposition in DISPOSITIONS {
        for fence in [Some(fence.clone()), None] {
            round_trip(DirectiveReceipt {
                seq: Seq(12),
                directive: DirectiveId("dir-1".into()),
                fence,
                disposition,
            });
        }
    }
    let usage = Usage {
        tokens: Tokens {
            input: 10,
            output: 20,
            cache_read: 30,
            cache_write: 40,
        },
        cost: Some(Cost {
            amount: 1.25,
            currency: "USD".into(),
            basis: "reported".into(),
        }),
        limits: vec![Limit {
            name: "five_hour".into(),
            used_percent: 12.5,
            resets_at: Some("2026-11-23T15:00:00Z".into()),
        }],
        source: "harness".into(),
        observed_at: "2026-11-23T12:00:00Z".into(),
    };
    round_trip(JobEvent {
        job: JobId("job-164".into()),
        seq: Seq(8),
        attempt: attempt.clone(),
        lease_epoch: LeaseEpoch(3),
        outcome: Some(JobOutcome::Stage(StageOutcome {
            status: OutcomeStatus::Complete,
            artefact: "handoff".into(),
            outputs: vec![".mintswarm/job/outputs/handoff.md".into()],
            questions: vec![],
            reason: String::new(),
            commit: Some("feat(domain): types".into()),
            pr_title: Some("feat(domain): types".into()),
        })),
        usage: Some(usage),
        warnings: vec!["inputs changed".into()],
    });
    round_trip(JobEvent {
        job: JobId("job-164".into()),
        seq: Seq(9),
        attempt: attempt.clone(),
        lease_epoch: LeaseEpoch(3),
        outcome: Some(JobOutcome::Step {
            step: StepKind::Gates,
            status: StepStatus::Failed,
            reason: "whitespace".into(),
        }),
        usage: None,
        warnings: vec![],
    });
    round_trip(Record {
        job: JobId("job-164".into()),
        seq: Seq(10),
        attempt: attempt.clone(),
        lease_epoch: LeaseEpoch(3),
        kind: RecordKind::LandingFloor,
        version: 1,
        sha256: "1".repeat(64),
        bytes: b"{}\n".to_vec(),
        bound: vec![Bound {
            path: "plan.md".into(),
            sha256: "2".repeat(64),
        }],
    });
    round_trip(JobId("job-164".into()));
    round_trip(rev('d'));
    for request in credential_requests() {
        round_trip(request);
    }
    for purpose in [CredentialPurpose::Fetch, CredentialPurpose::Push] {
        round_trip(purpose);
    }
    round_trip(Credential {
        token: "secret".into(),
        expires_at: "2026-11-23T13:00:00Z".into(),
        permissions: vec!["contents:write".into()],
    });
    round_trip(Seq(11));
}

const DISPOSITIONS: [DirectiveDisposition; 5] = [
    DirectiveDisposition::AppliedInSession,
    DirectiveDisposition::AppliedAtBoundary,
    DirectiveDisposition::Cancelled,
    DirectiveDisposition::NotApplicable,
    DirectiveDisposition::Unsupported,
];

fn held_attempt(phase: AttemptPhase) -> HeldAttempt {
    HeldAttempt {
        attempt: AttemptId("att-1".into()),
        lease_epoch: LeaseEpoch(3),
        phase,
    }
}

/// A fetch request, then a push to an existing branch, then a push that creates the branch.
fn credential_requests() -> [CredentialRequest; 3] {
    let request = |binding| CredentialRequest {
        job: JobId("job-164".into()),
        attempt: AttemptId("att-1".into()),
        lease_epoch: LeaseEpoch(3),
        binding,
    };
    let push = |expected_remote_head| CredentialBinding::Push {
        branch: "mintswarm/164-types".into(),
        commit: rev('e'),
        expected_remote_head,
        changes_bundle_sha256: "3".repeat(64),
    };
    [
        request(CredentialBinding::Fetch {
            remote: "origin".into(),
        }),
        request(push(Some(rev('f')))),
        request(push(None)),
    ]
}

#[test]
fn directive_dispositions_serialise_by_name() {
    let names: Vec<String> = DISPOSITIONS
        .iter()
        .map(|d| serde_json::to_string(d).expect("serialises"))
        .collect();
    assert_eq!(
        names,
        [
            "\"applied_in_session\"",
            "\"applied_at_boundary\"",
            "\"cancelled\"",
            "\"not_applicable\"",
            "\"unsupported\"",
        ]
    );
}

#[test]
fn runner_info_defaults_new_fields() {
    let info: RunnerInfo = serde_json::from_str(
        r#"{"runner": "r1", "version": "0.1.0", "host": "host", "harnesses": [], "probes": [],
            "account": null, "workers": 1, "repository": "mintswarm/mintswarm"}"#,
    )
    .expect("older RunnerInfo JSON reads");
    assert_eq!(info.mode, RunnerMode::Worker);
    assert!(info.features.is_empty());
    assert!(info.step_kinds.is_empty());
    assert_eq!(info.protocol_minor, 0);
    assert!(info.held_attempts.is_empty());
}

#[test]
fn missing_fence_is_rejected() {
    let event = r#"{"job": "job-164", "seq": 1, "attempt": "att-1", "outcome": null,
        "usage": null, "warnings": []}"#;
    assert!(serde_json::from_str::<JobEvent>(event).is_err());
    let record = r#"{"job": "job-164", "seq": 2, "attempt": "att-1", "kind": "floor",
        "version": 1, "sha256": "", "bytes": [], "bound": []}"#;
    assert!(serde_json::from_str::<Record>(record).is_err());
    // The same JSON with the epoch reads, so the failures above are the missing fence.
    let event = event.replacen('{', r#"{"lease_epoch": 1, "#, 1);
    assert!(serde_json::from_str::<JobEvent>(&event).is_ok());
    let record = record.replacen('{', r#"{"lease_epoch": 1, "#, 1);
    assert!(serde_json::from_str::<Record>(&record).is_ok());
}

#[test]
fn credential_request_purpose() {
    let purposes = credential_requests().map(|r| r.purpose());
    assert_eq!(
        purposes,
        [
            CredentialPurpose::Fetch,
            CredentialPurpose::Push,
            CredentialPurpose::Push
        ]
    );
}

/// A `PlaneLink` that implements only the required methods, so the defaults are what is tested.
struct Stub;

impl PlaneLink for Stub {
    type Error = Infallible;

    async fn hello(&self, info: RunnerInfo) -> Result<HelloAck, Infallible> {
        Ok(HelloAck {
            runner: info.runner,
            acked: Seq(0),
        })
    }

    async fn claim(&self, _worker: WorkerId) -> Result<Option<Assignment>, Infallible> {
        Ok(None)
    }

    async fn heartbeat(&self, _hb: Heartbeat) -> Result<Vec<Directive>, Infallible> {
        Ok(vec![])
    }

    async fn report(&self, _ev: JobEvent) -> Result<(), Infallible> {
        Ok(())
    }

    async fn put_record(&self, _rec: Record) -> Result<(), Infallible> {
        Ok(())
    }

    async fn credential(&self, _req: CredentialRequest) -> Result<Credential, Infallible> {
        Ok(Credential {
            token: "secret".into(),
            expires_at: "2026-11-23T13:00:00Z".into(),
            permissions: vec![],
        })
    }

    async fn acked(&self) -> Result<Seq, Infallible> {
        Ok(Seq(0))
    }
}

fn poll_once<F: Future>(future: F) -> Poll<F::Output> {
    let mut cx = Context::from_waker(Waker::noop());
    pin!(future).poll(&mut cx)
}

#[test]
fn plane_link_defaults() {
    let stub = Stub;
    let receipt = DirectiveReceipt {
        seq: Seq(1),
        directive: DirectiveId("dir-1".into()),
        fence: None,
        disposition: DirectiveDisposition::Unsupported,
    };
    assert!(matches!(
        poll_once(stub.receipt(receipt)),
        Poll::Ready(Ok(()))
    ));
    let notify = stub.directive_notify();
    assert!(poll_once(notify.notified()).is_pending());
}

#[test]
fn json_keeps_integers_above_i64() {
    let tree: Json = serde_json::from_str("[1, -1, 18446744073709551615]").expect("JSON");
    assert_eq!(
        tree,
        Json::Array(vec![Json::Int(1), Json::Int(-1), Json::UInt(u64::MAX)])
    );
    let text = serde_json::to_string(&tree).expect("serialises");
    assert_eq!(text, "[1,-1,18446744073709551615]");
}

#[test]
fn credential_debug_hides_the_token() {
    let cred = Credential {
        token: "ghs_secret".into(),
        expires_at: "2026-11-23T13:00:00Z".into(),
        permissions: vec!["contents:write".into()],
    };
    let shown = format!("{cred:?}");
    assert!(!shown.contains("ghs_secret"));
    assert!(shown.contains("contents:write"));
}

#[test]
fn code_host_declares_no_merge() {
    let source = include_str!("../src/tracker.rs");
    let start = source
        .find("pub trait CodeHost")
        .expect("tracker.rs declares CodeHost");
    let rest = &source[start..];
    let end = rest.find("\n}").expect("the CodeHost block ends");
    let block = &rest[..end];
    // Only lines that start with `fn ` declare a method; doc comments and bodies do not count.
    let names: Vec<&str> = block
        .lines()
        .filter_map(|line| line.trim_start().strip_prefix("fn "))
        .map(|s| s.split(['(', '<']).next().unwrap_or_default().trim())
        .collect();
    assert_eq!(
        names,
        ["open_pr", "mark_ready", "request_review", "read_pr"]
    );
    assert!(names.iter().all(|n| !n.contains("merge")), "{names:?}");
}

fn index(step: StepKind) -> usize {
    match step {
        StepKind::Prepare => 0,
        StepKind::Commit => 1,
        StepKind::Gates => 2,
        StepKind::Push => 3,
        StepKind::Bundle => 4,
        StepKind::KeepRound => 5,
        StepKind::DependencyInputs => 6,
        StepKind::LandingFacts => 7,
        StepKind::TrackerEffects => 8,
        StepKind::FileChildren => 9,
    }
}

#[test]
fn step_job_has_the_ten_steps() {
    let names: Vec<String> = StepKind::ALL
        .iter()
        .map(|s| serde_json::to_string(s).unwrap())
        .collect();
    assert_eq!(
        names,
        [
            "\"prepare\"",
            "\"commit\"",
            "\"gates\"",
            "\"push\"",
            "\"bundle\"",
            "\"keep_round\"",
            "\"dependency_inputs\"",
            "\"landing_facts\"",
            "\"tracker_effects\"",
            "\"file_children\"",
        ]
    );
    for (i, step) in StepKind::ALL.iter().enumerate() {
        assert_eq!(index(*step), i);
    }
}

fn option(id: &str, trade_off: Option<&str>) -> QuestionOption {
    QuestionOption {
        id: id.into(),
        label: format!("option {id}"),
        consequence: format!("choosing {id}"),
        trade_off: trade_off.map(Into::into),
    }
}

fn reference(kind: ReferenceKind, target: &str, lines: Option<&str>) -> Reference {
    Reference {
        kind,
        target: target.into(),
        lines: lines.map(Into::into),
    }
}

#[test]
fn stage_outcome_questions_round_trip() {
    let outcome = StageOutcome {
        status: OutcomeStatus::NeedsInput,
        artefact: "analysis".into(),
        outputs: vec![".mintswarm/job/outputs/analysis.md".into()],
        questions: vec![
            Question {
                id: "q1".into(),
                kind: QuestionKind::MustDecide,
                question: "What should the parser be called?".into(),
                why: "It names the module.".into(),
                shape: QuestionShape::Open,
                options: vec![],
                recommendation: None,
                references: vec![],
            },
            Question {
                id: "q2".into(),
                kind: QuestionKind::Recommended,
                question: "Which formats are read?".into(),
                why: "Sets the parser's scope.".into(),
                shape: QuestionShape::Several,
                options: vec![
                    option("a", Some("slower")),
                    option("b", None),
                    option("c", None),
                ],
                recommendation: Some(Recommendation {
                    choice: vec!["a".into(), "b".into()],
                    answer: None,
                    reason: "Both are in use.".into(),
                }),
                references: vec![],
            },
            Question {
                id: "q3".into(),
                kind: QuestionKind::Assumption,
                question: "Which component owns the parser?".into(),
                why: "Decides the reviewers.".into(),
                shape: QuestionShape::One,
                options: vec![option("a", None), option("b", None)],
                recommendation: Some(Recommendation {
                    choice: vec!["a".into()],
                    answer: None,
                    reason: "The types live there.".into(),
                }),
                references: vec![
                    reference(
                        ReferenceKind::File,
                        "crates/domain/src/work.rs",
                        Some("80-104"),
                    ),
                    reference(ReferenceKind::Artefact, "analysis", None),
                    reference(ReferenceKind::Ticket, "#347", None),
                    reference(ReferenceKind::Design, "#339 §4.1", None),
                    reference(ReferenceKind::Answer, "s7/q1", None),
                    reference(ReferenceKind::Url, "https://example.com/spec", None),
                ],
            },
        ],
        reason: String::new(),
        commit: None,
        pr_title: None,
    };
    outcome.check().expect("the outcome passes its checks");
    round_trip(outcome);

    let full = Question {
        id: "q1".into(),
        kind: QuestionKind::MustDecide,
        question: "Proceed?".into(),
        why: "Blocks the plan.".into(),
        shape: QuestionShape::YesNo,
        options: vec![],
        recommendation: Some(Recommendation {
            choice: vec![],
            answer: None,
            reason: "No preference.".into(),
        }),
        references: vec![],
    };
    let nulls = r#"{"id": "q1", "kind": "must_decide", "question": "Proceed?",
        "why": "Blocks the plan.", "shape": "yes_no", "options": null, "references": null,
        "recommendation": {"choice": null, "answer": null, "reason": "No preference."}}"#;
    let absent = r#"{"id": "q1", "kind": "must_decide", "question": "Proceed?",
        "why": "Blocks the plan.", "shape": "yes_no",
        "recommendation": {"reason": "No preference."}}"#;
    for text in [nulls, absent] {
        let question: Question = serde_json::from_str(text).expect("the question deserialises");
        assert_eq!(question, full, "{text}");
    }
}
