//! `tool plan-floor` and `tool plan-decision` against `plan-approval/floor.py` and `decision.py`.
//!
//! Parity: `fixtures/plan-approval-record.py` runs `.mintswarm/dogfood/plan-approval/selftest.py`
//! with every `floor.py` and `decision.py` call made through both sides, live, from the same
//! starting state. On each call the exit codes, stdout and the `floor.json` and `decision.json`
//! bytes must be equal, neither side may leave a temporary file or an object store, and neither
//! may change the job repository's refs, config, shallow, grafts, index or objects. Stderr is
//! the same on exit 0 and 1. On exit 2 it is one line each, in each side's own wording: an
//! argument refusal is the same after the prefix, with the usage line swapped, and a refusal that
//! names the fetch or a partial clone names it on both sides.
//!
//! The one difference accepted, by name in [`Difference`], and seen at least once:
//!
//! - `yq-missing`: with no `yq` on `PATH`, Python exits 2 with its one `cannot run yq: ` line; the
//!   Rust tools run no `yq` and give a verdict, exit 0 or 1, with the floor's stderr empty and
//!   the decision's its one `decision: ` line.
//!
//! The Rust tests below stand in for the self-test's cases that check the Python's internals.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{self, Command, Output};
use std::{env, fs};

use serde_json::Value;
use tempfile::TempDir;

const BIN: &str = env!("CARGO_BIN_EXE_mintswarm-runner");

fn repository() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Runs the recorder into a file outside the repository and returns its lines.
fn record() -> Vec<Value> {
    let root = repository();
    let out = env::temp_dir().join(format!(
        "mintswarm-runner-plan-approval-{}.jsonl",
        process::id()
    ));
    let run = Command::new("python3")
        .arg("-I")
        .arg("-B")
        .arg(root.join("crates/runner/tests/fixtures/plan-approval-record.py"))
        .arg(root.join(".mintswarm/dogfood/plan-approval/selftest.py"))
        .arg(BIN)
        .arg(&out)
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .output()
        .expect("python3 runs plan-approval-record.py");
    let recorded = fs::read_to_string(&out);
    let _ = fs::remove_file(&out);
    assert!(
        run.status.success(),
        "plan-approval-record.py failed: {}\n{}",
        run.status,
        String::from_utf8_lossy(&run.stderr)
    );
    recorded
        .expect("plan-approval-record.py wrote its output")
        .lines()
        .map(|line| serde_json::from_str(line).expect("each recorded line is JSON"))
        .collect()
}

/// A difference the parity check accepts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Difference {
    YqMissing,
}

/// One side of a recorded call.
struct Side<'a> {
    value: &'a Value,
}

impl Side<'_> {
    fn exit(&self) -> i64 {
        self.value["exit"].as_i64().expect("an exit code")
    }

    fn bytes(&self, key: &str) -> Vec<u8> {
        hex(self.value[key].as_str().expect("hex"))
    }

    fn text(&self, key: &str) -> String {
        String::from_utf8(self.bytes(key)).expect("UTF-8 output")
    }

    fn file(&self, name: &str) -> &Value {
        &self.value["files"][name]
    }

    fn file_bytes(&self, name: &str) -> Option<Vec<u8>> {
        let file = self.file(name);
        (file["state"] == "file").then(|| hex(file["hex"].as_str().expect("hex")))
    }

    fn left(&self) -> bool {
        !self.value["temporaries"]
            .as_array()
            .expect("a list")
            .is_empty()
            || !self.value["stores"].as_array().expect("a list").is_empty()
    }
}

fn hex(text: &str) -> Vec<u8> {
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).expect("a hex byte"))
        .collect()
}

