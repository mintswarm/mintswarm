//! Compiles every `.proto` under `api/proto/` with `protox` and generates
//! the Rust code with `tonic-prost-build` into `OUT_DIR`.

use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};

fn main() -> Result<(), Box<dyn Error>> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../api/proto");
    println!("cargo:rerun-if-changed={}", root.display());

    let mut files = Vec::new();
    collect_protos(&root, &mut files)?;
    files.sort();
    for file in &files {
        println!("cargo:rerun-if-changed={}", file.display());
    }

    let fds = protox::compile(&files, [&root])?;
    tonic_prost_build::configure().compile_fds(fds)?;
    Ok(())
}

/// Collects every `*.proto` file under `dir`, recursively.
fn collect_protos(dir: &Path, files: &mut Vec<PathBuf>) -> Result<(), Box<dyn Error>> {
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            println!("cargo:rerun-if-changed={}", path.display());
            collect_protos(&path, files)?;
        } else if path.extension().is_some_and(|ext| ext == "proto") {
            files.push(path);
        }
    }
    Ok(())
}
