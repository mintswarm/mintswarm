//! Rust code generated from the protos under `api/proto/`: the shared
//! `Secret`, the runner protocol, the job vocabulary, the watch protocol and
//! the authentication contract.
//!
//! It also generates the eight vendored OTLP packages under `api/proto/opentelemetry/`:
//! `opentelemetry.proto.{common,resource,trace,logs,metrics}.v1` and
//! `opentelemetry.proto.collector.{trace,logs,metrics}.v1`, at [`opentelemetry`]. Only
//! [`otlp`] uses them.
//!
//! `build.rs` compiles every `.proto` there into `OUT_DIR`; nothing generated
//! is committed. The messages carry prost's binary encoding; their proto3 JSON
//! mapping goes through [`json`]. The well-known types, such as
//! `google.protobuf.Timestamp`, are at [`prost_types`].

/// The well-known types, such as `Timestamp` and `Duration`, that the
/// generated messages use: `prost-reflect`'s re-export of `prost-types`, so
/// they are the same types that [`json`] serialises.
pub use prost_reflect::prost_types;

/// The `mintswarm.common.v1` package.
pub mod common {
    /// Version 1 of the types the other packages share: `Secret`
    /// (`secret.proto`), which `runner::v1` and `auth::v1` hold.
    pub mod v1 {
        tonic::include_proto!("mintswarm.common.v1");

        /// Prints `Secret(<redacted>)`, never the value (the runner protocol
        /// design on #156, §9.5).
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

/// The `mintswarm.runner.v1` package.
pub mod runner {
    /// Version 1 of the runner protocol: `RunnerService` and its messages
    /// (`runner.proto`), with the generated tonic client and server; the
    /// `Connect` stream's envelopes, `ConnectRequest` and `ConnectResponse`,
    /// and the messages they carry (`connect.proto`); `StepKind`
    /// (`steps.proto`); and the runner events, `RunnerEvent` and `EventBatch`
    /// (`events.proto`). Its secrets are `common::v1::Secret`.
    ///
    /// The client has no `connect(dst)` constructor, since it would clash
    /// with the `Connect` RPC's method: build one with
    /// `RunnerServiceClient::new(channel)`.
    pub mod v1 {
        tonic::include_proto!("mintswarm.runner.v1");
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
    /// and the stream's marks (`watch.proto`); the `github.webhook` body,
    /// `GitHubWebhook`, with `IssueDependency` and `UntrustedText`
    /// (`github.proto`); and `ItemRef`, which both use (`common.proto`).
    ///
    /// Like every client in this crate, the client has no `connect(dst)`
    /// constructor: build one with `WatchServiceClient::new(channel)`.
    pub mod v1 {
        tonic::include_proto!("mintswarm.watch.v1");
    }
}

/// The `mintswarm.auth.v1` package.
pub mod auth {
    /// Version 1 of the authentication contract (design #634, §13): the
    /// availability types, `ActionAvailability` with `ActionKind`,
    /// `AvailabilityState` and `AvailabilityReason` (`availability.proto`);
    /// and `AuthService`, its messages, `ClientKind` and `LinkState`
    /// (`auth.proto`), with the generated tonic client and server. Its
    /// secrets are `common::v1::Secret`.
    ///
    /// Like every client in this crate, the client has no `connect(dst)`
    /// constructor: build one with `AuthServiceClient::new(channel)`.
    pub mod v1 {
        tonic::include_proto!("mintswarm.auth.v1");
    }
}

/// The vendored OTLP packages, `opentelemetry.proto.*`, mounted at their package path so the
/// generated cross-package paths resolve. Only [`otlp`] uses them; no other crate names them.
pub mod opentelemetry {
    /// The `opentelemetry.proto` packages.
    pub mod proto {
        /// The `opentelemetry.proto.common` package.
        pub mod common {
            /// `AnyValue`, `KeyValue` and `InstrumentationScope` (`common.proto`).
            pub mod v1 {
                tonic::include_proto!("opentelemetry.proto.common.v1");
            }
        }

        /// The `opentelemetry.proto.resource` package.
        pub mod resource {
            /// `Resource` (`resource.proto`).
            pub mod v1 {
                tonic::include_proto!("opentelemetry.proto.resource.v1");
            }
        }

        /// The `opentelemetry.proto.trace` package.
        pub mod trace {
            /// Spans and their resource and scope groupings (`trace.proto`).
            pub mod v1 {
                tonic::include_proto!("opentelemetry.proto.trace.v1");
            }
        }

        /// The `opentelemetry.proto.logs` package.
        pub mod logs {
            /// Log records and their resource and scope groupings (`logs.proto`).
            pub mod v1 {
                tonic::include_proto!("opentelemetry.proto.logs.v1");
            }
        }

        /// The `opentelemetry.proto.metrics` package.
        pub mod metrics {
            /// Metrics and their resource and scope groupings (`metrics.proto`).
            pub mod v1 {
                tonic::include_proto!("opentelemetry.proto.metrics.v1");
            }
        }

        /// The `opentelemetry.proto.collector` packages: the export services.
        pub mod collector {
            /// The `opentelemetry.proto.collector.trace` package.
            pub mod trace {
                /// `TraceService` and `ExportTraceServiceRequest` (`trace_service.proto`).
                pub mod v1 {
                    tonic::include_proto!("opentelemetry.proto.collector.trace.v1");
                }
            }

            /// The `opentelemetry.proto.collector.logs` package.
            pub mod logs {
                /// `LogsService` and `ExportLogsServiceRequest` (`logs_service.proto`).
                pub mod v1 {
                    tonic::include_proto!("opentelemetry.proto.collector.logs.v1");
                }
            }