/// A call with no `yq` on `PATH` where Python exits 2 and Rust gives a verdict: accepted only
/// when Python's stderr is its one `cannot run yq` line and Rust's is what a verdict prints.
fn yq_missing(
    tool: &str,
    python: &Side<'_>,
    rust: &Side<'_>,
    file: &str,
    python_prefix: &str,
) -> Result<Option<Difference>, String> {
    let python_err = python.text("stderr");
    let cannot_run = format!("{python_prefix}cannot run yq: ");
    if !(python_err.starts_with(&cannot_run)
        && python_err.ends_with('\n')
        && python_err.lines().count() == 1)
    {
        return Err(format!("no yq, and Python's stderr is {python_err:?}"));
    }
    if rust.file_bytes(file).is_none() {
        return Err(format!("no yq, and Rust wrote no {file}"));
    }
    let rust_err = rust.text("stderr");
    let expected = match tool {
        "floor" => rust_err.is_empty(),
        _ => rust_err.starts_with("decision: ") && rust_err.lines().count() == 1,
    };
    if !expected {
        return Err(format!("no yq, and Rust's stderr is {rust_err:?}"));
    }
    Ok(Some(Difference::YqMissing))
}

/// What is wrong with one recorded call, or the difference it shows; `Ok(None)` when equal.
fn compare(line: &Value) -> Result<Option<Difference>, String> {
    let tool = line["tool"].as_str().expect("a tool");
    let python = Side {
        value: &line["python"],
    };
    let rust = Side {
        value: &line["rust"],
    };
    if python.left() || rust.left() {
        return Err("a temporary or a store is left".to_owned());
    }
    for (name, side) in [("Python", &python), ("Rust", &rust)] {
        if side.value["repository_changed"] != false {
            return Err(format!("{name} changed the job repository"));
        }
    }
    let (file, python_prefix, rust_prefix, python_usage, rust_usage) = match tool {
        "floor" => (
            "floor.json",
            "floor.py: ",
            "plan-floor: ",
            "usage: floor.py",
            "usage: mintswarm-runner tool plan-floor",
        ),
        _ => (
            "decision.json",
            "decision.py: ",
            "plan-decision: ",
            "usage: decision.py",
            "usage: mintswarm-runner tool plan-decision",
        ),
    };

    if line["yq"] == false && python.exit() == 2 && matches!(rust.exit(), 0 | 1) {
        return yq_missing(tool, &python, &rust, file, python_prefix);
    }
    if python.exit() != rust.exit() {
        return Err(format!("exit {} and {}", python.exit(), rust.exit()));
    }
    for name in ["floor.json", "decision.json"] {
        if python.file(name) != rust.file(name) {
            return Err(format!("{name} differs"));
        }
    }
    if python.text("stdout") != rust.text("stdout") {
        return Err("stdout differs".to_owned());
    }
    let (python_err, rust_err) = (python.text("stderr"), rust.text("stderr"));
    if python.exit() != 2 {
        if python_err != rust_err {
            return Err(format!("stderr {python_err:?} and {rust_err:?}"));
        }
        return Ok(None);
    }
    let python_lines: Vec<&str> = python_err.lines().collect();
    let rust_lines: Vec<&str> = rust_err.lines().collect();
    let [python_line] = python_lines.as_slice() else {
        return Err(format!("Python's stderr is not one line: {python_err:?}"));
    };
    let [rust_line] = rust_lines.as_slice() else {
        return Err(format!("Rust's stderr is not one line: {rust_err:?}"));
    };
    if rust_err.contains("hint:") || rust_err.contains("Traceback") {
        return Err(format!("Rust's stderr: {rust_err:?}"));
    }
    let (Some(python_text), Some(rust_text)) = (
        python_line.strip_prefix(python_prefix),
        rust_line.strip_prefix(rust_prefix),
    ) else {
        return Err(format!("prefixes: {python_line:?} and {rust_line:?}"));
    };
    let refusal = [python_usage, "--base must", "--issue must", "--components"]
        .iter()
        .any(|word| python_text.contains(word));
    if refusal && python_text.replace(python_usage, rust_usage) != rust_text {
        return Err(format!(
            "argument refusals {python_text:?} and {rust_text:?}"
        ));
    }
    for word in ["fetch", "partial clone"] {
        if python_text.contains(word) && !rust_text.contains(word) {
            return Err(format!("{rust_text:?} does not name {word:?}"));
        }
    }
    Ok(None)
}

