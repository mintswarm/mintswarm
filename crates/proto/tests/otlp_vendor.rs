//! The vendored OTLP protos under `api/proto/opentelemetry/`: the pinned tag, the exact file
//! set with each file's SHA-256, their exemption from the whitespace check, the fetcher's
//! offline refusals, and the boundary that keeps the generated OTLP types inside this crate.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use sha2::{Digest as _, Sha256};

/// The pinned opentelemetry-proto release.
const TAG: &str = "v1.10.0";

/// Every vendored file under `api/proto/opentelemetry/`, with its SHA-256, as upstream
/// release v1.10.0 has it.
const FILES: &[(&str, &str)] = &[
    (
        "LICENSE",
        "c71d239df91726fc519c6eb72d318ec65820627232b2f796219e87dcf35d0ab4",
    ),
    (
        "proto/collector/logs/v1/logs_service.proto",
        "0252687fc8f59d139ced709d383e83678e9bd4957bdbe76077242f1c32363fa5",
    ),
    (
        "proto/collector/metrics/v1/metrics_service.proto",
        "2bb3c6adee5f7609c8a32c5e4b1ec57057320015e298e9cf136699a42abc24d7",
    ),
    (
        "proto/collector/trace/v1/trace_service.proto",
        "03c8cc4e3e101087d884392d6eda32152ad5cd696e6344f50deaa59804a75c7a",
    ),
    (
        "proto/common/v1/common.proto",
        "833db934b8b08be193940aa412f26a9ed8c564dfa8ea3517dc583f5064142731",
    ),
    (
        "proto/logs/v1/logs.proto",
        "6b2e1eba0c01ae2da47927c63eabfaf3f151ee7ad846f2cdf3b2b71fb61004fe",
    ),
    (
        "proto/metrics/v1/metrics.proto",
        "d0a53dda45ad100edbf25675f5b339ce8bf689bdc51816f8cf19b0d51464e1a4",
    ),
    (
        "proto/resource/v1/resource.proto",
        "e0a7cdc0ffcfeffaa2606e8611839735ebffaa2d6acdf33e9356f2c48ae692d3",
    ),
    (
        "proto/trace/v1/trace.proto",
        "c3fb1385c90b8bc08a2a462e28b5d0c422c7b524a839f75f75e3cd9f64f36956",
    ),
];

/// The files at the top of `api/proto/opentelemetry/` that are the repository's own, not
/// vendored; every other file anywhere under it must be in [`FILES`].
const NOT_VENDORED: &[&str] = &[".gitattributes", "VENDOR", "vendor.py"];

/// The repository root.
fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("the repository root resolves")
}

/// The vendored tree.
fn vendored() -> PathBuf {
    root().join("api/proto/opentelemetry")
}

fn sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Every regular file under `dir`, recursively.
fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).expect("the tree reads") {
        let path = entry.expect("the tree reads").path();
        if path.is_dir() {
            walk(&path, out);
        } else {
            out.push(path);
        }
    }
}

/// The value of VENDOR's `name` line.
fn vendor_value(text: &str, name: &str) -> Option<String> {
    text.lines()
        .filter(|line| !line.starts_with('#'))
        .find_map(|line| {
            let (key, value) = line.split_once(' ').unwrap_or((line, ""));
            (key == name).then(|| value.trim().to_owned())
        })
}

#[test]
fn vendor_records_the_pinned_tag_and_a_hash() {
    let text = fs::read_to_string(vendored().join("VENDOR")).unwrap();
    assert_eq!(vendor_value(&text, "tag").as_deref(), Some(TAG));
    let digest = vendor_value(&text, "sha256").unwrap();
    assert_eq!(digest.len(), 64);
    assert!(
        digest
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    );
}

#[test]
fn vendored_files_are_exactly_the_pinned_set() {
    let dir = vendored();
    let mut files = Vec::new();
    walk(&dir, &mut files);
    let actual: BTreeMap<String, String> = files
        .iter()
        .filter(|path| {
            path.parent() != Some(dir.as_path())
                || !path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| NOT_VENDORED.contains(&name))
        })
        .map(|path| {
            let name = path
                .strip_prefix(&dir)
                .unwrap()
                .to_str()
                .unwrap()
                .to_owned();
            (name, sha256(&fs::read(path).unwrap()))
        })
        .collect();
    let expected: BTreeMap<String, String> = FILES
        .iter()
        .map(|(name, digest)| ((*name).to_owned(), (*digest).to_owned()))
        .collect();
    assert_eq!(actual, expected);
}

