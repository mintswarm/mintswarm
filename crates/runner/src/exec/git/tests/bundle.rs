//! The raw subjects read and the plan floor's input bundle, against `plan-approval/floor.py`:
//! `crates/runner/tests/fixtures/plan_floor_bundle_capture.py` builds a fixture repository, runs
//! `floor.py` on it for each case and records the `floor.json` text it wrote, in
//! `plan_floor_bundle.jsonl`. Here the same repository is rebuilt, fetched into a store, and the
//! floor of the extracted bundle must equal that text byte for byte, or both must be unusable.

use std::path::{Path, PathBuf};
use std::process::Command;

use mintswarm_domain::ids::{RepoKey, RevId};
use mintswarm_domain::records::{BaseRef, Json};
use mintswarm_domain::vcs::{RefOrId, Remote, Vcs};
use mintswarm_rules::exit;
use mintswarm_rules::plan_floor;
use tempfile::TempDir;

use super::super::{BundleError, Documents, GitError, GitStore, plan_floor_bundle, read};
use super::{Fixture, commit_with, fixture_command, fixture_git, git_probe, run_fixture, text};

const REFRESH: &str = "PYTHONDONTWRITEBYTECODE=1 python3 \
                       crates/runner/tests/fixtures/plan_floor_bundle_capture.py \
                       crates/runner/tests/fixtures/plan_floor_bundle.jsonl";

const LOG: &[&str] = &["-c", "log.showSignature=false", "log", "-z", "--format=%s"];

#[test]
fn subjects_are_the_raw_log_output() {
    let fx = Fixture::new();
    let tree = text(&fixture_git(
        &fx.source,
        &["rev-parse", &format!("{}^{{tree}}", fx.main.0)],
    ));
    let latin = commit_with(
        &fx.source,
        &tree,
        &[&fx.main.0],
        b"caf\xe9 au lait\n\nbody\n",
        Some("ISO-8859-1"),
        1_700_000_600,
    );
    fixture_git(&fx.source, &["update-ref", "refs/heads/latin", &latin.0]);
    let header = fixture_git(&fx.source, &["cat-file", "commit", &latin.0]);
    assert!(
        header
            .windows(b"\nencoding ISO-8859-1\n".len())
            .any(|window| window == b"\nencoding ISO-8859-1\n")
    );
    let store = fx.fetched_store("subjects");
    store
        .fetch(
            &fx.remote(),
            &[RefOrId::Ref("refs/heads/latin".to_owned())],
            None,
        )
        .unwrap();
    let got = read::subjects(&store, &latin).unwrap();
    let mut args = LOG.to_vec();
    args.extend([latin.0.as_str(), "--"]);
    let expected = fixture_git(&fx.source, &args);
    assert_eq!(got, expected);
    assert!(got.starts_with(b"caf"));
    assert_eq!(got.iter().filter(|&&byte| byte == 0).count(), 3);

    let tree = RevId(tree);
    assert!(matches!(
        read::subjects(&store, &tree),
        Err(GitError::NotFound(_))
    ));
    assert!(matches!(
        read::subjects(&store, &RevId("HEAD".to_owned())),
        Err(GitError::RevisionSyntax(_))
    ));
}

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

/// The value of `key` in a JSON object.
fn get<'a>(value: &'a Json, key: &str) -> &'a Json {
    let Json::Object(entries) = value else {
        panic!("{key}: not an object");
    };
    entries
        .iter()
        .find(|(name, _)| name == key)
        .map(|(_, value)| value)
        .unwrap_or_else(|| panic!("no {key}"))
}

fn string(value: &Json) -> &str {
    match value {
        Json::String(text) => text,
        other => panic!("not a string: {other:?}"),
    }
}

fn optional(value: &Json) -> Option<String> {
    match value {
        Json::Null => None,
        other => Some(string(other).to_owned()),
    }
}

fn array(value: &Json) -> &[Json] {
    match value {
        Json::Array(items) => items,
        other => panic!("not an array: {other:?}"),
    }
}

fn number(value: &Json) -> u64 {
    match value {
        Json::Int(n) => u64::try_from(*n).unwrap(),
        Json::UInt(n) => *n,
        other => panic!("not a number: {other:?}"),
    }
}

