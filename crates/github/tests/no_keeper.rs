//! Nothing in this crate names the keeper App's configuration file: its callers pass the path or
//! the token of the App they act as, so no code path here reads the keeper's configuration.
// Not redundant: clippy's `allow-unwrap-in-tests` covers a `tests/` crate only under `cfg(test)`.
#![cfg(test)]

use std::path::{Path, PathBuf};

use mintswarm_github::AppConfig;

/// Built from two halves so that this file does not contain it.
const NEEDLE: &str = concat!("keeper", "-app.json");

fn files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            files(&path, out);
        } else {
            out.push(path);
        }
    }
}

#[test]
fn no_source_names_the_keeper_config() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut paths = vec![root.join("Cargo.toml")];
    files(&root.join("src"), &mut paths);
    files(&root.join("tests"), &mut paths);
    assert!(
        paths.iter().any(|p| p.ends_with("src/token.rs")),
        "{paths:?}"
    );
    assert!(
        paths.iter().any(|p| p.ends_with("tests/no_keeper.rs")),
        "{paths:?}"
    );
    let needle = NEEDLE.as_bytes();
    for path in &paths {
        let text = std::fs::read(path).unwrap();
        assert!(
            !text.windows(needle.len()).any(|window| window == needle),
            "{} names {NEEDLE}",
            path.display()
        );
    }
}

#[test]
fn the_default_config_is_the_runner_apps() {
    let path = AppConfig::default_path(Path::new("/home/runner"));
    assert!(
        path.ends_with(".config/mintswarm/bee-app.json"),
        "{}",
        path.display()
    );
}