#[test]
fn every_selftest_call_equals_the_python() {
    let mut lines = record();
    let last = lines.pop().expect("a final line");
    let calls = last["calls"].as_u64().expect("the number of calls");
    assert_eq!(last["failed"], 0, "the self-test failed under the recorder");
    assert_eq!(lines.len() as u64, calls);
    for tool in ["floor", "decision"] {
        assert!(
            lines.iter().any(|line| line["tool"] == tool),
            "no {tool} call is recorded"
        );
    }
    let mut seen = Vec::new();
    let mut failures = Vec::new();
    for line in &lines {
        match compare(line) {
            Ok(Some(difference)) => seen.push(difference),
            Ok(None) => {}
            Err(why) => failures.push(format!(
                "#{} {} {}: {why}; argv {}",
                line["seq"], line["check"], line["tool"], line["argv"]
            )),
        }
    }
    assert!(failures.is_empty(), "differ from the Python: {failures:#?}");
    assert!(
        seen.contains(&Difference::YqMissing),
        "{:?} is accepted but never seen",
        Difference::YqMissing
    );
}

/// The `git` on `PATH`.
fn real_git() -> PathBuf {
    env::split_paths(&env::var_os("PATH").unwrap_or_default())
        .map(|dir| dir.join("git"))
        .find(|path| path.is_file())
        .expect("git is on PATH")
}

/// A job repository with the repository's config committed, a plan and the documents, and a
/// TMPDIR of its own.
struct Sandbox {
    dir: TempDir,
    repo: PathBuf,
    tmp: PathBuf,
    base: String,
}

impl Sandbox {
    fn new() -> Sandbox {
        let dir = TempDir::new().expect("a temporary directory");
        let repo = dir.path().join("repo");
        let tmp = dir.path().join("tmpdir");
        fs::create_dir_all(repo.join(".mintswarm")).expect("the repository");
        fs::create_dir(&tmp).expect("a TMPDIR");
        let sandbox = Sandbox {
            dir,
            repo,
            tmp,
            base: String::new(),
        };
        fs::copy(
            repository().join(".mintswarm/config.yaml"),
            sandbox.repo.join(".mintswarm/config.yaml"),
        )
        .expect("the config");
        sandbox.git(&["init", "-q", "-b", "master"]);
        sandbox.git(&["add", ".mintswarm/config.yaml"]);
        sandbox.git(&["commit", "-q", "-m", "chore(agents): config"]);
        let base = sandbox.git(&["rev-parse", "HEAD"]);
        for (name, text) in [
            ("plan.md", "# Plan: a case\n\nNo footprint.\n"),
            ("issue.md", "# An issue\n"),
            ("analysis.md", "# Analysis: a case\n"),
            ("assessment.md", "# Assessment\n"),
        ] {
            fs::write(sandbox.repo.join(name), text).expect("a document");
        }
        Sandbox { base, ..sandbox }
    }

    fn git(&self, args: &[&str]) -> String {
        let output = Command::new(real_git())
            .current_dir(&self.repo)
            .env_clear()
            .envs([
                ("GIT_CONFIG_NOSYSTEM", "1"),
                ("GIT_CONFIG_GLOBAL", "/dev/null"),
                ("GIT_AUTHOR_NAME", "Ada"),
                ("GIT_AUTHOR_EMAIL", "ada@example.invalid"),
                ("GIT_COMMITTER_NAME", "Ada"),
                ("GIT_COMMITTER_EMAIL", "ada@example.invalid"),
            ])
            .args(args)
            .output()
            .expect("git runs");
        assert!(output.status.success(), "git {args:?} failed");
        String::from_utf8(output.stdout)
            .expect("UTF-8")
            .trim_end()
            .to_owned()
    }

    fn floor_args(&self) -> Vec<String> {
        [
            "--plan",
            "plan.md",
            "--components",
            "runner",
            "--base",
            &self.base,
            "--issue-document",
            "issue.md",
            "--analysis",
            "analysis.md",
        ]
        .map(str::to_owned)
        .to_vec()
    }

