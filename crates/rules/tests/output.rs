//! Stage output capture: the quoted rules, the stage-aware checks, the result-block fixtures and
//! transcript edge cases beyond `save-output-selftest.py`.

#![cfg(test)]

use std::fs;
use std::path::{Path, PathBuf};

use mintswarm_domain::work::{OutcomeStatus, StageOutcome};
use mintswarm_rules::output::{
    CONTENT_RULE, EXPECTED_RULE, RULES, capture, capture_transcript, check_expected,
};

const DOCUMENT: &str = "# Review round 1: An issue\n\nPass, no findings.\n\n## Findings\n\n```yaml\nfindings: []\n```\n";

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// A reply holding DOCUMENT and a result block made of `lines`, as the self-test's `result_reply`.
fn result_reply(lines: &[&str]) -> String {
    format!(
        "````markdown\n{DOCUMENT}````\n\n```yaml\n{}\n```\n",
        lines.join("\n")
    )
}

fn outcome(status: OutcomeStatus, artefact: &str, outputs: &[&str], reason: &str) -> StageOutcome {
    StageOutcome {
        status,
        artefact: artefact.into(),
        outputs: outputs.iter().map(|o| o.to_string()).collect(),
        questions: Vec::new(),
        reason: reason.into(),
        commit: None,
        pr_title: None,
    }
}

/// The violations a fault lists, without their `- `.
fn listed(fault: &str) -> Vec<&str> {
    fault
        .lines()
        .filter_map(|line| line.strip_prefix("- "))
        .collect()
}

#[test]
fn every_quoted_rule_is_verbatim_in_stages_md() {
    let stages = fs::read_to_string(root().join(".mintswarm/stages.md")).unwrap();
    for (name, text) in RULES {
        assert!(stages.contains(text), "{name} is not verbatim in stages.md");
    }
    assert!(stages.contains(CONTENT_RULE));
    assert!(stages.contains(EXPECTED_RULE));
}

#[test]
fn check_expected_accepts_the_stage_outcome() {
    let outputs = [".mintswarm/job/outputs/plan.md"];
    assert!(
        check_expected(
            &outcome(OutcomeStatus::Complete, "plan", &outputs, ""),
            "plan"
        )
        .is_ok()
    );
    let failed = outcome(
        OutcomeStatus::Failed,
        "plan",
        &outputs,
        "the gates never pass",
    );
    assert!(check_expected(&failed, "plan").is_ok());
}

#[test]
fn check_expected_names_each_field() {
    let outputs = [".mintswarm/job/outputs/plan.md"];
    let cases = [
        (
            outcome(OutcomeStatus::Complete, "analysis", &outputs, ""),
            "artefact:",
        ),
        (
            outcome(
                OutcomeStatus::Complete,
                "plan",
                &[".mintswarm/job/outputs/analysis.md"],
                "",
            ),
            "outputs:",
        ),
        (
            outcome(
                OutcomeStatus::Complete,
                "plan",
                &[".mintswarm/job/outputs/plan.md", "extra.md"],
                "",
            ),
            "outputs:",
        ),
        (
            outcome(OutcomeStatus::Complete, "plan", &[], ""),
            "outputs:",
        ),
        (
            outcome(OutcomeStatus::Failed, "plan", &outputs, ""),
            "reason:",
        ),
        (
            outcome(OutcomeStatus::Failed, "plan", &outputs, " \t\u{3000}"),
            "reason:",
        ),
    ];
    for (outcome, field) in cases {
        let fault = check_expected(&outcome, "plan").expect_err(field);
        assert_eq!(fault.rule, "Result block");
        assert_eq!(fault.quote, EXPECTED_RULE);
        let found = listed(&fault.fault);
        assert_eq!(found.len(), 1, "{}", fault.fault);
        assert!(found[0].starts_with(field), "{}", fault.fault);
    }
    let all = outcome(OutcomeStatus::Failed, "review", &["x.md"], "");
    let fault = check_expected(&all, "plan").expect_err("three mismatches");
    let fields: Vec<&str> = listed(&fault.fault)
        .iter()
        .map(|line| line.split(':').next().unwrap())
        .collect();
    assert_eq!(fields, ["artefact", "outputs", "reason"]);
    assert!(
        fault
            .fault
            .starts_with("the result block has 3 violations:\n")
    );
}

