//! `mintswarm-runner tool pr-labels` in temporary git repositories: the changed paths come from
//! the commits between the merge base and head, the config from the base. Failures are checked
//! by exit code and an empty stdout only, never by wording.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use tempfile::TempDir;

const BIN: &str = env!("CARGO_BIN_EXE_mintswarm-runner");

/// The pipeline block every config needs, as in `crates/rules/tests/pr_labels.rs`.
const PIPELINE: &str = "pipeline:
  plan_approval:
    mode: always
    limits: { files: 10, lines: 500, new_files: 3 }
  landing:
    mode: always
    footprint: { lines_factor: 1.5 }
";

/// A config with the components `runner`, `cli` and `web`, the first under the name `runner`.
fn config(runner: &str) -> String {
    format!(
        "components:\n  \
           {runner}:\n    paths: [\"crates/runner/**\"]\n  \
           cli:\n    paths: [\"crates/cli/**\"]\n  \
           web:\n    paths: [\"web/**\"]\n\
         {PIPELINE}"
    )
}

/// A repository on `master` with the config and one file per component, and a directory for
/// the label files outside it.
struct Fixture {
    repo: TempDir,
    files: TempDir,
}

impl Fixture {
    fn new() -> Fixture {
        let fixture = Fixture {
            repo: TempDir::new().expect("a temporary repository"),
            files: TempDir::new().expect("a temporary directory for the label files"),
        };
        let status = Command::new("git")
            .args(["-c", "init.defaultBranch=master", "init", "-q"])
            .arg(fixture.repo.path())
            .env_remove("GIT_DIR")
            .status()
            .expect("git init runs");
        assert!(status.success(), "git init");
        fixture.write(".mintswarm/config.yaml", &config("runner"));
        fixture.write("crates/runner/a.rs", "runner\n");
        fixture.write("crates/cli/x.rs", "cli\n");
        fixture.write("web/a.ts", "web\n");
        fixture.git(&["add", "."]);
        fixture.commit("base");
        fixture
    }

    fn dir(&self) -> &Path {
        self.repo.path()
    }

    fn write(&self, path: &str, text: &str) {
        let path = self.dir().join(path);
        fs::create_dir_all(path.parent().expect("a parent")).expect("the parent is created");
        fs::write(path, text).expect("the file is written");
    }

    /// Runs git in the repository with a fixed identity and date; its trimmed stdout.
    fn git(&self, args: &[&str]) -> String {
        let output = Command::new("git")
            .args(args)
            .current_dir(self.dir())
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_AUTHOR_NAME", "Fixture")
            .env("GIT_AUTHOR_EMAIL", "fixture@example.invalid")
            .env("GIT_AUTHOR_DATE", "2026-01-01T00:00:00Z")
            .env("GIT_COMMITTER_NAME", "Fixture")
            .env("GIT_COMMITTER_EMAIL", "fixture@example.invalid")
            .env("GIT_COMMITTER_DATE", "2026-01-01T00:00:00Z")
            .output()
            .expect("git runs");
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout)
            .expect("git prints UTF-8")
            .trim()
            .to_owned()
    }

    /// Commits what is staged; the new commit's id.
    fn commit(&self, message: &str) -> String {
        self.git(&["commit", "-q", "--allow-empty", "-m", message]);
        self.git(&["rev-parse", "HEAD"])
    }

    /// Starts the branch `pr` at `master`; the fork point's id.
    fn branch(&self) -> String {
        self.git(&["checkout", "-q", "-b", "pr", "master"]);
        self.git(&["rev-parse", "HEAD"])
    }

    /// A label file holding `json`.
    fn labels(&self, name: &str, json: &str) -> PathBuf {
        let path = self.files.path().join(name);
        fs::write(&path, json).expect("the label file is written");
        path
    }

    /// The tool run in the repository with `--base base`, the given label files and `extra`.
    fn run_with(&self, base: &str, pr: &Path, repo: &Path, extra: &[&str]) -> Output {
        Command::new(BIN)
            .args(["tool", "pr-labels", "--base", base, "--pr-labels"])
            .arg(pr)
            .arg("--repo-labels")
            .arg(repo)
            .args(extra)
            .current_dir(self.dir())
            .env_remove("GIT_DIR")
            .output()
            .expect("mintswarm-runner runs")
    }

    /// The tool run with no labels on the PR or the repository.
    fn run(&self, base: &str, extra: &[&str]) -> Output {
        let pr = self.labels("pr.json", "[]");
        let repo = self.labels("repo.json", "[]");
        self.run_with(base, &pr, &repo, extra)
    }
}

