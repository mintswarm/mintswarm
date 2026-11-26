//! Rust code generated from the protos under `api/proto/`: the runner protocol,
//! the job vocabulary and the watch protocol.
//!
//! `build.rs` compiles every `.proto` there into `OUT_DIR`; nothing generated
//! is committed. The messages carry prost's binary encoding; their proto3 JSON
//! mapping goes through [`json`]. The well-known types, such as
//! `google.protobuf.Timestamp`, are at [`prost_types`].

/// The well-known types, such as `Timestamp` and `Duration`, that the
/// generated messages use: `prost-reflect`'s re-export of `prost-types`, so
/// they are the same types that [`json`] serialises.
pub use prost_reflect::prost_types;

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

    use crate::runner::v1::Secret;

    /// What a `Secret`'s value is replaced with.
    const REDACTED: &str = "<redacted>";

    /// The descriptors of every compiled proto, decoded once.
    static DESCRIPTORS: LazyLock<Result<DescriptorPool, String>> = LazyLock::new(|| {
        DescriptorPool::decode(
            include_bytes!(concat!(env!("OUT_DIR"), "/descriptors.bin")).as_slice(),
        )
        .map_err(|error| format!("the descriptor set does not decode: {error}"))
    });

    /// The descriptor of the generated message `M`.
    fn descriptor<M: Name>() -> Result<MessageDescriptor, String> {
        let pool = DESCRIPTORS.as_ref().map_err(Clone::clone)?;
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

/// The protocol's conformance scenarios and, in later changes, fakes of its
/// services.
#[cfg(feature = "fakes")]
pub mod fakes {
    pub mod scenarios;
}