/// `questions` empty under `needs_input` is rejected by `capture`, before `check_expected` runs,
/// under the `needs_input` status rule. Its message, which the domain's `StageOutcome::check`
/// gives, names the field through that rule rather than starting with `questions:`.
#[test]
fn needs_input_without_a_blocking_question_is_rejected() {
    let reply = result_reply(&[
        "msw-result:",
        "  status: needs_input",
        "  artefact: review",
        "  outputs: [.mintswarm/job/outputs/review.md]",
        "  questions: []",
        "  reason: \"\"",
    ]);
    let fault = capture(&reply).expect_err("needs_input with no question");
    assert_eq!(fault.rule, "Result block");
    assert_eq!(fault.quote, CONTENT_RULE);
    assert_eq!(
        listed(&fault.fault),
        ["needs_input needs a must_decide or recommended question"]
    );
}

#[test]
fn each_result_block_fixture_gives_its_record() {
    let fixtures = root().join("crates/domain/tests/fixtures/result-blocks");
    let mut cases: Vec<PathBuf> = fs::read_dir(&fixtures)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.is_dir())
        .collect();
    cases.sort();
    assert!(cases.len() >= 3, "too few result-block fixtures");
    for case in cases {
        let block = fs::read_to_string(case.join("block.yaml")).unwrap();
        let lines: Vec<&str> = block.trim_end_matches('\n').split('\n').collect();
        let expected = fs::read_to_string(case.join("result.json")).unwrap();
        let captured = capture(&result_reply(&lines))
            .unwrap_or_else(|fault| panic!("{}: {fault}", case.display()));
        assert_eq!(captured.record, expected, "{}", case.display());
        assert_eq!(captured.document, DOCUMENT);
        assert_eq!(captured.characters, DOCUMENT.chars().count());
    }
}

#[test]
fn transcript_edge_records_select_as_python_does() {
    let reply = result_reply(&[
        "msw-result:",
        "  status: complete",
        "  artefact: review",
        "  outputs: [.mintswarm/job/outputs/review.md]",
        "  questions: []",
        "  reason: \"\"",
    ]);
    let reply_json = serde_json::to_string(&reply).unwrap();
    let assistant = |id: &str, text: &str| {
        format!(
            r#"{{"type": "assistant", "message": {{"id": "{id}", "role": "assistant", "content": [{{"type": "text", "text": {text}}}]}}}}"#
        )
    };
    let transcript = [
        // a lone CR ends a line, as Python's universal newlines read it
        format!("{}\r{}", assistant("msg_1", &reply_json), assistant("msg_2", "\"Saved.\"")),
        // NaN is a number to Python's json.loads, so the record counts
        r#"{"type": "assistant", "cost": NaN, "message": {"id": "msg_3", "role": "assistant", "content": [{"type": "text", "text": "Costed."}]}}"#.into(),
        // a repeated key keeps its last value, so this record counts too
        r#"{"type": "user", "type": "assistant", "message": {"id": "msg_4", "role": "assistant", "content": "A plain string."}}"#.into(),
        // and this one does not
        r#"{"type": "assistant", "type": "user", "message": {"id": "msg_5", "role": "assistant", "content": "Not counted."}}"#.into(),
        "not json".into(),
    ]
    .join("\n");
    let found = capture_transcript(&transcript);
    let message = found.message.expect("a message is selected");
    assert_eq!(message.describe(), "assistant message 1 of 4 (msg_1)");
    assert_eq!(found.result.expect("the reply is saved").document, DOCUMENT);

    let none = capture_transcript("{\"type\": \"user\"}\n");
    assert_eq!(none.message, None);
    let fault = none.result.expect_err("no assistant text");
    assert_eq!(fault.fault, "no assistant text in the transcript");
    assert_eq!(fault.rule, "rule 1");
}