            /// The `opentelemetry.proto.collector.metrics` package.
            pub mod metrics {
                /// `MetricsService` and `ExportMetricsServiceRequest`
                /// (`metrics_service.proto`).
                pub mod v1 {
                    tonic::include_proto!("opentelemetry.proto.collector.metrics.v1");
                }
            }
        }
    }
}

/// The receive checks: each received `runner/v1` message, and a stage's
/// `StageResult`, is accepted or refused with a typed error naming the
/// message, the field and the rule.
pub mod check;

/// The proto3 JSON mapping of every generated message, derived from the
/// protos (design §12.4), with Z-normalised RFC 3339 timestamps.
///
/// `build.rs` writes the descriptor set into `OUT_DIR` and gives every message
/// `prost::Name`; this module finds a message's descriptor by its full name,
/// transcodes it to a `prost_reflect::DynamicMessage` through the binary
/// encoding, and lets `prost-reflect` write or read the JSON with its default
/// options: lowerCamel keys on output, JSON or proto names on input, full
/// enum value names, defaults omitted, proto3 `optional` presence kept, and
/// unknown keys refused.
///
/// Every `Secret`, at any depth, is written as `{"value":"<redacted>"}`, never
/// its value (design §9.5, D15). No JSON that holds a `Secret` is read, since
/// JSON is never a transport for the protocol (design §11); a message whose
/// `Secret` field is absent still parses, with the field `None`.
///
/// The two functions are shaped as a serde `with` module, so a field can use
/// `#[serde(with = "mintswarm_proto::json")]`.
pub mod json {
    use std::sync::LazyLock;

    use prost::{Message, Name};
    use prost_reflect::{
        DescriptorPool, DynamicMessage, MessageDescriptor, ReflectMessage as _, Value,
    };
    use serde::de::Error as _;
    use serde::ser::Error as _;
    use serde::{Deserializer, Serialize as _, Serializer};

    use crate::common::v1::Secret;

    /// What a `Secret`'s value is replaced with.
    const REDACTED: &str = "<redacted>";

    /// The descriptors of every compiled proto, decoded once.
    static DESCRIPTORS: LazyLock<Result<DescriptorPool, String>> = LazyLock::new(|| {
        DescriptorPool::decode(
            include_bytes!(concat!(env!("OUT_DIR"), "/descriptors.bin")).as_slice(),
        )
        .map_err(|error| format!("the descriptor set does not decode: {error}"))
    });

    /// The descriptors of every compiled proto, shared with [`crate::schema`].
    pub(crate) fn pool() -> Result<&'static DescriptorPool, String> {
        DESCRIPTORS.as_ref().map_err(Clone::clone)
    }

    /// The descriptor of the generated message `M`.
    fn descriptor<M: Name>() -> Result<MessageDescriptor, String> {
        let pool = pool()?;
        let name = M::full_name();
        pool.get_message_by_name(&name)
            .ok_or_else(|| format!("no descriptor for {name}"))
    }

    /// Writes `message` as proto3 JSON, with every `Secret` redacted.
    pub fn serialize<M, S>(message: &M, serializer: S) -> Result<S::Ok, S::Error>
    where
        M: Message + Name,
        S: Serializer,
    {
        let desc = descriptor::<M>().map_err(S::Error::custom)?;
        let mut dynamic = DynamicMessage::decode(desc, message.encode_to_vec().as_slice())
            .map_err(S::Error::custom)?;
        redact(&mut dynamic).map_err(S::Error::custom)?;
        dynamic.serialize(serializer)
    }

    /// Reads `M` from proto3 JSON, refusing any JSON that holds a `Secret`.
    pub fn deserialize<'de, M, D>(deserializer: D) -> Result<M, D::Error>
    where
        M: Message + Name + Default,
        D: Deserializer<'de>,
    {
        let desc = descriptor::<M>().map_err(D::Error::custom)?;
        let dynamic = DynamicMessage::deserialize(desc, deserializer)?;
        if holds_secret(&dynamic) {
            return Err(D::Error::custom("a Secret is never read from JSON"));
        }
        dynamic.transcode_to::<M>().map_err(D::Error::custom)
    }

    /// Whether `message` is a `Secret`.
    fn is_secret(message: &DynamicMessage) -> bool {
        message.descriptor().full_name() == Secret::full_name()
    }

    /// Replaces the value of every `Secret` in `message`, itself included.
    fn redact(message: &mut DynamicMessage) -> Result<(), prost::DecodeError> {
        if is_secret(message) {
            message.clear();
            return message.transcode_from(&Secret {
                value: REDACTED.to_owned(),
            });
        }
        for (_, value) in message.fields_mut() {
            redact_value(value)?;
        }
        Ok(())
    }

    /// Redacts every `Secret` inside `value`.
    fn redact_value(value: &mut Value) -> Result<(), prost::DecodeError> {
        match value {
            Value::Message(message) => redact(message),
            Value::List(values) => values.iter_mut().try_for_each(redact_value),
            Value::Map(values) => values.values_mut().try_for_each(redact_value),
            _ => Ok(()),
        }
    }

    /// Whether `message`, or any message inside it, is a `Secret`.
    fn holds_secret(message: &DynamicMessage) -> bool {
        is_secret(message) || message.fields().any(|(_, value)| value_holds_secret(value))
    }

    /// Whether any message inside `value` is a `Secret`.
    fn value_holds_secret(value: &Value) -> bool {
        match value {
            Value::Message(message) => holds_secret(message),
            Value::List(values) => values.iter().any(value_holds_secret),
            Value::Map(values) => values.values().any(value_holds_secret),
            _ => false,
        }
    }
}

pub mod schema;

/// The protocol's conformance scenarios and, in later changes, fakes of its
/// services.
#[cfg(feature = "fakes")]
pub mod fakes {
    pub mod scenarios;
}