fn bytes(value: &Json) -> Vec<u8> {
    let Json::Object(entries) = value else {
        panic!("bytes are an object");
    };
    match entries.first() {
        Some((kind, Json::String(text))) if kind == "utf8" => text.as_bytes().to_vec(),
        Some((kind, Json::String(hex))) if kind == "hex" => (0..hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
            .collect(),
        other => panic!("bytes: {other:?}"),
    }
}

/// The fixture's lines: the recipe, then the cases.
fn fixture() -> (Json, Vec<Json>) {
    let text = std::fs::read_to_string(fixtures().join("plan_floor_bundle.jsonl")).unwrap();
    let mut lines = text
        .lines()
        .map(|line| mintswarm_rules::json::read(line).unwrap());
    let recipe = lines.next().unwrap();
    assert_eq!(string(get(&recipe, "kind")), "recipe");
    let cases: Vec<Json> = lines.collect();
    assert!(cases.iter().all(|case| string(get(case, "kind")) == "case"));
    (recipe, cases)
}

/// The recipe's repository rebuilt with the same plumbing and fetched into a store.
struct Replayed {
    dir: TempDir,
    store: GitStore,
    /// The commits, in the recipe's order.
    ids: Vec<RevId>,
}

fn replay(recipe: &Json) -> Replayed {
    let dir = TempDir::new().unwrap();
    let source = dir.path().join("source.git");
    run_fixture(
        fixture_command(None),
        &[
            "init".as_ref(),
            "--bare".as_ref(),
            "--quiet".as_ref(),
            source.as_os_str(),
        ],
        b"",
    );
    let mut ids: Vec<RevId> = Vec::new();
    for (n, commit) in array(get(recipe, "commits")).iter().enumerate() {
        let index = dir.path().join(format!("index-{n}"));
        let indexed = || {
            let mut command = fixture_command(Some(&source));
            command.env("GIT_INDEX_FILE", &index);
            command
        };
        for file in array(get(commit, "files")) {
            let id = text(&run_fixture(
                fixture_command(Some(&source)),
                &["hash-object", "-w", "--stdin"],
                &bytes(get(file, "bytes")),
            ));
            let info = format!(
                "{},{id},{}",
                string(get(file, "mode")),
                string(get(file, "path"))
            );
            run_fixture(
                indexed(),
                &["update-index", "--add", "--cacheinfo", &info],
                b"",
            );
        }
        let tree = text(&run_fixture(indexed(), &["write-tree"], b""));
        let parents: Vec<String> = array(get(commit, "parents"))
            .iter()
            .map(|parent| ids[usize::try_from(number(parent)).unwrap()].0.clone())
            .collect();
        let parents: Vec<&str> = parents.iter().map(String::as_str).collect();
        let encoding = optional(get(commit, "encoding"));
        let id = commit_with(
            &source,
            &tree,
            &parents,
            &bytes(get(commit, "message")),
            encoding.as_deref(),
            number(get(commit, "time")),
        );
        assert_eq!(
            id.0,
            string(get(commit, "id")),
            "commit {n} of the recipe has another id"
        );
        ids.push(id);
    }
    let branch = format!("refs/heads/{}", string(get(recipe, "branch")));
    fixture_git(&source, &["update-ref", &branch, &ids[ids.len() - 1].0]);
    let path =
        super::super::store_path(&dir.path().join("state"), &RepoKey::new("bundle").unwrap());
    let mut store = GitStore::create(&path, &git_probe(Some("2.45.0"))).unwrap();
    store.allow_file_remote();
    let remote = Remote {
        name: "origin".to_owned(),
        url: format!("file://{}", source.display()),
    };
    store.fetch(&remote, &[RefOrId::Ref(branch)], None).unwrap();
    Replayed { dir, store, ids }
}

/// A case's documents root, with its files and directories written.
fn documents_root(parent: &Path, name: &str, case: &Json) -> PathBuf {
    let root = parent.join(name.replace(' ', "-"));
    std::fs::create_dir(&root).unwrap();
    for file in array(get(case, "files")) {
        let path = root.join(string(get(file, "path")));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes(get(file, "bytes"))).unwrap();
    }
    for dir in array(get(case, "directories")) {
        std::fs::create_dir_all(root.join(string(dir))).unwrap();
    }
    root
}

fn documents(case: &Json) -> Documents {
    let documents = get(case, "documents");
    Documents {
        plan: string(get(documents, "plan")).to_owned(),
        issue_document: optional(get(documents, "issue_document")),
        analysis: optional(get(documents, "analysis")),
        screen: optional(get(documents, "screen")),
    }
}

fn base(case: &Json) -> BaseRef {
    let base = get(case, "base");
    BaseRef {
        reference: string(get(base, "ref")).to_owned(),
        commit: RevId(string(get(base, "commit")).to_owned()),
    }
}

#[test]
fn plan_floor_from_the_bundle_equals_floor_py() {
    let (recipe, cases) = fixture();
    let replayed = replay(&recipe);
    assert_eq!(cases.len(), 8);
    let mut exits = Vec::new();
    for case in &cases {
        let name = string(get(case, "name"));
        let root = documents_root(replayed.dir.path(), name, case);
        let expected = number(get(case, "exit"));
        exits.push(expected);
        let Json::Array(names) = get(case, "components") else {
            panic!("{name}: components is not a list");
        };
        let components: Vec<String> = names.iter().map(|n| string(n).to_owned()).collect();
        let bundle = plan_floor_bundle(
            &replayed.store,
            base(case),
            &components,
            &root,
            &documents(case),
        );
        let floor = bundle
            .map_err(|error| error.unusable())
            .and_then(|bundle| plan_floor::floor(&bundle));
        match (floor, get(case, "floor")) {
            (Ok(floor), Json::String(captured)) => {
                assert_eq!(floor.text(), *captured, "{name}");
                assert_eq!(u64::from(exit::code(&floor.outcome())), expected, "{name}");
            }
            (Err(unusable), Json::Null) => {
                assert_eq!(expected, 2, "{name}: {unusable}");
                assert_eq!(exit::code(&Err(unusable)), 2, "{name}");
            }
            (got, captured) => panic!("{name}: {got:?} against {captured:?}"),
        }
    }
    // The fixture covers clean, hit and unusable runs.
    for class in [0, 1, 2] {
        assert!(exits.contains(&class), "no case exits {class}");
    }
}

