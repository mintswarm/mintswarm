//! `msw-vcs` against the dogfood shim, and the client end to end.
//!
//! Parity: `fixtures/msw-vcs-record.py` runs every case of `.mintswarm/dogfood/msw-vcs-selftest.py`
//! through the shim and through `mintswarm-runner tool msw-vcs`, live, and the two must give the
//! same stdout, stderr and exit code on each.
//!
//! End to end: the built `msw-vcs` client, copied into an attempt's tool directory, answers from
//! an unrelated working directory with only `PATH` in its environment.

use std::path::{Path, PathBuf};
use std::process::{self, Command};
use std::sync::Arc;
use std::{env, fs};

use mintswarm_domain::ids::{RepoKey, RevId};
use mintswarm_domain::probe::{ProbeResult, ProbeValue};
use mintswarm_domain::records::Json;
use mintswarm_rules::json;
use mintswarm_runner::exec::git::{GitStore, store_path};
use mintswarm_runner::exec::tools::{RequestRecord, RequestSink, ToolDir};

const BIN: &str = env!("CARGO_BIN_EXE_mintswarm-runner");
const CLIENT: &str = env!("CARGO_BIN_EXE_msw-vcs");

fn repository() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
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

/// Runs the recorder into a file outside the repository and returns its lines.
fn record() -> Vec<Json> {
    let root = repository();
    let out = env::temp_dir().join(format!("mintswarm-runner-msw-vcs-{}.jsonl", process::id()));
    let run = Command::new("python3")
        .arg("-I")
        .arg("-B")
        .arg(root.join("crates/runner/tests/fixtures/msw-vcs-record.py"))
        .arg(root.join(".mintswarm/dogfood/msw-vcs-selftest.py"))
        .arg(BIN)
        .arg(&out)
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .output()
        .expect("python3 runs msw-vcs-record.py");
    let recorded = fs::read_to_string(&out);
    let _ = fs::remove_file(&out);
    assert!(
        run.status.success(),
        "msw-vcs-record.py failed: {}\n{}",
        run.status,
        String::from_utf8_lossy(&run.stderr)
    );
    recorded
        .expect("msw-vcs-record.py wrote its output")
        .lines()
        .map(|line| json::read(line).expect("each recorded line is JSON"))
        .collect()
}

#[test]
fn every_selftest_case_equals_the_shim() {
    let mut lines = record();
    let total = match lines.pop().as_ref().map(|last| field(last, "cases")) {
        Some(Json::Int(total)) => usize::try_from(*total).expect("a count"),
        other => panic!("the last line counts the cases, not {other:?}"),
    };
    let differing: Vec<String> = lines
        .iter()
        .filter(|line| field(line, "shim") != field(line, "rust"))
        .map(|line| match field(line, "name") {
            Json::String(name) => name.clone(),
            other => format!("{other:?}"),
        })
        .collect();
    assert!(differing.is_empty(), "differ from the shim: {differing:#?}");
    assert_ne!(total, 0);
    assert_eq!(lines.len(), total);
}

/// Fixture git on the store, with no configuration from the system or the user.
fn git(store: &Path, args: &[&str], input: &str, date: &str) -> String {
    use std::io::Write;
    let mut child = Command::new("git")
        .env_clear()
        .env("PATH", env::var_os("PATH").unwrap_or_default())
        .envs([
            ("GIT_DIR", store.to_str().expect("a UTF-8 store path")),
            ("GIT_CONFIG_NOSYSTEM", "1"),
            ("GIT_CONFIG_GLOBAL", "/dev/null"),
            ("LC_ALL", "C"),
            ("GIT_AUTHOR_NAME", "Ada Author"),
            ("GIT_AUTHOR_EMAIL", "ada@example.invalid"),
            ("GIT_COMMITTER_NAME", "Cy Committer"),
            ("GIT_COMMITTER_EMAIL", "cy@example.invalid"),
            ("GIT_AUTHOR_DATE", date),
            ("GIT_COMMITTER_DATE", date),
        ])
        .args(args)
        .stdin(process::Stdio::piped())
        .stdout(process::Stdio::piped())
        .spawn()
        .expect("git runs");
    child
        .stdin
        .take()
        .expect("a stdin pipe")
        .write_all(input.as_bytes())
        .expect("git takes its input");
    let output = child.wait_with_output().expect("git ends");
    assert!(output.status.success(), "git {args:?} failed");
    String::from_utf8(output.stdout)
        .expect("UTF-8 output")
        .trim_end()
        .to_owned()
}

struct Discard;

impl RequestSink for Discard {
    fn record(&self, _request: &RequestRecord) {}
}

#[test]
fn the_client_answers_with_only_its_path() {
    let dir = tempfile::TempDir::new().expect("a temporary directory");
    let state = dir.path().join("state");
    let path = store_path(&state, &RepoKey::new("repo").expect("a repo key"));
    let probe = ProbeResult {
        namespace: "tool.git".parse().expect("a probe namespace"),
        value: ProbeValue::Unknown,
        observed_at: "2026-11-26T00:00:00Z".to_owned(),
    };
    let store = GitStore::create(&path, &probe).expect("a new store");

    let date = "2026-01-02T00:00:00Z";
    let tree = |name: &str, data: &str| {
        let blob = git(&path, &["hash-object", "-w", "--stdin"], data, date);
        git(
            &path,
            &["mktree"],
            &format!("100644 blob {blob}\t{name}\n"),
            date,
        )
    };
    let base_tree = tree("a.txt", "a\n");
    let base = git(&path, &["commit-tree", &base_tree, "-m", "base"], "", date);
    let head_tree = tree("b.txt", "b\n");
    let head = git(
        &path,
        &["commit-tree", &head_tree, "-p", &base, "-m", "job: add b"],
        "",
        "2026-01-03T00:00:00Z",
    );

    let attempt = format!("e2e{}", process::id());
    let tools = ToolDir::create(
        &attempt,
        Path::new(CLIENT),
        Arc::new(store),
        RevId(base.clone()),
        RevId(head.clone()),
        Arc::new(Discard),
    )
    .expect("a tool directory");
    let tool_dir = tools.path().to_owned();
    for private in [&state, &path] {
        assert!(!tool_dir.starts_with(private), "{}", tool_dir.display());
    }
    let mut names: Vec<String> = fs::read_dir(&tool_dir)
        .expect("a readable tool directory")
        .map(|entry| {
            entry
                .expect("an entry")
                .file_name()
                .into_string()
                .expect("a UTF-8 name")
        })
        .collect();
    names.sort();
    assert_eq!(names, ["msw-vcs", "s"]);

    let elsewhere = dir.path().join("elsewhere");
    fs::create_dir(&elsewhere).expect("a working directory");
    let output = Command::new("msw-vcs")
        .env_clear()
        .env("PATH", &tool_dir)
        .current_dir(&elsewhere)
        .arg("log")
        .output()
        .expect("msw-vcs runs from PATH");
    assert_eq!(String::from_utf8_lossy(&output.stderr), "");
    assert_eq!(output.status.code(), Some(0));
    let expected = format!(
        "commit {head}\nparent {base}\nauthor Ada Author <ada@example.invalid>\n\
         date 2026-01-03T00:00:00Z\nsubject job: add b\npath D a.txt\npath A b.txt\n"
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout), expected);

    tools.close().expect("the tool directory is removed");
    assert!(!tool_dir.exists());
}
