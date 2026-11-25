//! Rust code generated from the runner protocol under `api/proto/`.
//!
//! `build.rs` compiles every `.proto` there into `OUT_DIR`; nothing generated
//! is committed.

/// The `mintswarm.runner.v1` package.
pub mod runner {
    /// Version 1 of the runner protocol: `RunnerService`, its messages and
    /// `Secret`, with the generated tonic client and server.
    pub mod v1 {
        tonic::include_proto!("mintswarm.runner.v1");

        /// Prints `Secret(<redacted>)`, never the value (design §9.5).
        ///
        /// `build.rs` skips prost's derived `Debug` for `Secret`, so every
        /// message that contains one redacts it through its own derived
        /// `Debug`.
        impl std::fmt::Debug for Secret {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("Secret(<redacted>)")
            }
        }
    }
}

/// Test fakes of the protocol's services, filled by later changes.
#[cfg(feature = "fakes")]
pub mod fakes {}
