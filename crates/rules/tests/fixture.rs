//! The committed `tests/fixtures/python.jsonl` is what `capture.py` writes now: the Python helpers
//! it records have not changed since it was captured.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::{env, fs, process};

const REFRESH: &str = "PYTHONDONTWRITEBYTECODE=1 python3 crates/rules/tests/fixtures/capture.py \
                       crates/rules/tests/fixtures/python.jsonl";

#[test]
fn the_python_fixture_is_fresh() {
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    // outside the repository, so the check leaves the source tree untouched
    let out: PathBuf =
        env::temp_dir().join(format!("mintswarm-rules-capture-{}.jsonl", process::id()));
    let status = Command::new("python3")
        .arg("-I")
        .arg("-B")
        .arg(fixtures.join("capture.py"))
        .arg(&out)
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .status()
        .expect("python3 runs capture.py");
    let captured = fs::read(&out);
    let _ = fs::remove_file(&out);
    assert!(status.success(), "capture.py failed: {status}");
    let captured = captured.expect("capture.py wrote its output");
    let committed = fs::read(fixtures.join("python.jsonl")).expect("the committed fixture");
    assert!(
        captured == committed,
        "tests/fixtures/python.jsonl is stale against the Python helpers; refresh it with:\n    {REFRESH}"
    );
}
