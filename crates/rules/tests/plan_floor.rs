//! The plan floor against `plan-approval/floor.py`: every pure-floor run of its self-test,
//! captured by `tests/fixtures/plan_floor_capture.py` into `tests/fixtures/plan_floor.jsonl`, and
//! the refusal of a bundle whose bindings do not hold.
//!
//! Parity is the exit class and the whole `floor.json` record, its bindings, mode and verdict,
//! each rule's id and outcome, and the rules not evaluated, but not the rules' reasons. Reasons are
//! not compared: a reason can carry a parser's message, whose wording is
//! the parser's own and changes between versions.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::{env, fs, process};

use mintswarm_domain::ids::RevId;
use mintswarm_domain::records::{BaseRef, Verdict as RuleVerdict};
use mintswarm_rules::exit::{self, Outcome};
use mintswarm_rules::plan_floor::{self, Bundle, CONFIG, Item, Subjects, footprint};
use serde_json::Value;

const REFRESH: &str = "PYTHONDONTWRITEBYTECODE=1 python3 \
                       crates/rules/tests/fixtures/plan_floor_capture.py \
                       crates/rules/tests/fixtures/plan_floor.jsonl";

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

/// One captured run: its name, the bundle it stands for, its exit code and the decision of its
/// `floor.json`, as [`decision`] gives it.
struct Run {
    name: String,
    bundle: Bundle,
    exit: u8,
    floor: Option<Value>,
}

fn runs() -> Vec<Run> {
    let text = fs::read_to_string(fixtures().join("plan_floor.jsonl")).expect("the fixture");
    let mut blobs: HashMap<(String, String), Vec<u8>> = HashMap::new();
    let mut runs = Vec::new();
    for line in text.lines() {
        let entry: Value = serde_json::from_str(line).expect("a fixture line is JSON");
        let kind = entry["kind"].as_str().expect("a kind");
        if kind != "run" {
            let digest = entry["sha256"].as_str().expect("a digest").to_owned();
            blobs.insert((kind.to_owned(), digest), bytes(&entry["bytes"]));
            continue;
        }
        let blob = |kind: &str| {
            let digest = entry[kind].as_str().expect("a blob digest").to_owned();
            blobs[&(kind.to_owned(), digest)].clone()
        };
        let bundle = Bundle {
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
            components: entry["components"]
                .as_array()
                .expect("a components list")
                .iter()
                .map(|name| name.as_str().expect("a component name").to_owned())
                .collect(),
            plan: item(&entry["plan"]).expect("a plan"),
            issue_document: item(&entry["issue_document"]),
            analysis: item(&entry["analysis"]),
            screen: item(&entry["screen"]),
            analysis_record: item(&entry["analysis_record"]),
            plan_record: item(&entry["plan_record"]),
        };
        runs.push(Run {
            name: format!(
                "{} case {}",
                entry["group"].as_str().expect("a group"),
                entry["case"]
            ),
            bundle,
            exit: u8::try_from(entry["exit"].as_u64().expect("an exit code")).expect("0 to 255"),
            floor: Some(entry["floor"].clone()).filter(|floor| !floor.is_null()),
        });
    }
    runs
}

/// What the parity compares of a `floor.json` text, as `plan_floor_capture.py` records it.
fn decision(text: &str) -> Value {
    let mut floor: Value = serde_json::from_str(text).expect("floor.json is JSON");
    let rules: Vec<Value> = floor["rules"]
        .as_array()
        .expect("rules")
        .iter()
        .map(|rule| serde_json::json!([rule["rule"], rule["result"]]))
        .collect();
    floor["rules"] = Value::Array(rules);
    floor
}

fn outcome(bundle: &Bundle) -> (Outcome, Option<String>) {
    match plan_floor::floor(bundle) {
        Ok(floor) => (floor.outcome(), Some(floor.text())),
        Err(unusable) => (Err(unusable), None),
    }
}

