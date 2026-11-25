//! The plan decision against `plan-approval/decision.py`: every `Decider.run` of its self-test,
//! captured by `tests/fixtures/plan_decision_capture.py` into
//! `tests/fixtures/plan_decision.jsonl`, and `check_authority_triggers`.
//!
//! Parity is the exit class, the comment printed and the `decision.json` bytes. Where a case reads
//! the verdict `floor.py` wrote, the Rust floor's verdict for the same documents is fed instead;
//! where the two verdicts differ, because the Rust floor words a rule's reason itself, that case
//! compares the record with the rules' reasons and the floor layer's sha256 left out. Those
//! cases are named in [`RELAXED`].

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::{env, fs, process};

use mintswarm_domain::ids::RevId;
use mintswarm_domain::records::BaseRef;
use mintswarm_rules::exit;
use mintswarm_rules::plan_decision::{self, Input, Inputs};
use mintswarm_rules::plan_floor::{self, Bundle, CONFIG, Item, Subjects};
use mintswarm_rules::render::{AUTHORITY_TRIGGERS, HARD_TRIGGERS};
use serde_json::Value;

const REFRESH: &str = "PYTHONDONTWRITEBYTECODE=1 python3 \
                       crates/rules/tests/fixtures/plan_decision_capture.py \
                       crates/rules/tests/fixtures/plan_decision.jsonl";

/// The cases whose real verdict the Rust floor words differently, such as a `footprint` reason
/// that carries a parser's error: none of the self-test's cases today.
const RELAXED: &[&str] = &[];

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn bytes(value: &Value) -> Vec<u8> {
    if let Some(text) = value.get("utf8").and_then(Value::as_str) {
        return text.as_bytes().to_vec();
    }
    let hex = value
        .get("hex")
        .and_then(Value::as_str)
        .expect("bytes are utf8 or hex");
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).expect("hex digits"))
        .collect()
}

fn item(value: &Value) -> Option<Item> {
    if value.is_null() {
        return None;
    }
    let path = value["path"].as_str().expect("a document path");
    Some(Item::new(path, bytes(&value["bytes"])))
}

/// One captured run.
struct Run {
    name: String,
    bundle: Bundle,
    /// The bundle of the real verdict: the documents `floor.py` read when it was written.
    verdict_bundle: Option<Bundle>,
    verdict: Option<Vec<u8>>,
    assessment_path: String,
    assessment: Option<Vec<u8>>,
    issue: u64,
    artifact: String,
    harness: String,
    model: String,
    exit: u8,
    record: Option<Vec<u8>>,
    stdout: String,
}

struct Fixture {
    runs: Vec<Run>,
    triggers: Value,
}

fn with_documents(bundle: &Bundle, documents: &Value) -> Bundle {
    Bundle {
        plan: item(&documents["plan"]).expect("a plan"),
        issue_document: item(&documents["issue_document"]),
        analysis: item(&documents["analysis"]),
        screen: item(&documents["screen"]),
        analysis_record: item(&documents["analysis_record"]),
        plan_record: item(&documents["plan_record"]),
        ..bundle.clone()
    }
}

fn fixture() -> Fixture {
    let text = fs::read_to_string(fixtures().join("plan_decision.jsonl")).expect("the fixture");
    let mut blobs: HashMap<(String, String), Vec<u8>> = HashMap::new();
    let mut runs = Vec::new();
    let mut triggers = Value::Null;
    for line in text.lines() {
        let entry: Value = serde_json::from_str(line).expect("a fixture line is JSON");
        let kind = entry["kind"].as_str().expect("a kind").to_owned();
        match kind.as_str() {
            "config" | "history" => {
                let digest = entry["sha256"].as_str().expect("a digest").to_owned();
                blobs.insert((kind, digest), bytes(&entry["bytes"]));
                continue;
            }
            "triggers" => {
                triggers = entry;
                continue;
            }
            "run" => {}
            _ => continue,
        }
        let blob = |kind: &str| {
            let digest = entry[kind].as_str().expect("a blob digest").to_owned();
            blobs[&(kind.to_owned(), digest)].clone()
        };
        let base = Bundle {
            config: Item::new(CONFIG, blob("config")),
            subjects: Subjects::new(blob("history")),
            base: BaseRef {
                reference: entry["base"]["ref"].as_str().expect("a ref").to_owned(),
                commit: RevId(
                    entry["base"]["commit"]
                        .as_str()
                        .expect("a commit")
                        .to_owned(),
                ),
            },
            component: entry["component"].as_str().expect("a component").to_owned(),
            plan: Item::new("", Vec::new()),
            issue_document: None,
            analysis: None,
            screen: None,
            analysis_record: None,
            plan_record: None,
        };
        let bundle = with_documents(&base, &entry);
        let verdict = &entry["verdict"];
        let verdict_bundle =
            (verdict["real"] == true).then(|| with_documents(&base, &verdict["documents"]));
        let text = |key: &str| entry[key].as_str().expect("a string").to_owned();
        runs.push(Run {
            name: text("case"),
            bundle,
            verdict_bundle,
            verdict: (!verdict.is_null()).then(|| bytes(&verdict["bytes"])),
            assessment_path: entry["assessment"]["path"]
                .as_str()
                .expect("a path")
                .to_owned(),
            assessment: Some(&entry["assessment"]["bytes"])
                .filter(|b| !b.is_null())
                .map(bytes),
            issue: entry["issue"].as_u64().expect("an issue"),
            artifact: text("artifact"),
            harness: text("harness"),
            model: text("model"),
            exit: u8::try_from(entry["exit"].as_u64().expect("an exit code")).expect("0 to 255"),
            record: Some(&entry["record"]).filter(|r| !r.is_null()).map(bytes),
            stdout: text("stdout"),
        });
    }
    Fixture { runs, triggers }
}

