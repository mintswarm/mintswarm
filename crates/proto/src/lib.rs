//! Rust code generated from the runner protocol under `api/proto/`.
//!
//! `build.rs` compiles every `.proto` there into `OUT_DIR`; nothing generated
//! is committed.

/// The `mintswarm.runner.v1` package.
pub mod runner {
    /// Version 1 of the runner protocol.
    ///
    /// The package has no definitions yet, so prost emits no file for it; the
    /// first change that adds messages includes the generated code here with
    /// `tonic::include_proto!("mintswarm.runner.v1")`.
    pub mod v1 {}
}

/// The protocol's conformance scenarios and, in later changes, fakes of its
/// services.
#[cfg(feature = "fakes")]
pub mod fakes {
    pub mod scenarios;
}
