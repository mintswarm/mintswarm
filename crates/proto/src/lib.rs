//! Rust code generated from the protos under `api/proto/`: the runner protocol,
//! the job vocabulary and the watch protocol.
//!
//! `build.rs` compiles every `.proto` there into `OUT_DIR`; nothing generated
//! is committed.

/// The `mintswarm.runner.v1` package.
pub mod runner {
    /// Version 1 of the runner protocol: `RunnerService`, its messages and
    /// `Secret` (`runner.proto` and `common.proto`), with the generated tonic
    /// client and server; the `Connect` stream's envelopes, `ConnectRequest`
    /// and `ConnectResponse`, and the messages they carry (`connect.proto`);
    /// `StepKind` (`steps.proto`); and the runner events, `RunnerEvent` and
    /// `EventBatch` (`events.proto`).
    ///
    /// The client has no `connect(dst)` constructor, since it would clash
    /// with the `Connect` RPC's method: build one with
    /// `RunnerServiceClient::new(channel)`.
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

/// The `mintswarm.job.v1` package.
pub mod job {
    /// Version 1 of the job vocabulary: `JobKind`, `JobState`, `Event`,
    /// `EscalationCause` and `AssignReason` (`job.proto`).
    ///
    /// `Event` is a generic name; use it qualified, as `job::v1::Event`.
    pub mod v1 {
        tonic::include_proto!("mintswarm.job.v1");
    }
}

/// The `mintswarm.watch.v1` package.
pub mod watch {
    /// Version 1 of the watch protocol: `WatchService`, with the generated
    /// tonic client and server, the `WatchEvent` envelope, the `WatchFilter`
    /// and the stream's marks (`watch.proto`); and the `github.webhook` body,
    /// `GitHubWebhook`, with `UntrustedText` (`github.proto`).
    ///
    /// Like every client in this crate, the client has no `connect(dst)`
    /// constructor: build one with `WatchServiceClient::new(channel)`.
    pub mod v1 {
        tonic::include_proto!("mintswarm.watch.v1");
    }
}

/// The protocol's conformance scenarios and, in later changes, fakes of its
/// services.
#[cfg(feature = "fakes")]
pub mod fakes {
    pub mod scenarios;
}
