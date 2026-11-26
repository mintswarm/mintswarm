//! Checks the migration tree under `migrations/` and generates `OUT_DIR/migrations.rs`: the
//! sorted list of each file's path relative to the tree and its bytes, by `include_bytes!`.
//!
//! The check is the library's own, from the same source file. A tree with errors fails the
//! build, naming each file, and a missing tree is an empty one.

use std::env;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

#[path = "src/migrations/tree.rs"]
mod tree;

fn main() -> ExitCode {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let root = manifest.join("migrations");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=src/migrations/tree.rs");
    // Cargo scans a watched folder recursively. With the tree absent, the crate's folder is
    // watched so creating the tree is seen, since a missing watched path reruns every build.
    let watched = if root.exists() { &root } else { manifest };
    println!("cargo:rerun-if-changed={}", watched.display());

    match generate(&root) {
        Ok(()) => ExitCode::SUCCESS,
        Err(errors) => {
            for error in errors {
                eprintln!("migrations/{error}");
            }
            ExitCode::FAILURE
        }
    }
}

/// Reads and checks the tree, and writes the embedded list.
fn generate(root: &Path) -> Result<(), Vec<String>> {
    let files = tree::read_tree(root).map_err(|error| vec![error.to_string()])?;
    let pairs: Vec<(&str, &[u8])> = files
        .iter()
        .map(|(path, bytes)| (path.as_str(), bytes.as_slice()))
        .collect();
    tree::validate(&pairs)
        .map_err(|errors| errors.iter().map(ToString::to_string).collect::<Vec<_>>())?;

    let mut list = String::from("&[\n");
    for (path, _) in &files {
        let absolute = root.join(path);
        let absolute = absolute
            .to_str()
            .ok_or_else(|| vec![format!("{path}: the absolute path is not UTF-8")])?;
        // Infallible: writing to a String cannot fail.
        let _ = writeln!(list, "    ({path:?}, include_bytes!({absolute:?})),");
    }
    list.push_str("]\n");

    let out = env::var_os("OUT_DIR")
        .map(PathBuf::from)
        .ok_or_else(|| vec!["OUT_DIR is not set".to_owned()])?;
    let target = out.join("migrations.rs");
    fs::write(&target, list).map_err(|_| vec![format!("{}: cannot be written", target.display())])
}