    fn decision_args(&self) -> Vec<String> {
        [
            "--issue",
            "186",
            "--components",
            "runner",
            "--plan",
            "plan.md",
            "--issue-document",
            "issue.md",
            "--analysis",
            "analysis.md",
            "--assessment",
            "assessment.md",
            "--assessment-artifact",
            "https://example.invalid/a",
            "--base",
            &self.base,
            "--harness",
            "claude",
            "--model",
            "sonnet",
        ]
        .map(str::to_owned)
        .to_vec()
    }

    /// `BIN tool <tool> args` in the repository, with `PATH` as given and the sandbox's TMPDIR.
    fn run(&self, tool: &str, args: &[String], path: &std::ffi::OsStr) -> Output {
        Command::new(BIN)
            .current_dir(&self.repo)
            .env_clear()
            .env("PATH", path)
            .env("HOME", self.dir.path())
            .env("TMPDIR", &self.tmp)
            .arg("tool")
            .arg(tool)
            .args(args)
            .output()
            .expect("the tool runs")
    }

    fn outputs(&self) -> PathBuf {
        self.repo.join(".mintswarm/job/outputs")
    }

    fn tmp_entries(&self) -> Vec<String> {
        fs::read_dir(&self.tmp)
            .expect("a readable TMPDIR")
            .map(|entry| {
                entry
                    .expect("an entry")
                    .file_name()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect()
    }
}

fn script(path: &Path, body: &str) {
    fs::write(path, format!("#!/bin/sh\n{body}\n")).expect("a script");
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).expect("an executable");
}

/// Stands in for the self-test's `unusable yq missing` and `decision unusable yq missing`: with
/// only `git` and stubs of `yq`, `python3` and `python` on `PATH`, both tools give a verdict and
/// no stub runs.
#[test]
fn runs_no_yq_and_no_python() {
    let sandbox = Sandbox::new();
    let bin = sandbox.dir.path().join("bin");
    fs::create_dir(&bin).expect("a bin directory");
    std::os::unix::fs::symlink(real_git(), bin.join("git")).expect("a git link");
    let marker = sandbox.dir.path().join("ran");
    for stub in ["yq", "python3", "python"] {
        script(
            &bin.join(stub),
            &format!("echo {stub} >> '{}'\nexit 1", marker.display()),
        );
    }
    let floor = sandbox.run("plan-floor", &sandbox.floor_args(), bin.as_os_str());
    assert!(
        matches!(floor.status.code(), Some(0 | 1)),
        "{}",
        String::from_utf8_lossy(&floor.stderr)
    );
    assert!(sandbox.outputs().join("floor.json").is_file());
    let decision = sandbox.run("plan-decision", &sandbox.decision_args(), bin.as_os_str());
    assert!(
        matches!(decision.status.code(), Some(0 | 1)),
        "{}",
        String::from_utf8_lossy(&decision.stderr)
    );
    assert!(sandbox.outputs().join("decision.json").is_file());
    assert!(
        !marker.exists(),
        "a stub ran: {:?}",
        fs::read_to_string(&marker)
    );
}

/// The git subcommand and its arguments in a line of the logging wrapper, `GIT_DIR` and the
/// global options before it left out.
fn git_command(line: &str) -> Vec<&str> {
    let words: Vec<&str> = line.split(' ').collect();
    let mut i = 1;
    while i < words.len() {
        match words[i] {
            "-c" => i += 2,
            word if word.starts_with("--") => i += 1,
            _ => break,
        }
    }
    words[i.min(words.len())..].to_vec()
}

/// Stands in for the self-test's `object store: the store is made in TMPDIR, and the base,
/// config and history are read through it` and its `no store is left` checks: git is run with
/// `GIT_DIR` at a store in a `msw-floor-objects-` directory of TMPDIR, which is empty again after
/// a clean run and after an exit 2 run. Every content read and fetch runs in the store; on the job
/// repository git runs only `rev-parse`, `config --list -z` and `hash-object` without `-w`.
#[test]
fn the_store_is_fresh_in_tmpdir_and_removed() {
    let sandbox = Sandbox::new();
    let bin = sandbox.dir.path().join("logging");
    fs::create_dir(&bin).expect("a bin directory");
    let log = sandbox.dir.path().join("git.log");
    script(
        &bin.join("git"),
        &format!(
            "printf '%s %s\\n' \"$GIT_DIR\" \"$*\" >> '{}'\nexec '{}' \"$@\"",
            log.display(),
            real_git().display()
        ),
    );
    let path = env::join_paths(
        [bin.clone()]
            .into_iter()
            .chain(env::split_paths(&env::var_os("PATH").unwrap_or_default())),
    )
    .expect("a PATH");
    let floor = sandbox.run("plan-floor", &sandbox.floor_args(), &path);
    assert!(
        matches!(floor.status.code(), Some(0 | 1)),
        "{}",
        String::from_utf8_lossy(&floor.stderr)
    );
    let store = format!("{}/msw-floor-objects-", sandbox.tmp.display());
    let calls = fs::read_to_string(&log).expect("git ran through the wrapper");
    for line in calls.lines() {
        let (in_store, command) = (line.starts_with(&store), git_command(line));
        match command.first().copied() {
            Some("ls-tree" | "cat-file" | "log" | "diff" | "show" | "rev-list" | "fetch") => {
                assert!(in_store, "a content read outside the store: {line}");
            }
            _ if in_store => {}
            Some("rev-parse") => {}
            Some("config") => assert_eq!(command, ["config", "--list", "-z"], "{line}"),
            Some("hash-object") => assert!(!command.contains(&"-w"), "a write: {line}"),
            _ => panic!("a git call on the job repository outside the allowlist: {line}"),
        }
    }
    assert!(
        calls
            .lines()
            .any(|line| line.starts_with(&store) && line.contains(" fetch ")),
        "{calls}"
    );
    assert!(
        calls
            .lines()
            .any(|line| line.starts_with(&store) && line.contains(" rev-list ")),
        "{calls}"
    );
    assert_eq!(sandbox.tmp_entries(), Vec::<String>::new());

    let mut missing_plan = sandbox.floor_args();
    missing_plan[1] = "missing.md".to_owned();
    let refused = sandbox.run("plan-floor", &missing_plan, &path);
    assert_eq!(refused.status.code(), Some(2));
    assert_eq!(sandbox.tmp_entries(), Vec::<String>::new());
}

/// Stands in for the self-test's `read-only outputs, no stale verdict: exit 2 from the
/// catch-all`: with the outputs read-only and no stale verdict, the floor exits 2 with one line
/// naming the verdict, and leaves no `.floor.*` file.
#[test]
fn a_failed_write_is_one_line_exit_2() {
    let sandbox = Sandbox::new();
    let outputs = sandbox.outputs();
    fs::create_dir_all(&outputs).expect("the outputs");
    fs::set_permissions(&outputs, fs::Permissions::from_mode(0o555)).expect("read-only outputs");
    if fs::write(outputs.join("probe"), "").is_ok() {
        // running as root: read-only directories do not stop a write
        fs::remove_file(outputs.join("probe")).expect("the probe is removed");
        fs::set_permissions(&outputs, fs::Permissions::from_mode(0o755)).expect("writable");
        return;
    }
    let path = env::var_os("PATH").unwrap_or_default();
    let floor = sandbox.run("plan-floor", &sandbox.floor_args(), &path);
    let names: Vec<String> = fs::read_dir(&outputs)
        .expect("readable outputs")
        .map(|entry| {
            entry
                .expect("an entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    fs::set_permissions(&outputs, fs::Permissions::from_mode(0o755)).expect("writable");
    assert_eq!(floor.status.code(), Some(2));
    assert_eq!(
        String::from_utf8_lossy(&floor.stderr),
        "plan-floor: cannot write the verdict .mintswarm/job/outputs/floor.json\n"
    );
    assert!(floor.stdout.is_empty());
    assert_eq!(names, Vec::<String>::new());
}