/// The changes a successful run printed.
fn changes(output: &Output) -> serde_json::Value {
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("stdout is JSON")
}

/// The names under `key`: strings for `add` and `remove`, the `name` of objects for `create`.
fn names(changes: &serde_json::Value, key: &str) -> Vec<String> {
    changes[key]
        .as_array()
        .unwrap_or_else(|| panic!("{key} is an array"))
        .iter()
        .map(|entry| {
            entry
                .as_str()
                .or_else(|| entry["name"].as_str())
                .expect("a label name")
                .to_owned()
        })
        .collect()
}

fn added(output: &Output) -> Vec<String> {
    names(&changes(output), "add")
}

/// Exit 2 and nothing on stdout.
fn assert_unusable(output: &Output) {
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty(), "stdout is empty");
}

#[test]
fn modified_runner_file_adds_runner_label() {
    let fixture = Fixture::new();
    let base = fixture.branch();
    fixture.write("crates/runner/a.rs", "changed\n");
    fixture.git(&["add", "crates/runner/a.rs"]);
    fixture.commit("change the runner");
    assert_eq!(added(&fixture.run(&base, &[])), ["component:runner"]);
}

#[test]
fn rename_labels_both_sides() {
    let fixture = Fixture::new();
    let base = fixture.branch();
    fixture.git(&["mv", "crates/cli/x.rs", "crates/runner/x.rs"]);
    fixture.commit("move x");
    assert_eq!(
        added(&fixture.run(&base, &[])),
        ["component:cli", "component:runner"]
    );
}

#[test]
fn deleted_web_file_adds_web_label() {
    let fixture = Fixture::new();
    let base = fixture.branch();
    fixture.git(&["rm", "-q", "web/a.ts"]);
    fixture.commit("delete web");
    assert_eq!(added(&fixture.run(&base, &[])), ["component:web"]);
}

#[test]
fn worktree_changes_are_ignored() {
    let fixture = Fixture::new();
    let base = fixture.branch();
    fixture.write("crates/runner/a.rs", "changed\n");
    fixture.git(&["add", "crates/runner/a.rs"]);
    fixture.commit("change the runner");
    let before = fixture.run(&base, &[]);
    assert_eq!(added(&before), ["component:runner"]);

    fixture.write("web/a.ts", "uncommitted\n");
    fixture.write("crates/cli/x.rs", "staged\n");
    fixture.git(&["add", "crates/cli/x.rs"]);
    fixture.write("crates/cli/new.rs", "untracked\n");
    let after = fixture.run(&base, &[]);
    assert_eq!(after.status.code(), Some(0));
    assert_eq!(after.stdout, before.stdout);
}

#[test]
fn config_is_read_at_base() {
    let fixture = Fixture::new();
    let base = fixture.branch();
    fixture.write(".mintswarm/config.yaml", &config("daemon"));
    fixture.write("crates/runner/a.rs", "changed\n");
    fixture.git(&["add", ".mintswarm/config.yaml", "crates/runner/a.rs"]);
    fixture.commit("rename the runner component");
    fixture.write(".mintswarm/config.yaml", &config("other"));
    let added = added(&fixture.run(&base, &[]));
    assert!(added.contains(&"component:runner".to_owned()), "{added:?}");
    assert!(!added.contains(&"component:daemon".to_owned()), "{added:?}");
    assert!(!added.contains(&"component:other".to_owned()), "{added:?}");
}

