//! The `mintswarm.auth.v1` contract (design #634, §5 and §13): its value
//! sets, field shapes and HTTP annotations, checked against descriptors
//! compiled with `protox`; the redaction of every `Secret` it holds; the
//! exported tonic client and server; and `docs/architecture/authentication.md`
//! against the contract.

#![cfg(test)]

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use mintswarm_proto::auth::v1::{
    ActionKind, BeginLinkResponse, BeginLoginResponse, CompleteLinkRequest, CompleteLoginRequest,
    CompleteLoginResponse, DeviceChallenge, GetSessionRequest, LoopbackRedirect,
    PollDeviceLinkRequest, PollDeviceLoginRequest, PollDeviceLoginResponse, SessionGrant,
    auth_service_client, auth_service_server, begin_link_response, begin_login_response,
    complete_login_request, poll_device_login_response,
};
use mintswarm_proto::common::v1::Secret;
use mintswarm_proto::json;
use mintswarm_proto::prost_types::{
    DescriptorProto, Duration, EnumDescriptorProto, FieldDescriptorProto, FileDescriptorSet,
    Timestamp,
};
use prost_reflect::{DescriptorPool, Kind, MessageDescriptor};

/// A value no formatting should ever print.
const SENTINEL: &str = "sentinel-secret-5c1d";

/// The service file, relative to `api/proto`.
const AUTH_PROTO: &str = "mintswarm/auth/v1/auth.proto";

/// The availability file, relative to `api/proto`.
const AVAILABILITY_PROTO: &str = "mintswarm/auth/v1/availability.proto";

/// The full name of the shared `Secret`.
const SECRET: &str = ".mintswarm.common.v1.Secret";

/// Every `mintswarm.auth.v1` message that holds a `Secret`, directly or
/// nested. A change that adds a `Secret` to another message must add that
/// message here and to `secrets_are_redacted`.
const SECRET_HOLDERS: [&str; 11] = [
    "DeviceChallenge",
    "BeginLoginResponse",
    "BeginLinkResponse",
    "PollDeviceLoginRequest",
    "PollDeviceLoginResponse",
    "PollDeviceLinkRequest",
    "LoopbackRedirect",
    "SessionGrant",
    "CompleteLoginRequest",
    "CompleteLoginResponse",
    "CompleteLinkRequest",
];

/// The 13 RPCs of `AuthService`.
const RPCS: [&str; 13] = [
    "ListProviders",
    "BeginLogin",
    "PollDeviceLogin",
    "CompleteLogin",
    "GetSession",
    "ListSessions",
    "RevokeSession",
    "Logout",
    "BeginLink",
    "PollDeviceLink",
    "CompleteLink",
    "Unlink",
    "GetLink",
];

/// The RPCs that carry a `google.api.http` annotation (design #634, §13).
const ANNOTATED: [&str; 6] = [
    "ListProviders",
    "GetSession",
    "ListSessions",
    "RevokeSession",
    "GetLink",
    "Unlink",
];

/// The architecture document that describes the contract.
const AUTHENTICATION_MD: &str = include_str!("../../../docs/architecture/authentication.md");

/// `api/proto`.
fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../api/proto")
}

/// Compiles both `auth.v1` files with `protox::compile`.
fn compiled() -> FileDescriptorSet {
    let root = root();
    protox::compile(
        [root.join(AUTH_PROTO), root.join(AVAILABILITY_PROTO)],
        [&root],
    )
    .expect("the auth.v1 protos compile")
}

/// The descriptor of the `mintswarm.auth.v1` message `name`.
fn message(name: &str) -> DescriptorProto {
    compiled()
        .file
        .into_iter()
        .filter(|file| file.package() == "mintswarm.auth.v1")
        .flat_map(|file| file.message_type)
        .find(|candidate| candidate.name() == name)
        .unwrap_or_else(|| panic!("mintswarm.auth.v1 defines {name}"))
}

/// The field `field` of the `mintswarm.auth.v1` message `name`.
fn field(name: &str, field: &str) -> FieldDescriptorProto {
    message(name)
        .field
        .into_iter()
        .find(|candidate| candidate.name() == field)
        .unwrap_or_else(|| panic!("no field {name}.{field}"))
}