#[test]
fn every_selftest_floor_case_matches() {
    let runs = runs();
    assert!(runs.len() > 100, "the fixture holds {} runs", runs.len());
    let unusable = runs.iter().filter(|run| run.exit == 2).count();
    assert!(unusable >= 20, "the fixture holds {unusable} exit-2 runs");
    let mut failures = Vec::new();
    for run in &runs {
        let (outcome, text) = outcome(&run.bundle);
        let code = exit::code(&outcome);
        let got = text.as_deref().map(decision);
        if code != run.exit || got != run.floor {
            failures.push(format!(
                "{}: exit {code}, expected {}\n--- got\n{}\n--- expected\n{}",
                run.name,
                run.exit,
                got.map_or_else(
                    || outcome
                        .as_ref()
                        .err()
                        .map_or_else(String::new, |e| e.message().to_owned()),
                    |got| got.to_string()
                ),
                run.floor
                    .as_ref()
                    .map_or_else(|| "(none)".to_owned(), Value::to_string)
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
}

#[test]
fn malformed_footprints_have_the_footprint_rule() {
    let mut checked = 0;
    for run in runs() {
        let Some(captured) = &run.floor else { continue };
        let rule = &captured["rules"][0];
        assert_eq!(rule[0], "footprint", "{}", run.name);
        if rule[1] != "hit" {
            continue;
        }
        let plan = std::str::from_utf8(&run.bundle.plan.bytes).expect("a UTF-8 plan");
        footprint::parse(plan).expect_err(&run.name);
        let floor = plan_floor::floor(&run.bundle).expect(&run.name);
        let first = &floor.record().rules[0];
        assert_eq!(
            (first.rule.as_str(), first.result),
            ("footprint", RuleVerdict::Hit),
            "{}",
            run.name
        );
        checked += 1;
    }
    assert!(checked >= 33, "{checked} malformed footprints checked");
}

/// The reasons `rule` gives on `bundle`, which must be usable.
fn reasons(bundle: &Bundle, rule: &str) -> Vec<String> {
    let floor = plan_floor::floor(bundle).expect("a usable bundle");
    floor
        .record()
        .rules
        .iter()
        .find(|r| r.rule == rule)
        .map(|r| r.reasons.clone())
        .unwrap_or_default()
}

/// The captured runs whose `--components` were `names`.
fn given<'r>(runs: &'r [Run], names: &[&str]) -> Vec<&'r Run> {
    runs.iter()
        .filter(|run| run.bundle.components == names)
        .collect()
}

#[test]
fn component_scope_names_the_components() {
    let runs = runs();
    let mut checked = 0;
    for run in &runs {
        if run.exit == 2 {
            continue;
        }
        let names = &run.bundle.components;
        let outside = match names.as_slice() {
            [one] => format!(" is outside component {one} and shared"),
            _ => format!(" is outside components {} and shared", names.join(", ")),
        };
        for reason in reasons(&run.bundle, "component-scope") {
            assert!(reason.ends_with(&outside), "{}: {reason}", run.name);
            checked += 1;
        }
    }
    assert!(checked >= 3, "{checked} component-scope reasons checked");
    let two = given(&runs, &["domain", "runner"]);
    assert!(two.iter().any(|run| reasons(&run.bundle, "component-scope")
        == ["web/index.html is outside components domain, runner and shared"]));
}

#[test]
fn the_component_rules_follow_floor_py_wording() {
    let runs = runs();
    let wants: [(&[&str], &str, &[&str]); 6] = [
        (
            &["cli", "runner"],
            "first-of-kind",
            &[
                "first of its kind: no commit reachable from the base commit has a subject scoped (cli)",
                "first of its kind: no commit reachable from the base commit has a subject scoped (runner)",
            ],
        ),
        (
            &["core", "cli"],
            "first-of-kind",
            &[
                "first of its kind: no commit reachable from the base commit has a subject scoped (cli)",
            ],
        ),
        (
            &["docs", "web"],
            "first-of-kind",
            &["the issue's components are 'docs', not --components docs,web"],
        ),
        (
            &["docs", "agents"],
            "require-human-components",
            &["the issue's component agents is in require_human.components"],
        ),
        (
            &["docs", "web"],
            "limit-lines",
            &["lines is 501, over the limit of 500 (limits_by_component.web)"],
        ),
        (
            &["cli", "docs"],
            "limit-lines",
            &["lines is 401, over the limit of 400 (limits_by_component.cli)"],
        ),
    ];
    for (names, rule, want) in wants {
        let found = given(&runs, names)
            .iter()
            .filter(|run| run.exit != 2)
            .any(|run| reasons(&run.bundle, rule) == want);
        assert!(
            found,
            "no run with --components {names:?} gives {rule}: {want:?}"
        );
    }
}

#[test]
fn bad_components_are_unusable_with_floor_py_wording() {
    let runs = runs();
    let wants: [(&[&str], &str); 4] = [
        (&["docs", ""], "--components has an empty name"),
        (&["", "docs"], "--components has an empty name"),
        (&["docs", "docs"], "--components names 'docs' twice"),
        (
            &["docs", "nope"],
            "--components 'nope' is not a component in the config",
        ),
    ];
    for (names, want) in wants {
        let found = given(&runs, names);
        assert!(!found.is_empty(), "no run with --components {names:?}");
        for run in found {
            assert_eq!(run.exit, 2, "{}", run.name);
            let error = plan_floor::floor(&run.bundle).expect_err(&run.name);
            assert_eq!(error.message(), want, "{}", run.name);
        }
    }
}

#[test]
fn no_components_read_as_an_empty_name() {
    let runs = runs();
    let run = runs.iter().find(|run| run.exit != 2).expect("a usable run");
    let mut bundle = run.bundle.clone();
    bundle.components.clear();
    let error = plan_floor::floor(&bundle).expect_err(&run.name);
    assert_eq!(error.message(), "--components has an empty name");
}

/// An optional item of a bundle.
type OptionalItem = fn(&mut Bundle) -> Option<&mut Item>;

/// Each content item of `bundle` changed by `change`, one at a time.
fn each_item(bundle: &Bundle, change: impl Fn(&mut Vec<u8>, &mut String)) -> Vec<(String, Bundle)> {
    let mut out = Vec::new();
    let mut push = |name: &str, edit: &dyn Fn(&mut Bundle)| {
        let mut changed = bundle.clone();
        edit(&mut changed);
        out.push((name.to_owned(), changed));
    };
    push("config", &|b| {
        change(&mut b.config.bytes, &mut b.config.sha256)
    });
    push("subjects", &|b| {
        change(&mut b.subjects.bytes, &mut b.subjects.sha256)
    });
    push("plan", &|b| change(&mut b.plan.bytes, &mut b.plan.sha256));
    let optional: [(&str, OptionalItem); 5] = [
        ("issue document", |b| b.issue_document.as_mut()),
        ("analysis", |b| b.analysis.as_mut()),
        ("screen", |b| b.screen.as_mut()),
        ("analysis record", |b| b.analysis_record.as_mut()),
        ("plan record", |b| b.plan_record.as_mut()),
    ];
    for (name, field) in optional {
        if field(&mut bundle.clone()).is_some() {
            push(name, &|b| {
                if let Some(item) = field(b) {
                    change(&mut item.bytes, &mut item.sha256);
                }
            });
        }
    }
    out
}

#[test]
fn a_mismatched_binding_is_unusable() {
    let runs = runs();
    // a run with every document, given both result records, so that each binding is tried
    let mut full = runs
        .iter()
        .find(|run| {
            let b = &run.bundle;
            b.issue_document.is_some() && b.analysis.is_some() && b.screen.is_some()
        })
        .expect("a run with an issue document, an analysis and a screen record")
        .bundle
        .clone();
    let record = br#"{"questions": []}"#.to_vec();
    full.analysis_record = Some(Item::new("analysis.result.json", record.clone()));
    full.plan_record = Some(Item::new("plan.result.json", record));
    assert!(
        plan_floor::floor(&full).is_ok(),
        "the untouched bundle is usable"
    );
    let byte = each_item(&full, |bytes, _| match bytes.first_mut() {
        Some(first) => *first ^= 0x20,
        None => bytes.push(b' '),
    });
    let digest = each_item(&full, |_, sha256| {
        let last = if sha256.ends_with('0') { "1" } else { "0" };
        sha256.pop();
        sha256.push_str(last);
    });
    assert_eq!(byte.len(), 8);
    let mut unread = byte[6].1.clone();
    unread.analysis = None;
    assert!(
        plan_floor::floor(&unread).is_ok(),
        "the analysis record is not read without the analysis"
    );
    for (name, bundle) in byte.into_iter().chain(digest) {
        let (outcome, text) = outcome(&bundle);
        assert_eq!(exit::code(&outcome), 2, "{name}: {outcome:?}");
        assert!(text.is_none(), "{name}");
        let message = outcome.err().map(|e| e.message().to_owned());
        assert!(
            message.is_some_and(|m| m.contains("does not match its sha256 binding")),
            "{name}"
        );
    }
}

#[test]
fn the_plan_floor_fixture_is_fresh() {
    // outside the repository, so the check leaves the source tree untouched
    let out = env::temp_dir().join(format!("mintswarm-plan-floor-{}.jsonl", process::id()));
    let output = Command::new("python3")
        .arg("-I")
        .arg("-B")
        .arg(fixtures().join("plan_floor_capture.py"))
        .arg(&out)
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .output()
        .expect("python3 runs plan_floor_capture.py");
    let captured = fs::read(&out);
    let _ = fs::remove_file(&out);
    assert!(
        output.status.success(),
        "plan_floor_capture.py failed: {}\n{}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    let captured = captured.expect("plan_floor_capture.py wrote its output");
    let committed = fs::read(fixtures().join("plan_floor.jsonl")).expect("the committed fixture");
    assert!(
        captured == committed,
        "tests/fixtures/plan_floor.jsonl is stale against floor.py and its self-test; refresh it \
         with:\n    {REFRESH}"
    );
}