#[test]
fn extraction_failures_are_unusable() {
    let (recipe, cases) = fixture();
    let replayed = replay(&recipe);
    let clean = cases
        .iter()
        .find(|case| string(get(case, "name")) == "clean")
        .unwrap();
    let head = replayed.ids[replayed.ids.len() - 1].clone();
    let root = documents_root(replayed.dir.path(), "extraction", clean);
    let full = documents(clean);
    let at = |commit: &RevId| BaseRef {
        reference: "master".to_owned(),
        commit: commit.clone(),
    };
    let widgets = ["widgets".to_owned()];
    let extract = |base: BaseRef, documents: &Documents| {
        plan_floor_bundle(&replayed.store, base, &widgets, &root, documents)
    };

    let bundle = extract(at(&head), &full).unwrap();
    assert_eq!(bundle.base, at(&head));
    assert!(bundle.analysis_record.is_some() && bundle.plan_record.is_some());
    assert_eq!(
        bundle.subjects.bytes,
        read::subjects(&replayed.store, &head).unwrap()
    );

    let unknown = RevId("0123456789abcdef0123456789abcdef01234567".to_owned());
    let tree = RevId(text(&fixture_git(
        replayed.store.path(),
        &["rev-parse", &format!("{}^{{tree}}", head.0)],
    )));
    let mut failures: Vec<(&str, Result<_, BundleError>)> = vec![
        ("unknown base", extract(at(&unknown), &full)),
        ("tree base", extract(at(&tree), &full)),
        ("base without config", extract(at(&replayed.ids[0]), &full)),
    ];
    let mut missing = full.clone();
    missing.plan = ".mintswarm/job/outputs/missing.md".to_owned();
    failures.push(("missing plan", extract(at(&head), &missing)));
    let mut directory = full.clone();
    directory.analysis = Some(".mintswarm/job/inputs".to_owned());
    failures.push(("analysis a directory", extract(at(&head), &directory)));
    std::fs::remove_file(root.join(".mintswarm/job/outputs/plan.result.json")).unwrap();
    std::fs::create_dir(root.join(".mintswarm/job/outputs/plan.result.json")).unwrap();
    failures.push(("record a directory", extract(at(&head), &full)));

    let classes: Vec<&str> = failures
        .iter()
        .map(|(name, result)| match result {
            Err(error) => {
                assert_eq!(exit::code(&Err(error.unusable())), 2, "{name}");
                match error {
                    BundleError::Base { .. } => "base",
                    BundleError::NoConfig(_) => "no config",
                    BundleError::Document { .. } => "document",
                    BundleError::Store { .. } => "store",
                }
            }
            Ok(_) => panic!("{name}: a bundle"),
        })
        .collect();
    assert_eq!(
        classes,
        [
            "base",
            "base",
            "no config",
            "document",
            "document",
            "document"
        ]
    );

    // Without the analysis, its record is not read; an absent plan record is no record.
    std::fs::remove_dir(root.join(".mintswarm/job/outputs/plan.result.json")).unwrap();
    let mut alone = full;
    alone.analysis = None;
    let bundle = extract(at(&head), &alone).unwrap();
    assert!(bundle.analysis.is_none() && bundle.analysis_record.is_none());
    assert!(bundle.plan_record.is_none());
}

#[test]
fn plan_floor_bundle_fixture_is_fresh() {
    let dir = TempDir::new().unwrap();
    let out = dir.path().join("plan_floor_bundle.jsonl");
    let output = Command::new("python3")
        .arg("-I")
        .arg("-B")
        .arg(fixtures().join("plan_floor_bundle_capture.py"))
        .arg(&out)
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .output()
        .expect("python3 runs plan_floor_bundle_capture.py");
    assert!(
        output.status.success(),
        "plan_floor_bundle_capture.py failed: {}\n{}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    let captured = std::fs::read(&out).expect("plan_floor_bundle_capture.py wrote its output");
    let committed =
        std::fs::read(fixtures().join("plan_floor_bundle.jsonl")).expect("the committed fixture");
    assert!(
        captured == committed,
        "tests/fixtures/plan_floor_bundle.jsonl is stale against floor.py; refresh it with:\n    \
         {REFRESH}"
    );
}
