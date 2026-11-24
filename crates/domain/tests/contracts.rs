//! Contract tests: the record types against the Python tools' output, the plane types through
//! serde_json, the code host's lack of a merge, and the ten steps.

use std::fmt::Debug;
use std::path::PathBuf;

use mintswarm_domain::ids::{AttemptId, IssueNumber, JobId, RevId, RunnerId, Seq, WorkerId};
use mintswarm_domain::plane::{
    Assignment, Credential, Directive, DirectiveKind, HarnessInfo, Heartbeat, HelloAck, JobEvent,
    JobOutcome, Lease, ProbeSummary, Record, RecordKind, RunnerInfo, StepStatus, Work,
};
use mintswarm_domain::records::{
    Bound, DecisionRecord, FactsRecord, FloorRecord, Json, LandingFloorRecord, RoundRecord,
};
use mintswarm_domain::work::{
    Cost, InputFile, Limit, OutcomeStatus, StageJob, StageOutcome, StepJob, StepKind, Tokens, Usage,
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
        component: "domain".into(),
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
    });
    round_trip(HelloAck {
        runner: RunnerId("r1".into()),
        acked: Seq(7),
    });
    round_trip(WorkerId("w1".into()));
    let lease = Lease {
        expires_at: "2026-11-23T12:00:00Z".into(),
    };
    round_trip(Assignment {
        work: Work::Stage(stage_job()),
        lease: lease.clone(),
    });
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
        },
        DirectiveKind::Interrupt {
            attempt: attempt.clone(),
            message: Some("stop".into()),
        },
        DirectiveKind::Deliver {
            attempt: attempt.clone(),
            message: "a steer".into(),
        },
        DirectiveKind::Drain,
        DirectiveKind::UpgradeRequired,
    ] {
        round_trip(Directive { kind });
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
        attempt,
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
    round_trip(Credential {
        token: "secret".into(),
        expires_at: "2026-11-23T13:00:00Z".into(),
    });
    round_trip(Seq(11));
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
    };
    assert!(!format!("{cred:?}").contains("ghs_secret"));
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
