//! `cargo xtask`: repository tasks.
//!
//! - `schemas` writes the JSON Schemas generated from Rust types under `api/schemas/`;
//!   `schemas --check` fails when a committed schema differs from the generated one.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

mod schemas;

const USAGE: &str = "usage: cargo xtask schemas [--check]";

/// The workspace root: the directory above this crate.
fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("the xtask crate sits one directory below the workspace root")
        .to_path_buf()
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let root = workspace_root();
    let result = match args.as_slice() {
        ["schemas"] => schemas::write(&root),
        ["schemas", "--check"] => schemas::check(&root),
        _ => {
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}