/// The descriptor of the `mintswarm.auth.v1` enum `name`.
fn enumeration(name: &str) -> EnumDescriptorProto {
    compiled()
        .file
        .into_iter()
        .filter(|file| file.package() == "mintswarm.auth.v1")
        .flat_map(|file| file.enum_type)
        .find(|candidate| candidate.name() == name)
        .unwrap_or_else(|| panic!("mintswarm.auth.v1 defines {name}"))
}

/// Asserts that the enum `name` has `<prefix>_UNSPECIFIED = 0` and exactly
/// the values `<prefix>_<VALUE>` for each lowercase `value` besides it.
fn assert_values(name: &str, prefix: &str, values: &[&str]) {
    let descriptor = enumeration(name);
    let zero = descriptor
        .value
        .iter()
        .find(|value| value.number() == 0)
        .unwrap_or_else(|| panic!("{name} has no zero value"));
    assert_eq!(zero.name(), format!("{prefix}_UNSPECIFIED"));

    let found: BTreeSet<String> = descriptor
        .value
        .iter()
        .filter(|value| value.number() != 0)
        .map(|value| value.name().to_owned())
        .collect();
    let expected: BTreeSet<String> = values
        .iter()
        .map(|value| format!("{prefix}_{}", value.to_ascii_uppercase()))
        .collect();
    assert_eq!(found, expected, "{name}");
}

fn secret() -> Option<Secret> {
    Some(Secret {
        value: SENTINEL.to_owned(),
    })
}

/// Writes `message` as proto3 JSON text through `mintswarm_proto::json`.
fn to_json<M: prost::Message + prost::Name>(message: &M) -> String {
    let mut written = Vec::new();
    json::serialize(message, &mut serde_json::Serializer::new(&mut written)).expect("serialises");
    String::from_utf8(written).expect("JSON is UTF-8")
}

/// One redaction case: a message's name with its `Debug` output, both forms,
/// and its JSON.
struct Case {
    name: &'static str,
    printed: [String; 2],
    written: String,
}

fn case<M: prost::Message + prost::Name + std::fmt::Debug>(
    name: &'static str,
    message: &M,
) -> Case {
    Case {
        name,
        printed: [format!("{message:?}"), format!("{message:#?}")],
        written: to_json(message),
    }
}

fn challenge() -> DeviceChallenge {
    DeviceChallenge {
        user_code: "ABCD-1234".to_owned(),
        verification_url: "https://github.com/login/device".to_owned(),
        interval: Some(Duration {
            seconds: 5,
            nanos: 0,
        }),
        expires_at: Some(Timestamp {
            seconds: 1_800_000_000,
            nanos: 0,
        }),
        handle: secret(),
    }
}

fn redirect() -> LoopbackRedirect {
    LoopbackRedirect {
        code: secret(),
        state: "state-1".to_owned(),
    }
}

fn grant() -> SessionGrant {
    SessionGrant {
        session_id: "session-1".to_owned(),
        token: secret(),
        expires_at: Some(Timestamp {
            seconds: 1_800_000_000,
            nanos: 0,
        }),
        idle_limit: Some(Duration {
            seconds: 86_400,
            nanos: 0,
        }),
    }
}

