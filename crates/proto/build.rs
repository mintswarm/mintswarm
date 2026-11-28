//! Compiles every `.proto` under `api/proto/` with `protox` and generates
//! the Rust code with `tonic-prost-build` into `OUT_DIR`.
//!
//! `mintswarm.common.v1.Secret` is generated without a derived `Debug`:
//! `src/lib.rs` implements one that prints `Secret(<redacted>)`, so no secret
//! is ever logged (the runner protocol design on #156, §9.5).
//!
//! Every message implements `prost::Name` with its full proto name, and the
//! full descriptor set `protox` returns is written to
//! `OUT_DIR/descriptors.bin`. `src/lib.rs` builds the proto3 JSON mapping on
//! the two, through `prost-reflect`. The set holds `prost_types` descriptors,
//! which carry no extension options, so `google.api.http` annotations are not
//! in it. The well-known types, such as `google.protobuf.Timestamp`, are
//! `prost-reflect`'s re-export of `prost-types`.
//!
//! The vendored `google.api` files under `api/proto/google/api/` stay in
//! `descriptors.bin`, so the descriptor pool resolves the imports of the
//! protos that use them, but they are dropped from the set given to code
//! generation: no Rust module is generated for `google.api`, since no
//! generated type refers to one; only method options do.
//!
//! `ProbeResult.Known.facts` is generated as a `BTreeMap`, the map type of
//! `domain::probe`.
//!
//! The transport helpers are not generated: tonic's `connect(dst)` constructor
//! on `RunnerServiceClient<Channel>` would clash with the client method of the
//! `Connect` RPC, which is also named `connect`. `build_transport(false)`
//! applies to every client, so every client, `WatchServiceClient` included,
//! is built with `new(channel)` from a `tonic::transport::Channel`.

use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};

use prost::Message as _;

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

    let mut fds = protox::compile(&files, [&root])?;
    let out_dir = PathBuf::from(std::env::var_os("OUT_DIR").ok_or("OUT_DIR is not set")?);
    fs::write(out_dir.join("descriptors.bin"), fds.encode_to_vec())?;
    // Only the HTTP annotations, which are method options, refer to
    // `google.api`; see the module doc.
    fds.file.retain(|file| file.package() != "google.api");

    let mut config = tonic_prost_build::Config::new();
    // `prost::Name` gives `src/lib.rs` each message's descriptor in the set.
    config.enable_type_names();
    tonic_prost_build::configure()
        // Without `compile_well_known_types`, prost maps `.google.protobuf`
        // to `::prost_types` itself and refuses a second mapping; with it,
        // the `extern_path` decides, and none of them is compiled here.
        .compile_well_known_types(true)
        .extern_path(".google.protobuf", "::prost_reflect::prost_types")
        // `RunnerServiceClient::connect(dst)` would clash with the `Connect`
        // RPC's `connect` method; see the module doc.
        .build_transport(false)
        .skip_debug([".mintswarm.common.v1.Secret"])
        // A `BTreeMap`, the map type of `domain::probe`, so the keys keep the
        // same order.
        .btree_map(".mintswarm.runner.v1.ProbeResult.Known.facts")
        // `Hello` is far larger than the other cases; it is sent once per
        // stream, so it stays unboxed.
        .enum_attribute(
            ".mintswarm.runner.v1.ConnectRequest.body",
            "#[allow(clippy::large_enum_variant)]",
        )
        // One message per send, so `WatchEvent` stays unboxed.
        .enum_attribute(
            ".mintswarm.watch.v1.SubscribeResponse.message",
            "#[allow(clippy::large_enum_variant)]",
        )
        .compile_fds_with_config(fds, config)?;
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