#[test]
fn base_advance_is_not_counted() {
    let fixture = Fixture::new();
    fixture.branch();
    fixture.write("crates/runner/a.rs", "changed\n");
    fixture.git(&["add", "crates/runner/a.rs"]);
    fixture.commit("change the runner");
    fixture.git(&["checkout", "-q", "master"]);
    fixture.write("web/a.ts", "advanced\n");
    fixture.git(&["add", "web/a.ts"]);
    let base = fixture.commit("advance the base");
    fixture.git(&["checkout", "-q", "pr"]);
    assert_eq!(added(&fixture.run(&base, &[])), ["component:runner"]);
}

#[test]
fn no_band_leaves_verdict_labels() {
    let fixture = Fixture::new();
    let base = fixture.branch();
    fixture.write("crates/runner/a.rs", "changed\n");
    fixture.git(&["add", "crates/runner/a.rs"]);
    fixture.commit("change the runner");
    let pr = fixture.labels(
        "pr.json",
        r#"[{"name": "msw-verdict:safe", "color": "ededed"}]"#,
    );
    let repo = fixture.labels("repo.json", r#"[{"name": "msw-verdict:safe"}]"#);
    let changes = changes(&fixture.run_with(&base, &pr, &repo, &[]));
    for key in ["add", "remove", "create"] {
        let names = names(&changes, key);
        assert!(
            !names.iter().any(|name| name.starts_with("msw-verdict:")),
            "{key}: {names:?}"
        );
    }
}

#[test]
fn band_safe_adds_verdict_label() {
    let fixture = Fixture::new();
    let base = fixture.branch();
    let changes = changes(&fixture.run(&base, &["--band", "safe"]));
    assert_eq!(names(&changes, "add"), ["msw-verdict:safe"]);
    assert_eq!(names(&changes, "create"), ["msw-verdict:safe"]);
}

#[test]
fn band_outside_set_is_usage_error() {
    let fixture = Fixture::new();
    let base = fixture.branch();
    for band in ["bogus", "Safe", "likely_safe"] {
        assert_unusable(&fixture.run(&base, &["--band", band]));
    }
}

#[test]
fn malformed_labels_exit_2() {
    let fixture = Fixture::new();
    let base = fixture.branch();
    let good = fixture.labels("good.json", r#"[{"name": "x"}]"#);
    for (i, bad) in ["[", "{}", r#"[{"nom":"x"}]"#, r#"[{"name":3}]"#, "[1]"]
        .iter()
        .enumerate()
    {
        let bad = fixture.labels(&format!("bad{i}.json"), bad);
        assert_unusable(&fixture.run_with(&base, &bad, &good, &[]));
        assert_unusable(&fixture.run_with(&base, &good, &bad, &[]));
    }
}

#[test]
fn unknown_base_exits_2() {
    let fixture = Fixture::new();
    fixture.branch();
    for base in ["0123456789abcdef0123456789abcdef01234567", "master"] {
        assert_unusable(&fixture.run(base, &[]));
    }
}

#[test]
fn explicit_head_commit_is_used() {
    let fixture = Fixture::new();
    let base = fixture.branch();
    fixture.write("crates/runner/a.rs", "changed\n");
    fixture.git(&["add", "crates/runner/a.rs"]);
    let head = fixture.commit("change the runner");
    fixture.git(&["checkout", "-q", "master"]);
    // the worktree's HEAD is the base, so only the explicit head gives a change
    assert_eq!(added(&fixture.run(&base, &[])), Vec::<String>::new());
    assert_eq!(
        added(&fixture.run(&base, &["--head", &head])),
        ["component:runner"]
    );
    assert_eq!(
        added(&fixture.run(&base, &["--head", "HEAD"])),
        Vec::<String>::new()
    );
}

#[test]
fn unresolvable_head_exits_2() {
    let fixture = Fixture::new();
    let base = fixture.branch();
    let short = &base[..12];
    for head in ["0123456789abcdef0123456789abcdef01234567", "pr", short] {
        assert_unusable(&fixture.run(&base, &["--head", head]));
    }
}

#[test]
fn manifest_has_no_network_dependency() {
    let manifest = fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml"))
        .expect("the manifest is read");
    for name in ["mintswarm-github", "reqwest", "hyper", "ureq", "octocrab"] {
        assert!(!manifest.contains(name), "{name}");
    }
}
