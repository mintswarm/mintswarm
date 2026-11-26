//! Compiles every `.proto` under `api/proto/` with `protox` and generates
//! the Rust code with `tonic-prost-build` into `OUT_DIR`.
//!
//! `mintswarm.runner.v1.Secret` is generated without a derived `Debug`:
//! `src/lib.rs` implements one that prints `Secret(<redacted>)`, so no secret
//! is ever logged (design §9.5).
//!
//! `ProbeResult.Known.facts` is generated as a `BTreeMap`, the map type of
//! `domain::probe`.
//!
//! The transport helpers are not generated: tonic's `connect(dst)` constructor
//! on `RunnerServiceClient<Channel>` would clash with the client method of the
//! `Connect` RPC, which is also named `connect`. Clients are built with
//! `RunnerServiceClient::new(channel)` from a `tonic::transport::Channel`.

use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};

fn main() -> Result<(), Box<dyn Error>> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .ok_or("crates/proto has no repository root above it")?
        .join("api/proto");
    println!("cargo:rerun-if-changed={}", root.display());

    let mut files = Vec::new();
    collect_protos(&root, &mut files)?;
    files.sort();
    // The root line alone makes Cargo watch the whole tree; the per-file lines
    // also list in `-vv` output exactly which protos this build compiles.
    for file in &files {
        println!("cargo:rerun-if-changed={}", file.display());
    }

    let fds = protox::compile(&files, [&root])?;
    tonic_prost_build::configure()
        // `RunnerServiceClient::connect(dst)` would clash with the `Connect`
        // RPC's `connect` method; see the module doc.
        .build_transport(false)
        .skip_debug([".mintswarm.runner.v1.Secret"])
        // A `BTreeMap`, the map type of `domain::probe`, so the keys keep the
        // same order.
        .btree_map(".mintswarm.runner.v1.ProbeResult.Known.facts")
        // `Hello` is far larger than the other cases; it is sent once per
        // stream, so it stays unboxed.
        .enum_attribute(
            ".mintswarm.runner.v1.ConnectRequest.body",
            "#[allow(clippy::large_enum_variant)]",
        )
        .compile_fds(fds)?;
    Ok(())
}

/// Collects every `*.proto` file under `dir`, recursively.
fn collect_protos(dir: &Path, files: &mut Vec<PathBuf>) -> Result<(), Box<dyn Error>> {
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            collect_protos(&path, files)?;
        } else if path.extension().is_some_and(|ext| ext == "proto") {
            files.push(path);
        }
    }
    Ok(())
}