/// Every message of `SECRET_HOLDERS` prints and writes `Secret(<redacted>)`
/// or `{"value":"<redacted>"}`, never the value; `CompleteLoginRequest` in
/// both credentials cases.
#[test]
fn secrets_are_redacted() {
    let cases = [
        case("DeviceChallenge", &challenge()),
        case(
            "BeginLoginResponse",
            &BeginLoginResponse {
                start: Some(begin_login_response::Start::Device(challenge())),
            },
        ),
        case(
            "BeginLinkResponse",
            &BeginLinkResponse {
                start: Some(begin_link_response::Start::Device(challenge())),
            },
        ),
        case(
            "PollDeviceLoginRequest",
            &PollDeviceLoginRequest { handle: secret() },
        ),
        case(
            "PollDeviceLoginResponse",
            &PollDeviceLoginResponse {
                result: Some(poll_device_login_response::Result::Session(grant())),
            },
        ),
        case(
            "PollDeviceLinkRequest",
            &PollDeviceLinkRequest { handle: secret() },
        ),
        case("LoopbackRedirect", &redirect()),
        case("SessionGrant", &grant()),
        case(
            "CompleteLoginRequest",
            &CompleteLoginRequest {
                provider: "token".to_owned(),
                client_kind: mintswarm_proto::auth::v1::ClientKind::Cli.into(),
                host: "laptop".to_owned(),
                credentials: Some(complete_login_request::Credentials::Token(Secret {
                    value: SENTINEL.to_owned(),
                })),
            },
        ),
        case(
            "CompleteLoginRequest",
            &CompleteLoginRequest {
                provider: "oidc".to_owned(),
                client_kind: mintswarm_proto::auth::v1::ClientKind::ConsoleMcp.into(),
                host: "laptop".to_owned(),
                credentials: Some(complete_login_request::Credentials::Redirect(redirect())),
            },
        ),
        case(
            "CompleteLoginResponse",
            &CompleteLoginResponse {
                session: Some(grant()),
            },
        ),
        case(
            "CompleteLinkRequest",
            &CompleteLinkRequest {
                redirect: Some(redirect()),
            },
        ),
    ];

    let covered: BTreeSet<&str> = cases.iter().map(|case| case.name).collect();
    assert_eq!(covered, BTreeSet::from(SECRET_HOLDERS));

    for Case {
        name,
        printed,
        written,
    } in &cases
    {
        for printed in printed {
            assert!(
                !printed.contains(SENTINEL),
                "{name} prints its secret: {printed}"
            );
            assert!(
                printed.contains("Secret(<redacted>)"),
                "{name} does not print the redaction: {printed}"
            );
        }
        assert!(
            !written.contains(SENTINEL),
            "{name} writes its secret: {written}"
        );
        assert!(
            written.contains(r#"{"value":"<redacted>"}"#),
            "{name} does not write the redaction: {written}"
        );
    }
}

/// Whether `message` reaches a `Secret` through its fields, at any depth.
fn reaches_secret(message: &MessageDescriptor, seen: &mut BTreeSet<String>) -> bool {
    if format!(".{}", message.full_name()) == SECRET {
        return true;
    }
    if !seen.insert(message.full_name().to_owned()) {
        return false;
    }
    message.fields().any(|field| match field.kind() {
        Kind::Message(inner) => reaches_secret(&inner, seen),
        _ => false,
    })
}

/// A walk over the package's descriptors finds exactly `SECRET_HOLDERS`, so
/// the redaction test covers every message that holds a `Secret`.
#[test]
fn every_secret_holder_is_covered() {
    let pool =
        DescriptorPool::from_file_descriptor_set(compiled()).expect("the descriptor set decodes");
    let holders: BTreeSet<String> = pool
        .all_messages()
        .filter(|message| message.package_name() == "mintswarm.auth.v1")
        .filter(|message| reaches_secret(message, &mut BTreeSet::new()))
        .map(|message| message.name().to_owned())
        .collect();
    let expected: BTreeSet<String> = SECRET_HOLDERS.iter().map(|&name| name.to_owned()).collect();
    assert_eq!(holders, expected);
}

/// No JSON that holds a `Secret` is read; a message without one parses.
#[test]
fn secret_is_never_read_from_json() {
    assert!(
        json::deserialize::<CompleteLoginRequest, _>(serde_json::json!({
            "provider": "token",
            "clientKind": "CLIENT_KIND_CLI",
            "token": {"value": "x"},
        }))
        .is_err()
    );
    assert!(
        json::deserialize::<PollDeviceLoginRequest, _>(serde_json::json!({
            "handle": {"value": "x"},
        }))
        .is_err()
    );
    assert!(
        json::deserialize::<PollDeviceLinkRequest, _>(serde_json::json!({
            "handle": {"value": "x"},
        }))
        .is_err()
    );
    assert!(
        json::deserialize::<CompleteLinkRequest, _>(serde_json::json!({
            "redirect": {"code": {"value": "x"}, "state": "state-1"},
        }))
        .is_err()
    );

    let _: GetSessionRequest = json::deserialize(serde_json::json!({})).expect("parses");
    let parsed: CompleteLoginRequest = json::deserialize(serde_json::json!({
        "provider": "github",
        "clientKind": "CLIENT_KIND_CLI",
        "host": "laptop",
    }))
    .expect("parses");
    assert_eq!(parsed.credentials, None);
    assert_eq!(parsed.provider, "github");
}

#[test]
fn availability_state_values() {
    assert_values(
        "AvailabilityState",
        "AVAILABILITY_STATE",
        &["act", "confirm", "open_on_github", "hidden"],
    );
}

#[test]
fn availability_reason_values() {
    assert_values(
        "AvailabilityReason",
        "AVAILABILITY_REASON",
        &[
            "no_link",
            "link_broken",
            "insufficient_role",
            "not_trusted",
            "app_permission_missing",
            "endpoint_refuses_user_token",
            "non_interactive_session",
            "acting_disabled",
        ],
    );
}

#[test]
fn action_kind_values() {
    assert_values(
        "ActionKind",
        "ACTION_KIND",
        &[
            "comment",
            "edit_comment",
            "react",
            "update_artefact",
            "add_label",
            "remove_label",
            "set_field",
            "review",
            "approve_design",
            "approve_plan",
            "approve_decomposition",
            "settings_write",
            "branch_protection",
            "rulesets",
            "repository_settings",
            "issue_types",
            "issue_fields",
            "merge",
        ],
    );
}

#[test]
fn client_kind_values() {
    assert_values("ClientKind", "CLIENT_KIND", &["web", "cli", "console_mcp"]);
}

#[test]
fn link_state_values() {
    assert_values(
        "LinkState",
        "LINK_STATE",
        &["active", "refresh_failed", "revoked"],
    );
}

/// The service has exactly the 13 RPCs, and `google.api.http` is set on
/// exactly the six of design #634, §13. Read through a `protox::Compiler`
/// pool, since `protox::compile`'s `prost_types` descriptors drop extension
/// options.
#[test]
fn http_annotations() {
    let pool = protox::Compiler::new([root()])
        .expect("the include root opens")
        .include_imports(true)
        .open_files([AUTH_PROTO])
        .expect("auth.proto compiles")
        .descriptor_pool();
    let service = pool
        .get_service_by_name("mintswarm.auth.v1.AuthService")
        .expect("auth.proto defines AuthService");
    let http = pool
        .get_extension_by_name("google.api.http")
        .expect("the pool holds google.api.http");

    let methods: BTreeSet<String> = service
        .methods()
        .map(|method| method.name().to_owned())
        .collect();
    let expected: BTreeSet<String> = RPCS.iter().map(|&name| name.to_owned()).collect();
    assert_eq!(methods, expected);

    let annotated: BTreeSet<String> = service
        .methods()
        .filter(|method| method.options().has_extension(&http))
        .map(|method| method.name().to_owned())
        .collect();
    let expected: BTreeSet<String> = ANNOTATED.iter().map(|&name| name.to_owned()).collect();
    assert_eq!(annotated, expected);
}

#[test]
fn begin_login_takes_a_client_kind() {
    assert_eq!(
        field("BeginLoginRequest", "client_kind").type_name(),
        ".mintswarm.auth.v1.ClientKind"
    );
}

#[test]
fn get_link_returns_a_link_state() {
    assert_eq!(
        field("GetLinkResponse", "link").type_name(),
        ".mintswarm.auth.v1.Link"
    );
    assert_eq!(
        field("Link", "state").type_name(),
        ".mintswarm.auth.v1.LinkState"
    );
}

/// `CompleteLogin` and `PollDeviceLogin` return a `SessionGrant`, whose token
/// is a `Secret` with its absolute expiry and idle limit.
#[test]
fn session_token_is_a_secret_with_limits() {
    for (message, name) in [
        ("CompleteLoginResponse", "session"),
        ("PollDeviceLoginResponse", "session"),
    ] {
        assert_eq!(
            field(message, name).type_name(),
            ".mintswarm.auth.v1.SessionGrant",
            "{message}.{name}"
        );
    }
    assert_eq!(field("SessionGrant", "token").type_name(), SECRET);
    assert_eq!(
        field("SessionGrant", "expires_at").type_name(),
        ".google.protobuf.Timestamp"
    );
    assert_eq!(
        field("SessionGrant", "idle_limit").type_name(),
        ".google.protobuf.Duration"
    );
}

#[test]
fn get_link_holds_no_token() {
    for name in ["GetLinkResponse", "Link"] {
        for field in message(name).field {
            assert_ne!(field.type_name(), SECRET, "{name}.{}", field.name());
            assert!(
                !field.name().contains("token"),
                "{name}.{} names a token",
                field.name()
            );
        }
    }
    let _ = field("Link", "state");
    let _ = field("Link", "login");
}

/// `build.rs` keeps the vendored `google.api` files out of code generation.
#[test]
fn no_google_api_module_is_generated() {
    let out = Path::new(env!("OUT_DIR"));
    assert!(out.join("mintswarm.auth.v1.rs").is_file());
    assert!(out.join("mintswarm.common.v1.rs").is_file());
    assert!(!out.join("google.api.rs").exists());
}

/// Names the generated tonic client and server from outside the crate, so
/// they are proven exported.
#[test]
fn client_and_server_are_exported() {
    let _ =
        std::mem::size_of::<auth_service_client::AuthServiceClient<tonic::transport::Channel>>();
    let _ = server_size::<Never>;
}

/// The size of the generated server over `T`; naming it proves the server
/// and its service trait exported.
fn server_size<T: auth_service_server::AuthService>() -> usize {
    std::mem::size_of::<auth_service_server::AuthServiceServer<T>>()
}

/// A service that is never built; it only lets `server_size` be named.
enum Never {}

#[tonic::async_trait]
impl auth_service_server::AuthService for Never {
    async fn list_providers(
        &self,
        _: tonic::Request<mintswarm_proto::auth::v1::ListProvidersRequest>,
    ) -> Result<tonic::Response<mintswarm_proto::auth::v1::ListProvidersResponse>, tonic::Status>
    {
        match *self {}
    }
    async fn begin_login(
        &self,
        _: tonic::Request<mintswarm_proto::auth::v1::BeginLoginRequest>,
    ) -> Result<tonic::Response<BeginLoginResponse>, tonic::Status> {
        match *self {}
    }
    async fn poll_device_login(
        &self,
        _: tonic::Request<PollDeviceLoginRequest>,
    ) -> Result<tonic::Response<PollDeviceLoginResponse>, tonic::Status> {
        match *self {}
    }
    async fn complete_login(
        &self,
        _: tonic::Request<CompleteLoginRequest>,
    ) -> Result<tonic::Response<CompleteLoginResponse>, tonic::Status> {
        match *self {}
    }
    async fn get_session(
        &self,
        _: tonic::Request<GetSessionRequest>,
    ) -> Result<tonic::Response<mintswarm_proto::auth::v1::GetSessionResponse>, tonic::Status> {
        match *self {}
    }
    async fn list_sessions(
        &self,
        _: tonic::Request<mintswarm_proto::auth::v1::ListSessionsRequest>,
    ) -> Result<tonic::Response<mintswarm_proto::auth::v1::ListSessionsResponse>, tonic::Status>
    {
        match *self {}
    }
    async fn revoke_session(
        &self,
        _: tonic::Request<mintswarm_proto::auth::v1::RevokeSessionRequest>,
    ) -> Result<tonic::Response<mintswarm_proto::auth::v1::RevokeSessionResponse>, tonic::Status>
    {
        match *self {}
    }
    async fn logout(
        &self,
        _: tonic::Request<mintswarm_proto::auth::v1::LogoutRequest>,
    ) -> Result<tonic::Response<mintswarm_proto::auth::v1::LogoutResponse>, tonic::Status> {
        match *self {}
    }
    async fn begin_link(
        &self,
        _: tonic::Request<mintswarm_proto::auth::v1::BeginLinkRequest>,
    ) -> Result<tonic::Response<BeginLinkResponse>, tonic::Status> {
        match *self {}
    }
    async fn poll_device_link(
        &self,
        _: tonic::Request<PollDeviceLinkRequest>,
    ) -> Result<tonic::Response<mintswarm_proto::auth::v1::PollDeviceLinkResponse>, tonic::Status>
    {
        match *self {}
    }
    async fn complete_link(
        &self,
        _: tonic::Request<CompleteLinkRequest>,
    ) -> Result<tonic::Response<mintswarm_proto::auth::v1::CompleteLinkResponse>, tonic::Status>
    {
        match *self {}
    }
    async fn unlink(
        &self,
        _: tonic::Request<mintswarm_proto::auth::v1::UnlinkRequest>,
    ) -> Result<tonic::Response<mintswarm_proto::auth::v1::UnlinkResponse>, tonic::Status> {
        match *self {}
    }
    async fn get_link(
        &self,
        _: tonic::Request<mintswarm_proto::auth::v1::GetLinkRequest>,
    ) -> Result<tonic::Response<mintswarm_proto::auth::v1::GetLinkResponse>, tonic::Status> {
        match *self {}
    }
}

/// Each `ActionKind` value with its row label in design #634, §12.
const SECTION_12: [(&str, &str); 18] = [
    ("comment", "Comment on an issue or PR"),
    ("edit_comment", "Edit own comment, react"),
    ("react", "Edit own comment, react"),
    ("update_artefact", "Update a design artefact"),
    ("add_label", "Add or remove a label (incl. `mintswarm`)"),
    ("remove_label", "Add or remove a label (incl. `mintswarm`)"),
    ("set_field", "Set an issue field"),
    ("review", "Review a PR"),
    ("approve_design", "Approve a plan, design or decomposition"),
    ("approve_plan", "Approve a plan, design or decomposition"),
    (
        "approve_decomposition",
        "Approve a plan, design or decomposition",
    ),
    ("settings_write", "Settings-branch write (§10.1)"),
    ("branch_protection", "Branch protection, rulesets"),
    ("rulesets", "Branch protection, rulesets"),
    ("repository_settings", "Repository settings"),
    ("issue_types", "Issue types, issue fields (organisation)"),
    ("issue_fields", "Issue types, issue fields (organisation)"),
    ("merge", "Merge"),
];

/// The first two cells of each row of the doc's `ActionKind` table.
fn action_kind_table() -> Vec<(String, String)> {
    let mut lines = AUTHENTICATION_MD
        .lines()
        .skip_while(|line| !line.starts_with("| `ActionKind` value |"));
    assert!(
        lines.next().is_some(),
        "authentication.md has no ActionKind table"
    );
    lines
        .skip(1)
        .take_while(|line| line.starts_with('|'))
        .map(|line| {
            let cells: Vec<&str> = line.trim_matches('|').split(" | ").map(str::trim).collect();
            assert!(cells.len() >= 2, "a short row: {line}");
            (cells[0].to_owned(), cells[1].to_owned())
        })
        .collect()
}

/// The doc maps every `ActionKind` value to its §12 row, and the fixed table
/// here holds exactly the generated values, so neither drifts alone.
#[test]
fn action_kinds_are_mapped_in_authentication_md() {
    let expected: Vec<(String, String)> = SECTION_12
        .iter()
        .map(|(value, label)| (format!("`{value}`"), (*label).to_owned()))
        .collect();
    assert_eq!(action_kind_table(), expected);

    let generated: BTreeSet<String> = (1..=i32::from(u8::MAX))
        .filter_map(|number| ActionKind::try_from(number).ok())
        .map(|kind| {
            kind.as_str_name()
                .strip_prefix("ACTION_KIND_")
                .expect("every value has the prefix")
                .to_ascii_lowercase()
        })
        .collect();
    let fixed: BTreeMap<&str, &str> = SECTION_12.into_iter().collect();
    let fixed: BTreeSet<String> = fixed.keys().map(|&value| value.to_owned()).collect();
    assert_eq!(generated, fixed);
}

/// Whether `text` uses the edge's `/auth/` route prefix as a route: an
/// `/auth/` that starts the text or follows a space, a backtick or `(`.
fn names_an_edge_route(text: &str) -> bool {
    text.match_indices("/auth/").any(|(at, _)| {
        text[..at]
            .chars()
            .next_back()
            .is_none_or(|before| matches!(before, ' ' | '`' | '(' | '\n'))
    })
}

/// The doc names every RPC, enum and the shared `Secret`, and none of the
/// terms of behaviour that is not built.
#[test]
fn authentication_md_covers_the_contract() {
    // The matcher's own check.
    assert!(!names_an_edge_route(
        "api/proto/mintswarm/auth/v1/auth.proto"
    ));
    assert!(!names_an_edge_route("GET /v1/auth/link"));
    assert!(names_an_edge_route("`/auth/github/callback`"));
    assert!(names_an_edge_route("GET /auth/start"));

    let names = RPCS.iter().chain(&[
        "ActionKind",
        "AvailabilityState",
        "AvailabilityReason",
        "ClientKind",
        "LinkState",
        "ActionAvailability",
        "Secret",
        "mintswarm.common.v1",
    ]);
    for name in names {
        assert!(
            AUTHENTICATION_MD.contains(&format!("`{name}`")),
            "authentication.md does not name {name}"
        );
    }

    assert!(
        !AUTHENTICATION_MD.lines().any(names_an_edge_route),
        "authentication.md names an edge route"
    );
    let lowercase = AUTHENTICATION_MD.to_lowercase();
    for term in ["callback", "cookie", "authprovider", "secretstore", "pkce"] {
        assert!(
            !lowercase.contains(term),
            "authentication.md mentions {term}"
        );
    }
}