/// `(path, whitespace attribute)` for every tracked file under `api/`.
fn whitespace_attributes() -> Vec<(String, String)> {
    let root = root();
    let listed = Command::new("git")
        .args(["ls-files", "-z", "--", "api/"])
        .current_dir(&root)
        .output()
        .expect("git lists the attributes");
    assert!(listed.status.success());
    let mut check = Command::new("git")
        .args(["check-attr", "-z", "--stdin", "whitespace"])
        .current_dir(&root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("git lists the attributes");
    check
        .stdin
        .take()
        .expect("git lists the attributes")
        .write_all(&listed.stdout)
        .expect("git lists the attributes");
    let output = check.wait_with_output().expect("git lists the attributes");
    assert!(output.status.success());
    let text = String::from_utf8(output.stdout).expect("git lists the attributes");
    let fields: Vec<&str> = text.split('\0').collect();
    fields
        .chunks_exact(3)
        .map(|chunk| (chunk[0].to_owned(), chunk[2].to_owned()))
        .collect()
}

#[test]
fn only_the_vendored_files_skip_the_whitespace_check() {
    let attributes = whitespace_attributes();
    assert!(
        attributes
            .iter()
            .any(|(path, _)| path == "api/proto/buf.yaml")
    );
    let unset: BTreeSet<String> = attributes
        .into_iter()
        .filter(|(_, value)| value == "unset")
        .map(|(path, _)| path)
        .collect();
    let expected: BTreeSet<String> = FILES
        .iter()
        .map(|(name, _)| format!("api/proto/opentelemetry/{name}"))
        .collect();
    assert_eq!(unset, expected);
}

/// Runs a copy of `vendor.py` beside a VENDOR holding `vendor`, in a fresh directory, and
/// returns its exit code and the names left in the directory.
fn run_fetcher(name: &str, vendor: &str, args: &[&str]) -> (Option<i32>, Vec<String>) {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    if dir.exists() {
        fs::remove_dir_all(&dir).expect("the fetcher runs in its directory");
    }
    fs::create_dir_all(&dir).expect("the fetcher runs in its directory");
    fs::copy(vendored().join("vendor.py"), dir.join("vendor.py"))
        .expect("the fetcher runs in its directory");
    fs::write(dir.join("VENDOR"), vendor).expect("the fetcher runs in its directory");
    let status = Command::new("python3")
        .arg("vendor.py")
        .args(args)
        .current_dir(&dir)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("the fetcher runs in its directory");
    let mut names: Vec<String> = fs::read_dir(&dir)
        .expect("the fetcher runs in its directory")
        .map(|entry| {
            entry
                .expect("the fetcher runs in its directory")
                .file_name()
                .into_string()
                .expect("the fetcher runs in its directory")
        })
        .collect();
    names.sort();
    (status.code(), names)
}

#[test]
fn fetcher_refuses_to_fetch_without_a_recorded_hash() {
    let (code, names) = run_fetcher("otlp-vendor-no-hash", "tag v1.10.0\nsha256\n", &[]);
    assert_eq!(code, Some(1));
    assert_eq!(names, ["VENDOR", "vendor.py"]);
}

#[test]
fn fetcher_refuses_a_first_hash_when_one_is_recorded() {
    let vendor = format!("tag v1.10.0\nsha256 {}\n", "0".repeat(64));
    let (code, names) = run_fetcher("otlp-vendor-recorded", &vendor, &["--record-first-hash"]);
    assert_eq!(code, Some(1));
    assert_eq!(names, ["VENDOR", "vendor.py"]);
}

/// Whether `text` holds the word `opentelemetry`, bounded by characters other than ASCII
/// letters, digits and `_`, and not followed by `.io`.
fn names_otlp_word(text: &str) -> bool {
    const WORD: &str = "opentelemetry";
    let bytes = text.as_bytes();
    let is_word = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
    text.match_indices(WORD).any(|(at, _)| {
        let end = at + WORD.len();
        let before = at == 0 || !is_word(bytes[at - 1]);
        let after = end == bytes.len() || !is_word(bytes[end]);
        before && after && !text[end..].starts_with(".io")
    })
}

#[test]
fn word_scan_rules() {
    assert!(names_otlp_word(
        "use mintswarm_proto::{opentelemetry as o};"
    ));
    assert!(names_otlp_word("opentelemetry-proto = \"0.33\""));
    assert!(names_otlp_word("package = \"opentelemetry\""));
    assert!(!names_otlp_word("https://opentelemetry.io/schemas/1.40.0"));
    assert!(!names_otlp_word("my_opentelemetry_thing"));
    assert!(!names_otlp_word("OTLP"));
}

/// Every workspace manifest: the root's and each member's.
fn manifests() -> Vec<PathBuf> {
    let root = root();
    let mut out = vec![root.join("Cargo.toml"), root.join("xtask/Cargo.toml")];
    for entry in fs::read_dir(root.join("crates")).expect("the crates directory reads") {
        let manifest = entry
            .expect("the crates directory reads")
            .path()
            .join("Cargo.toml");
        if manifest.exists() {
            out.push(manifest);
        }
    }
    out
}

#[test]
fn no_other_crate_names_the_generated_otlp_types() {
    let root = root();
    let mut files = Vec::new();
    for entry in fs::read_dir(root.join("crates")).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() && path.file_name().is_some_and(|name| name != "proto") {
            walk(&path, &mut files);
        }
    }
    walk(&root.join("xtask"), &mut files);
    if root.join("tests").is_dir() {
        walk(&root.join("tests"), &mut files);
    }
    files.push(root.join("Cargo.toml"));
    let mut offenders: Vec<String> = files
        .iter()
        .filter(|path| {
            path.extension().is_some_and(|ext| ext == "rs")
                || path.file_name().is_some_and(|name| name == "Cargo.toml")
        })
        .filter(|path| names_otlp_word(&fs::read_to_string(path).unwrap()))
        .map(|path| path.display().to_string())
        .collect();
    offenders.sort();
    assert!(offenders.is_empty(), "{offenders:?}");
}

#[test]
fn no_workspace_manifest_adds_an_otlp_crate() {
    let offenders: Vec<String> = manifests()
        .iter()
        .filter(|path| names_otlp_word(&fs::read_to_string(path).unwrap()))
        .map(|path| path.display().to_string())
        .collect();
    assert!(offenders.is_empty(), "{offenders:?}");
}
