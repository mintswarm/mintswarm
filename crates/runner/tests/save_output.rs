//! `mintswarm-runner tool save-output` against `.mintswarm/dogfood/save-output.py`: every call
//! its self-test makes, recorded live by the rules crate's `save-output-record.py`, is replayed
//! through the built binary. Exit code, stdout and the bytes of both saved files must equal the
//! recording. On exit 1, stderr is `mintswarm-rules`' own wording, the reference, and matches
//! `save-output.py`'s on the prefix, the broken rule and its quoted text; Python's `repr` and its
//! JSON decoder's messages are never compared.
//!
//! The command-line tests check that a run the tool refuses leaves nothing behind.

use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::{self, Command, Output};
use std::{env, fs};

use mintswarm_domain::records::Json;
use mintswarm_rules::json;
use mintswarm_rules::output::{STAGES, capture, capture_transcript};

const BIN: &str = env!("CARGO_BIN_EXE_mintswarm-runner");

/// The tests of the self-test that never run save-output.py; the first is mirrored by
/// `write_both_cleans_up_on_any_exception` in `src/tool/save_output.rs`.
const UNRECORDED: &[&str] = &[
    "Questions.test_write_both_cleans_up_on_any_exception",
    "Skeletons.test_stages_are_known",
];

/// Runs the rules crate's recorder into a temporary file and returns its lines.
fn record() -> Vec<Json> {
    let recorder =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../rules/tests/fixtures/save-output-record.py");
    // outside the repository, so the run leaves the source tree untouched
    let out = env::temp_dir().join(format!(
        "mintswarm-runner-save-output-{}.jsonl",
        process::id()
    ));
    let run = Command::new("python3")
        .arg("-I")
        .arg("-B")
        .arg(&recorder)
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
        .map(|line| json::read(line).expect("each recorded line is JSON"))
        .collect()
}

fn field<'v>(value: &'v Json, key: &str) -> &'v Json {
    match value {
        Json::Object(pairs) => pairs
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v)
            .unwrap_or_else(|| panic!("{key} is recorded")),
        _ => panic!("a recorded line is an object"),
    }
}

fn text<'v>(value: &'v Json, key: &str) -> &'v str {
    match field(value, key) {
        Json::String(s) => s,
        other => panic!("{key} is a string, not {other:?}"),
    }
}

fn names_of<'v>(value: &'v Json, key: &str) -> Vec<&'v str> {
    match field(value, key) {
        Json::Array(items) => items
            .iter()
            .map(|item| match item {
                Json::String(s) => s.as_str(),
                other => panic!("{key} lists strings, not {other:?}"),
            })
            .collect(),
        other => panic!("{key} is an array, not {other:?}"),
    }
}