fn input(bytes: Option<&Vec<u8>>) -> Input<'_> {
    bytes.map_or(Input::Missing, |bytes| Input::Bytes(bytes))
}

/// A record without what the Rust floor words itself: the rules' reasons and the verdict's
/// sha256.
fn without_floor_wording(text: &[u8]) -> Value {
    let mut record: Value = serde_json::from_slice(text).expect("a JSON record");
    let floor = &mut record["layers"]["floor"];
    floor["sha256"] = Value::Null;
    if let Some(rules) = floor["rules"].as_array_mut() {
        for rule in rules {
            rule["reasons"] = Value::Null;
        }
    }
    record
}

#[test]
fn every_selftest_decision_case_matches() {
    let fixture = fixture();
    let runs = fixture.runs;
    assert!(runs.len() >= 120, "the fixture holds {} runs", runs.len());
    let auto = runs.iter().filter(|run| run.exit == 0).count();
    let shadow = runs
        .iter()
        .filter(|run| run.stdout.starts_with("### Shadow record"))
        .count();
    let real = runs
        .iter()
        .filter(|run| run.verdict_bundle.is_some())
        .count();
    assert!(
        auto >= 15 && shadow >= 8 && real >= 80,
        "{auto} auto, {shadow} shadow and {real} real-verdict runs"
    );
    let mut failures = Vec::new();
    let mut relaxed = Vec::new();
    for run in &runs {
        let real = run.verdict_bundle.as_ref().map(|bundle| {
            plan_floor::floor(bundle)
                .unwrap_or_else(|e| panic!("{}: the real verdict's floor: {e}", run.name))
                .text()
                .into_bytes()
        });
        let loosen = real.is_some() && real.as_ref() != run.verdict.as_ref();
        let verdict = real.as_ref().or(run.verdict.as_ref());
        let inputs = Inputs {
            bundle: &run.bundle,
            issue: run.issue,
            verdict: input(verdict),
            assessment_path: &run.assessment_path,
            assessment: input(run.assessment.as_ref()),
            artifact: &run.artifact,
            harness: &run.harness,
            model: &run.model,
        };
        let decided = plan_decision::decide(&inputs);
        // decision.py prints nothing on unusable input, so the error goes only into the failure
        let (code, text, stdout, unusable) = match &decided {
            Ok(d) => (
                exit::code(&d.outcome()),
                Some(d.text().as_bytes().to_vec()),
                d.comment().unwrap_or("").to_owned(),
                String::new(),
            ),
            Err(e) => (2, None, String::new(), format!(" (unusable: {e})")),
        };
        let same_record = if loosen {
            relaxed.push(run.name.clone());
            text.as_deref().map(without_floor_wording)
                == run.record.as_deref().map(without_floor_wording)
        } else {
            text == run.record
        };
        if code != run.exit || !same_record || stdout != run.stdout {
            failures.push(format!(
                "{}: exit {code}{unusable}, expected {}\n--- got\n{}{}\n--- expected\n{}{}",
                run.name,
                run.exit,
                text.as_deref()
                    .map_or_else(String::new, |t| String::from_utf8_lossy(t).into_owned()),
                stdout,
                run.record
                    .as_deref()
                    .map_or_else(String::new, |t| String::from_utf8_lossy(t).into_owned()),
                run.stdout
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {} runs differ:\n{}",
        failures.len(),
        runs.len(),
        failures.join("\n")
    );
    assert_eq!(
        relaxed, RELAXED,
        "the cases compared without the floor's wording"
    );
}

#[test]
fn check_authority_triggers() {
    let triggers = fixture().triggers;
    let names = |key: &str| -> Vec<String> {
        triggers[key]
            .as_array()
            .expect("a list")
            .iter()
            .map(|t| t.as_str().expect("a name").to_owned())
            .collect()
    };
    assert_eq!(names("hard"), HARD_TRIGGERS, "decision.py's, in order");
    assert_eq!(
        names("authority"),
        AUTHORITY_TRIGGERS,
        "decision.py's, in order"
    );
    for (index, trigger) in AUTHORITY_TRIGGERS.iter().enumerate() {
        assert!(
            HARD_TRIGGERS.contains(trigger),
            "{trigger} is a hard trigger"
        );
        assert!(
            !AUTHORITY_TRIGGERS[..index].contains(trigger),
            "{trigger} is listed once"
        );
    }
}

#[test]
fn a_bundle_without_the_documents_is_unusable() {
    let run = fixture()
        .runs
        .into_iter()
        .find(|run| run.exit != 2)
        .expect("a run");
    let mut bundle = run.bundle.clone();
    bundle.analysis = None;
    let inputs = Inputs {
        bundle: &bundle,
        issue: run.issue,
        verdict: Input::Missing,
        assessment_path: &run.assessment_path,
        assessment: Input::Unreadable,
        artifact: &run.artifact,
        harness: &run.harness,
        model: &run.model,
    };
    assert!(plan_decision::decide(&inputs).is_err());
    let inputs = Inputs {
        bundle: &run.bundle,
        ..inputs
    };
    let decided = plan_decision::decide(&inputs).expect("usable input");
    let judgement = &decided.record().layers.judgement;
    assert_eq!(judgement.problems, ["the assessment cannot be read"]);
    assert_eq!(judgement.sha256, None);
    let mut altered = run.bundle.clone();
    altered.plan.bytes.push(b' ');
    let inputs = Inputs {
        bundle: &altered,
        ..inputs
    };
    assert!(plan_decision::decide(&inputs).is_err(), "a broken binding");
}

#[test]
fn the_plan_decision_fixture_is_fresh() {
    // outside the repository, so the check leaves the source tree untouched
    let out = env::temp_dir().join(format!("mintswarm-plan-decision-{}.jsonl", process::id()));
    let output = Command::new("python3")
        .arg("-I")
        .arg("-B")
        .arg(fixtures().join("plan_decision_capture.py"))
        .arg(&out)
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .output()
        .expect("python3 runs plan_decision_capture.py");
    let captured = fs::read(&out);
    let _ = fs::remove_file(&out);
    assert!(
        output.status.success(),
        "plan_decision_capture.py failed: {}\n{}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    let captured = captured.expect("plan_decision_capture.py wrote its output");
    let committed =
        fs::read(fixtures().join("plan_decision.jsonl")).expect("the committed fixture");
    let (captured, captured_inert) = split_inert(&captured);
    let (committed, committed_inert) = split_inert(&committed);
    assert!(
        captured == committed,
        "tests/fixtures/plan_decision.jsonl is stale against decision.py, floor.py and their \
         self-test; refresh it with:\n    {REFRESH}"
    );
    // The removed ranges follow the Unicode version of the Python running the capture, so they
    // are compared only when that version is the committed one.
    let version = |line: &[u8]| -> String {
        let value: Value = serde_json::from_slice(line).expect("the inert line is JSON");
        value["unicode"]
            .as_str()
            .expect("the inert line names its Unicode version")
            .to_owned()
    };
    let (running, recorded) = (version(captured_inert), version(committed_inert));
    if running == recorded {
        assert!(
            captured_inert == committed_inert,
            "the inert line of tests/fixtures/plan_decision.jsonl is stale; refresh it with:\n    \
             {REFRESH}"
        );
    } else {
        eprintln!(
            "the inert line is not compared: python3 has Unicode {running}, the fixture {recorded}"
        );
    }
}

/// The fixture's lines apart from its `inert` line, and that line.
fn split_inert(fixture: &[u8]) -> (Vec<&[u8]>, &[u8]) {
    let mut inert = None;
    let mut rest = Vec::new();
    for line in fixture.split(|&b| b == b'\n') {
        if line.starts_with(br#"{"kind": "inert""#) {
            assert!(inert.is_none(), "one inert line");
            inert = Some(line);
        } else {
            rest.push(line);
        }
    }
    (rest, inert.expect("the inert line"))
}
