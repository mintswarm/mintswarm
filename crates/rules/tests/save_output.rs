//! Parity with `.mintswarm/dogfood/save-output.py`: every call its self-test makes, recorded live
//! by `tests/fixtures/save-output-record.py`, gives the same saved bytes, stdout and stderr
//! through `mintswarm_rules::output`.

#![cfg(test)]

use std::path::Path;
use std::process::Command;
use std::{env, fs, process};

use mintswarm_rules::markdown::py_rstrip;
use mintswarm_rules::output::{capture, capture_transcript};
use serde_json::Value;

/// The tests of the self-test that never run save-output.py.
const UNRECORDED: &[&str] = &[
    "Questions.test_write_both_cleans_up_on_any_exception",
    "Skeletons.test_stages_are_known",
];

/// Runs the recorder into a temporary file and returns its lines.
fn record() -> Vec<Value> {
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    // outside the repository, so the run leaves the source tree untouched
    let out = env::temp_dir().join(format!(
        "mintswarm-rules-save-output-{}.jsonl",
        process::id()
    ));
    let run = Command::new("python3")
        .arg("-I")
        .arg("-B")
        .arg(fixtures.join("save-output-record.py"))
        .arg(&out)
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .output()
        .expect("python3 runs save-output-record.py");
    let recorded = fs::read_to_string(&out);
    let _ = fs::remove_file(&out);
    assert!(
        run.status.success(),
        "save-output-record.py failed: {}\n{}",
        run.status,
        String::from_utf8_lossy(&run.stderr)
    );
    recorded
        .expect("save-output-record.py wrote its output")
        .lines()
        .map(|line| serde_json::from_str(line).expect("each recorded line is JSON"))
        .collect()
}

fn text<'v>(entry: &'v Value, key: &str) -> &'v str {
    entry[key]
        .as_str()
        .unwrap_or_else(|| panic!("{key} is a string in {entry}"))
}

/// The mismatch between what `entry` recorded and what the Rust port gives, if any.
fn replay(entry: &Value) -> Option<String> {
    let input = text(entry, "input");
    let output = text(entry, "output");
    let (where_, result) = match text(entry, "mode") {
        "text" => (None, capture(input)),
        _ => {
            let found = capture_transcript(input);
            (found.message.map(|m| m.describe()), found.result)
        }
    };
    let mut wrong = Vec::new();
    match (entry["exit"].as_i64(), result) {
        (Some(0), Ok(capture)) => {
            if Some(capture.document.as_str()) != entry["document"].as_str() {
                wrong.push(format!(
                    "document: {:?} != {}",
                    capture.document, entry["document"]
                ));
            }
            if Some(capture.record.as_str()) != entry["record"].as_str() {
                wrong.push(format!(
                    "record: {:?} != {}",
                    capture.record, entry["record"]
                ));
            }
            let from = where_.map_or(String::new(), |w| format!("document from {w}\n"));
            let stdout = format!(
                "{}\n{from}saved {} characters to {output}\n",
                py_rstrip(&capture.result),
                capture.characters
            );
            if stdout != text(entry, "stdout") {
                wrong.push(format!("stdout: {stdout:?} != {:?}", text(entry, "stdout")));
            }
        }
        (Some(1), Err(fault)) => {
            let prefix = where_.map_or(String::new(), |w| format!("{w}: "));
            let stderr = format!("save-output: {prefix}{}\n", fault.explain());
            if stderr != text(entry, "stderr") {
                wrong.push(format!("stderr: {stderr:?} != {:?}", text(entry, "stderr")));
            }
        }
        (Some(0), Err(fault)) => wrong.push(format!("rejected what Python saved: {fault}")),
        (_, Ok(_)) => wrong.push(format!(
            "saved what Python rejected: {}",
            text(entry, "stderr")
        )),
        (exit, Err(_)) => wrong.push(format!("unexpected exit {exit:?}")),
    }
    (!wrong.is_empty()).then(|| format!("{}: {}", text(entry, "test"), wrong.join("\n  ")))
}

#[test]
fn every_save_output_call_of_the_self_test_is_reproduced() {
    let mut lines = record();
    let summary = lines.pop().expect("the recorder's summary line");
    let unrecorded: Vec<&str> = summary["unrecorded"]
        .as_array()
        .expect("the summary lists the unrecorded tests")
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert_eq!(
        unrecorded, UNRECORDED,
        "tests that made no save-output.py call"
    );

    let mut mismatches = Vec::new();
    let (mut text_mode, mut transcript_mode, mut skipped) = (0, 0, 0);
    for entry in &lines {
        if !matches!(entry["exit"].as_i64(), Some(0 | 1)) {
            // usage and I/O errors belong to the command line, not the rules
            skipped += 1;
            continue;
        }
        match text(entry, "mode") {
            "text" => text_mode += 1,
            _ => transcript_mode += 1,
        }
        mismatches.extend(replay(entry));
    }
    eprintln!(
        "replayed {text_mode} text and {transcript_mode} transcript calls; skipped {skipped} with exit 2"
    );
    assert!(
        mismatches.is_empty(),
        "{} of {} calls differ:\n{}",
        mismatches.len(),
        lines.len(),
        mismatches.join("\n")
    );
    assert!(text_mode > 0, "no text-mode call was replayed");
    assert!(transcript_mode > 0, "no transcript-mode call was replayed");
}