/// The names in `dir`, sorted.
fn names(dir: &Path) -> BTreeSet<String> {
    fs::read_dir(dir)
        .expect("the directory is readable")
        .map(|entry| {
            entry
                .expect("an entry is readable")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect()
}

fn set(names: &[&str]) -> BTreeSet<String> {
    names.iter().map(|name| (*name).to_string()).collect()
}

/// Runs `mintswarm-runner tool save-output` with `operands` in `dir`.
fn save_output<S: AsRef<OsStr>>(dir: &Path, operands: &[S]) -> Output {
    Command::new(BIN)
        .arg("tool")
        .arg("save-output")
        .args(operands)
        .current_dir(dir)
        .output()
        .expect("mintswarm-runner runs")
}

/// Captured output decoded with its CR translated to LF, as the recorder's `text=True` capture
/// did.
fn decoded(bytes: &[u8]) -> String {
    String::from_utf8(bytes.to_vec())
        .expect("the output is UTF-8")
        .replace("\r\n", "\n")
        .replace('\r', "\n")
}

/// The rule name and quoted text of a fault, from its stderr: the last line that starts
/// `breaks <rule> of <STAGES>: `, with the rest of the stderr as the quote.
fn broken_rule(stderr: &str) -> Option<(&str, &str)> {
    let at = stderr.rfind("\nbreaks ")?;
    let (rule, quote) = stderr[at + "\nbreaks ".len()..].split_once(&format!(" of {STAGES}: "))?;
    Some((rule, quote.strip_suffix('\n').unwrap_or(quote)))
}

/// The record's path for `output`, as `save-output.py` places it.
fn record_name(file: &str) -> String {
    format!("{}.result.json", file.strip_suffix(".md").unwrap_or(file))
}

/// The mismatches between what `entry` recorded and what the binary gives.
fn replay(entry: &Json) -> Vec<String> {
    let dir = tempfile::tempdir().expect("a temporary directory");
    let input = text(entry, "input");
    fs::write(dir.path().join("input"), input.as_bytes()).expect("the input is written");
    let recorded_output = text(entry, "output");
    let recorded_path = Path::new(recorded_output);
    let file = recorded_path
        .file_name()
        .and_then(OsStr::to_str)
        .expect("the recorded output names a file");
    let parent = recorded_path
        .parent()
        .and_then(Path::file_name)
        .and_then(OsStr::to_str)
        .expect("the recorded output has a parent directory");
    let output: PathBuf = dir.path().join(parent).join(file);
    let output_str = output.to_str().expect("a UTF-8 path");
    let text_mode = text(entry, "mode") == "text";

    let mut operands = Vec::new();
    if text_mode {
        operands.push("--text");
    }
    operands.extend(["input", output_str]);
    let run = save_output(dir.path(), &operands);
    let code = run.status.code();
    let stdout = decoded(&run.stdout).replace(output_str, recorded_output);
    let stderr = decoded(&run.stderr);

    let mut wrong = Vec::new();
    let recorded_exit = match field(entry, "exit") {
        Json::Int(exit) => *exit,
        other => panic!("exit is an integer, not {other:?}"),
    };
    if code != i32::try_from(recorded_exit).ok() {
        wrong.push(format!("exit {code:?} != {recorded_exit}"));
    }
    match recorded_exit {
        0 => {
            if stdout != text(entry, "stdout") {
                wrong.push(format!("stdout {stdout:?} != {:?}", text(entry, "stdout")));
            }
            if stderr != text(entry, "stderr") {
                wrong.push(format!("stderr {stderr:?} != {:?}", text(entry, "stderr")));
            }
            let record = record_name(file);
            for (name, key) in [(file, "document"), (record.as_str(), "record")] {
                let saved = fs::read(dir.path().join(parent).join(name));
                if saved.as_deref().ok() != Some(text(entry, key).as_bytes()) {
                    wrong.push(format!("{key}: {saved:?} != {:?}", text(entry, key)));
                }
            }
            if names(dir.path()) != set(&["input", parent]) {
                wrong.push(format!("left behind: {:?}", names(dir.path())));
            }
            let saved = names(&dir.path().join(parent));
            if saved != set(&[file, &record]) {
                wrong.push(format!("left beside the output: {saved:?}"));
            }
        }
        1 => {
            if stdout != text(entry, "stdout") {
                wrong.push(format!("stdout {stdout:?} != {:?}", text(entry, "stdout")));
            }
            let (message, result) = if text_mode {
                (None, capture(input))
            } else {
                let found = capture_transcript(input);
                (found.message.map(|m| m.describe()), found.result)
            };
            let prefix = match &message {
                Some(message) => format!("save-output: {message}: "),
                None => "save-output: ".to_string(),
            };
            match result {
                Ok(_) => wrong.push("mintswarm-rules saves what Python rejected".into()),
                Err(fault) => {
                    let expected = format!("{prefix}{}\n", fault.explain());
                    if stderr != expected {
                        wrong.push(format!("stderr {stderr:?} != {expected:?}"));
                    }
                    let python = text(entry, "stderr");
                    let names_a_message = python.starts_with("save-output: assistant message ");
                    if !python.starts_with(&prefix) || (message.is_none() && names_a_message) {
                        wrong.push(format!("prefix {prefix:?} differs from {python:?}"));
                    }
                    if broken_rule(python) != Some((fault.rule, fault.quote)) {
                        wrong.push(format!(
                            "rule {:?} != {:?}",
                            (fault.rule, fault.quote),
                            broken_rule(python)
                        ));
                    }
                }
            }
            if names(dir.path()) != set(&["input"]) {
                wrong.push(format!("left behind: {:?}", names(dir.path())));
            }
        }
        _ => {
            if !stdout.is_empty() || !stderr.starts_with("save-output: cannot ") {
                wrong.push(format!("exit 2 gave {stdout:?} and {stderr:?}"));
            }
            if names(dir.path()) != set(&["input"]) {
                wrong.push(format!("left behind: {:?}", names(dir.path())));
            }
        }
    }
    wrong
        .into_iter()
        .map(|w| format!("{}: {w}", text(entry, "test")))
        .collect()
}

#[test]
fn every_save_output_call_of_the_self_test_is_reproduced() {
    let mut lines = record();
    let summary = lines.pop().expect("the recorder's summary line");
    assert_eq!(
        names_of(&summary, "unrecorded"),
        UNRECORDED,
        "tests that made no save-output.py call"
    );
    // a test that raises may stop before all its calls, so fewer are recorded
    let errored = names_of(&summary, "errored");
    assert!(
        errored.is_empty(),
        "self-test errors: {}",
        errored.join(", ")
    );

    let mut mismatches = Vec::new();
    let (mut text_mode, mut transcript_mode) = (0, 0);
    for entry in &lines {
        match text(entry, "mode") {
            "text" => text_mode += 1,
            _ => transcript_mode += 1,
        }
        mismatches.extend(replay(entry));
    }
    eprintln!("replayed {text_mode} text and {transcript_mode} transcript calls");
    assert!(
        mismatches.is_empty(),
        "{} differences over {} calls:\n{}",
        mismatches.len(),
        lines.len(),
        mismatches.join("\n")
    );
    assert!(text_mode > 0, "no text-mode call was replayed");
    assert!(transcript_mode > 0, "no transcript-mode call was replayed");
}

/// A reply `save-output.py --text` saves.
const REPLY: &str = "````markdown\n# Review\n\nAll good.\n````\n\n```yaml\nmsw-result:\n  \
                     status: complete\n  artefact: review\n  outputs: \
                     [.mintswarm/job/outputs/review.md]\n  questions: []\n  reason: \"\"\n```\n";

fn assert_refused(run: &Output, start: &str) {
    assert_eq!(run.status.code(), Some(2), "{run:?}");
    assert!(run.stdout.is_empty(), "{run:?}");
    assert!(
        String::from_utf8_lossy(&run.stderr).starts_with(start),
        "{run:?}"
    );
}

#[test]
fn the_reply_used_by_the_cli_tests_is_saved() {
    let dir = tempfile::tempdir().expect("a temporary directory");
    fs::write(dir.path().join("input"), REPLY).expect("the input is written");
    let run = save_output(dir.path(), &["--text", "input", "out/review.md"]);
    assert_eq!(run.status.code(), Some(0), "{run:?}");
    assert!(
        String::from_utf8_lossy(&run.stdout).ends_with("saved 20 characters to out/review.md\n"),
        "{run:?}"
    );
    assert_eq!(
        names(&dir.path().join("out")),
        set(&["review.md", "review.result.json"])
    );
}

#[test]
fn usage_errors_leave_the_directory_empty() {
    let dir = tempfile::tempdir().expect("a temporary directory");
    let usage = "usage: mintswarm-runner tool save-output [--text] INPUT OUTPUT.md\n";
    let rejected: &[&[&str]] = &[
        &[],
        &["input"],
        &["input", "out.md", "more"],
        &["--", "input", "out.md"],
        &["--text", "--", "out.md"],
        &["input", "--text", "out.md"],
        &["--help", "out.md"],
    ];
    for operands in rejected {
        let run = save_output(dir.path(), operands);
        assert_eq!(run.status.code(), Some(2), "{operands:?}");
        assert!(run.stdout.is_empty(), "{operands:?}");
        assert_eq!(String::from_utf8_lossy(&run.stderr), usage, "{operands:?}");
    }
    assert_eq!(names(dir.path()), set(&[]));
}

#[test]
fn an_unreadable_input_writes_nothing() {
    let dir = tempfile::tempdir().expect("a temporary directory");
    fs::write(dir.path().join("latin1"), b"# R\xe9sum\xe9\n").expect("the input is written");
    for operands in [
        ["--text", "missing", "out/review.md"],
        ["--text", "latin1", "out/review.md"],
    ] {
        let run = save_output(dir.path(), &operands);
        assert_refused(&run, &format!("save-output: cannot read {}: ", operands[1]));
    }
    let run = save_output(dir.path(), &["latin1", "out/review.md"]);
    assert_refused(&run, "save-output: cannot read latin1: ");
    assert_eq!(names(dir.path()), set(&["latin1"]));
}

#[test]
fn a_parent_that_is_a_file_leaves_nothing_behind() {
    let dir = tempfile::tempdir().expect("a temporary directory");
    fs::write(dir.path().join("input"), REPLY).expect("the input is written");
    fs::write(dir.path().join("blocker"), "").expect("a regular file");
    let run = save_output(dir.path(), &["--text", "input", "blocker/review.md"]);
    assert_refused(&run, "save-output: cannot write blocker/review.md: ");
    assert_eq!(names(dir.path()), set(&["blocker", "input"]));
}

#[test]
fn a_record_blocked_by_a_directory_leaves_no_output_and_no_temporary() {
    let dir = tempfile::tempdir().expect("a temporary directory");
    fs::write(dir.path().join("input"), REPLY).expect("the input is written");
    let record = dir.path().join("out/review.result.json");
    fs::create_dir_all(&record).expect("a directory where the record goes");
    fs::write(record.join("keep"), "").expect("a file that keeps it non-empty");
    let run = save_output(dir.path(), &["--text", "input", "out/review.md"]);
    assert_refused(&run, "save-output: cannot write out/review.md: ");
    assert_eq!(names(&dir.path().join("out")), set(&["review.result.json"]));
    assert_eq!(names(&record), set(&["keep"]));
}
