//! `mintswarm-runner tool save-output` against `.mintswarm/dogfood/save-output.py`: every call
//! its self-test makes, recorded live by the rules crate's `save-output-record.py`, is replayed
//! through the built binary. Exit code, stdout, stderr and the bytes of both saved files must
//! equal the recording. A recorded call that exits 2 is not replayed: its cause lies in the file
//! system, which the recording does not hold.
//!
//! The command-line tests run `save-output.py` and the binary side by side, each in its own copy
//! of the same set-up with the same operands, and require the same exit code, stdout, stderr and
//! directory contents; they cover the exit-2 cases, a refused run leaving nothing behind.

use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{self, Command, Output};
use std::{env, fs};

use mintswarm_domain::records::Json;
use mintswarm_rules::json;

const BIN: &str = env!("CARGO_BIN_EXE_mintswarm-runner");

/// The tests of the self-test that never run save-output.py: those of `errors.py` and
/// `save-output.py`'s source, and `test_write_both_cleans_up_on_any_exception`, mirrored by the
/// unit test of that name in `src/tool/save_output.rs`.
const UNRECORDED: &[&str] = &[
    "ErrorModule.test_crash_line_for_an_unexpected_exception",
    "ErrorModule.test_decode_position",
    "ErrorModule.test_domain_error_and_crash_line",
    "ErrorModule.test_no_exception_text_in_source",
    "ErrorModule.test_read_phrase",
    "ErrorModule.test_standard_library_only",
    "ErrorModule.test_write_phrase",
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

/// The record's path for `output`, as `save-output.py` places it.
fn record_name(file: &str) -> String {
    format!("{}.result.json", file.strip_suffix(".md").unwrap_or(file))
}

/// The mismatches between what `entry`, a call that exited 0 or 1, recorded and what the binary
/// gives.
fn replay(entry: &Json) -> Vec<String> {
    let dir = tempfile::tempdir().expect("a temporary directory");
    let input = text(entry, "input");
    fs::write(dir.path().join("input"), input.as_bytes()).expect("the input is written");
    let recorded_output = text(entry, "output");
    let recorded_path = Path::new(recorded_output);
    // an output that names no file, such as `/`, is passed as given; only a rejection reaches it
    let placed = recorded_path
        .file_name()
        .and_then(OsStr::to_str)
        .map(|file| {
            let parent = recorded_path
                .parent()
                .and_then(Path::file_name)
                .and_then(OsStr::to_str)
                .expect("the recorded output has a parent directory");
            (parent, file)
        });
    let output: PathBuf = match placed {
        Some((parent, file)) => dir.path().join(parent).join(file),
        None => PathBuf::from(recorded_output),
    };
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
    if stdout != text(entry, "stdout") {
        wrong.push(format!("stdout {stdout:?} != {:?}", text(entry, "stdout")));
    }
    if stderr != text(entry, "stderr") {
        wrong.push(format!("stderr {stderr:?} != {:?}", text(entry, "stderr")));
    }
    match (recorded_exit, placed) {
        (0, Some((parent, file))) => {
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
        (0, None) => wrong.push(format!("saved to {recorded_output:?}, which names no file")),
        _ => {
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
    let failed = names_of(&summary, "failed");
    assert!(
        failed.is_empty(),
        "self-test failures: {}",
        failed.join(", ")
    );

    let mut mismatches = Vec::new();
    let (mut text_mode, mut transcript_mode, mut skipped) = (0, 0, 0);
    for entry in &lines {
        if matches!(field(entry, "exit"), Json::Int(2)) {
            // the file system set-up behind an exit 2 is not recorded; the CLI tests cover it
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
        "{} differences over {} calls:\n{}",
        mismatches.len(),
        lines.len(),
        mismatches.join("\n")
    );
    assert!(text_mode > 0, "no text-mode call was replayed");
    assert!(transcript_mode > 0, "no transcript-mode call was replayed");
    assert!(skipped > 0, "no exit-2 call was recorded");
}

/// A reply `save-output.py --text` saves.
const REPLY: &str = "````markdown\n# Review\n\nAll good.\n````\n\n```yaml\nmsw-result:\n  \
                     status: complete\n  artefact: review\n  outputs: \
                     [.mintswarm/job/outputs/review.md]\n  questions: []\n  reason: \"\"\n```\n";

/// `save-output.py`, the reference for the command-line tests.
const SCRIPT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../.mintswarm/dogfood/save-output.py"
);

/// What a run gave: its exit code, stdout, stderr and the paths left in its directory.
#[derive(Debug, PartialEq)]
struct Ran {
    code: Option<i32>,
    stdout: String,
    stderr: String,
    left: Vec<String>,
}

impl Ran {
    fn of(run: &Output, dir: &Path) -> Self {
        Self {
            code: run.status.code(),
            stdout: String::from_utf8_lossy(&run.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&run.stderr).into_owned(),
            left: contents(dir),
        }
    }
}

/// Every path under `dir`, relative to it, sorted.
fn contents(dir: &Path) -> Vec<String> {
    fn walk(root: &Path, dir: &Path, found: &mut Vec<String>) {
        for entry in fs::read_dir(dir).expect("the directory is readable") {
            let path = entry.expect("an entry is readable").path();
            let relative = path.strip_prefix(root).expect("a path under the root");
            found.push(relative.to_string_lossy().into_owned());
            if path.is_dir() && !path.is_symlink() {
                walk(root, &path, found);
            }
        }
    }
    let mut found = Vec::new();
    walk(dir, dir, &mut found);
    found.sort();
    found
}

/// Runs `save-output.py` and the binary with `operands`, each in its own temporary directory
/// built by `setup`, asserts that both give the same exit code, stdout, stderr and directory
/// contents, and returns that. `setup` returns false when the set-up does not give the case it
/// is for, as when mode bits do not bind; the case is then skipped and `None` returned.
fn side_by_side(setup: impl Fn(&Path) -> bool, operands: &[&str]) -> Option<Ran> {
    let (python_dir, rust_dir) = (
        tempfile::tempdir().expect("a temporary directory"),
        tempfile::tempdir().expect("a temporary directory"),
    );
    if !setup(python_dir.path()) || !setup(rust_dir.path()) {
        eprintln!("skipped {operands:?}: the set-up is accessible here");
        return None;
    }
    let python = Command::new("python3")
        .arg("-I")
        .arg("-B")
        .arg(SCRIPT)
        .args(operands)
        .current_dir(python_dir.path())
        .output()
        .expect("python3 runs save-output.py");
    let rust = save_output(rust_dir.path(), operands);
    let (python, rust) = (
        Ran::of(&python, python_dir.path()),
        Ran::of(&rust, rust_dir.path()),
    );
    assert_eq!(
        rust, python,
        "the binary and save-output.py differ on {operands:?}"
    );
    Some(rust)
}

/// Asserts that `ran` refused the run with exit 2, nothing on stdout, exactly `line` on stderr
/// and the set-up left as `left`.
fn assert_refused(ran: Option<Ran>, line: &str, left: &[&str]) {
    let Some(ran) = ran else { return };
    let left: Vec<String> = left.iter().map(|path| (*path).to_string()).collect();
    assert_eq!(
        ran,
        Ran {
            code: Some(2),
            stdout: String::new(),
            stderr: format!("{line}\n"),
            left,
        }
    );
}

/// A set-up with the reply as `input`.
fn reply(dir: &Path) -> bool {
    fs::write(dir.join("input"), REPLY).expect("the input is written");
    true
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
fn an_output_with_a_trailing_slash_is_saved_as_pathlib_reads_it() {
    let dir = tempfile::tempdir().expect("a temporary directory");
    fs::write(dir.path().join("input"), REPLY).expect("the input is written");
    let run = save_output(dir.path(), &["--text", "input", "out/review.md/"]);
    assert_eq!(run.status.code(), Some(0), "{run:?}");
    assert!(
        String::from_utf8_lossy(&run.stdout).ends_with("saved 20 characters to out/review.md/\n"),
        "{run:?}"
    );
    assert_eq!(names(dir.path()), set(&["input", "out"]));
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
    let latin1 = |dir: &Path| {
        fs::write(dir.join("latin1"), b"# R\xe9sum\xe9\n").expect("the input is written");
        true
    };
    let cases: &[(&[&str], &str)] = &[
        (
            &["--text", "latin1", "out/review.md"],
            "latin1: not UTF-8 text",
        ),
        (&["latin1", "out/review.md"], "latin1: not UTF-8 text"),
        (
            &["--text", "missing", "out/review.md"],
            "missing: no such file",
        ),
        (&["--text", "missing", "/"], "missing: no such file"),
        (&["missing", "out/review.md"], "missing: no such file"),
    ];
    for (operands, why) in cases {
        let line = format!("save-output: cannot read {why}");
        assert_refused(side_by_side(latin1, operands), &line, &["latin1"]);
    }
}

#[test]
fn an_input_that_is_no_file_writes_nothing() {
    let directory = |dir: &Path| {
        fs::create_dir(dir.join("input")).expect("a directory");
        true
    };
    assert_refused(
        side_by_side(directory, &["--text", "input", "out/review.md"]),
        "save-output: cannot read input: read failed",
        &["input"],
    );
    assert_refused(
        side_by_side(reply, &["--text", "input/reply", "out/review.md"]),
        "save-output: cannot read input/reply: read failed",
        &["input"],
    );
}

#[test]
fn a_parent_that_is_a_file_leaves_nothing_behind() {
    let blocker = |dir: &Path| {
        reply(dir);
        fs::write(dir.join("blocker"), "").expect("a regular file");
        true
    };
    for output in ["blocker/review.md", "blocker/sub/review.md"] {
        assert_refused(
            side_by_side(blocker, &["--text", "input", output]),
            &format!("save-output: cannot write {output}: a parent is not a directory"),
            &["blocker", "input"],
        );
    }
}

#[test]
fn a_record_blocked_by_a_directory_leaves_no_output_and_no_temporary() {
    let blocked = |dir: &Path| {
        reply(dir);
        let record = dir.join("out/review.result.json");
        fs::create_dir_all(&record).expect("a directory where the record goes");
        fs::write(record.join("keep"), "").expect("a file that keeps it non-empty");
        true
    };
    assert_refused(
        side_by_side(blocked, &["--text", "input", "out/review.md"]),
        "save-output: cannot write out/review.md: a directory is in the way",
        &[
            "input",
            "out",
            "out/review.result.json",
            "out/review.result.json/keep",
        ],
    );
}

#[test]
fn an_output_that_names_no_file_is_refused_after_the_reply_check() {
    for output in ["/", ".", ".."] {
        assert_refused(
            side_by_side(reply, &["--text", "input", output]),
            &format!("save-output: cannot write {output}: it names no file"),
            &["input"],
        );
    }
    // a rejected reply is reported first, whatever OUTPUT is
    let rejected = |dir: &Path| {
        fs::write(dir.join("input"), "No document here.\n").expect("the input is written");
        true
    };
    let ran = side_by_side(rejected, &["--text", "input", "/"]).expect("no skip");
    assert_eq!(ran.code, Some(1), "{ran:?}");
    assert!(ran.stdout.is_empty(), "{ran:?}");
    assert!(
        ran.stderr.starts_with("save-output: no document block"),
        "{ran:?}"
    );
}

#[test]
fn permission_denied_is_reported_in_the_same_words() {
    // the accessible case always runs, so the test never only skips
    let readable = |dir: &Path| {
        reply(dir);
        fs::create_dir(dir.join("out")).expect("a directory");
        true
    };
    let ran = side_by_side(readable, &["--text", "input", "out/review.md"]).expect("no skip");
    assert_eq!(ran.code, Some(0), "{ran:?}");

    let unreadable = |dir: &Path| {
        reply(dir);
        let input = dir.join("input");
        fs::set_permissions(&input, fs::Permissions::from_mode(0o000)).expect("mode 000");
        fs::File::open(&input).is_err()
    };
    assert_refused(
        side_by_side(unreadable, &["--text", "input", "out/review.md"]),
        "save-output: cannot read input: permission denied",
        &["input"],
    );

    let read_only = |dir: &Path| {
        reply(dir);
        let out = dir.join("out");
        fs::create_dir(&out).expect("a directory");
        fs::set_permissions(&out, fs::Permissions::from_mode(0o555)).expect("mode 555");
        let probe = out.join("probe");
        let writable = fs::write(&probe, "").is_ok();
        let _ = fs::remove_file(&probe);
        !writable
    };
    assert_refused(
        side_by_side(read_only, &["--text", "input", "out/review.md"]),
        "save-output: cannot write out/review.md: permission denied",
        &["input", "out"],
    );
}
