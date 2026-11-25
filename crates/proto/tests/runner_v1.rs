//! The `mintswarm.runner.v1` code: its messages carry the design's fields,
//! the tonic client and server are exported, and no `Secret` prints its value.

#![cfg(test)]

use mintswarm_proto::runner::v1::{
    AuthenticateRequest, AuthenticateResponse, ConnectRequest, ConnectResponse, GetBlobHeader,
    GetBlobRequest, GetBlobResponse, PutBlobHeader, PutBlobRequest, PutBlobResponse,
    Reauthenticate, RegisterRequest, RegisterResponse, Repository, Secret, StatBlobsRequest,
    StatBlobsResponse, connect_request, get_blob_response, put_blob_request, runner_service_client,
    runner_service_server,
};

/// A value no formatting should ever print.
const SENTINEL: &str = "sentinel-secret-7f3a";

fn secret() -> Option<Secret> {
    Some(Secret {
        value: SENTINEL.to_owned(),
    })
}

fn register_request() -> RegisterRequest {
    RegisterRequest {
        registration_token: secret(),
        public_key: vec![7; 32],
        repository: Some(Repository {
            remote: "https://github.com/mintswarm/mintswarm.git".to_owned(),
        }),
        host: "build-1".to_owned(),
        os: "linux".to_owned(),
        arch: "aarch64".to_owned(),
        tags: vec!["gpu".to_owned()],
        account: "subscription".to_owned(),
        workers_total: 2,
        runner_version: "0.1.0".to_owned(),
        protocol_minor: 0,
    }
}

fn authenticate_request() -> AuthenticateRequest {
    AuthenticateRequest {
        client_assertion_type: "urn:ietf:params:oauth:client-assertion-type:jwt-bearer".to_owned(),
        client_assertion: secret(),
    }
}

fn authenticate_response() -> AuthenticateResponse {
    AuthenticateResponse {
        access_token: secret(),
        expires_at: Some(prost_types::Timestamp {
            seconds: 1_800_000_000,
            nanos: 0,
        }),
    }
}

/// Covers every message that contains a `Secret`, directly or nested. A
/// change that adds a `Secret` to another message must add that message here.
#[test]
fn secrets_are_redacted() {
    let secret = Secret {
        value: SENTINEL.to_owned(),
    };
    let register = register_request();
    let authenticate = authenticate_request();
    let authenticated = authenticate_response();
    let reauthenticate = Reauthenticate {
        access_token: Some(secret.clone()),
    };
    let connect = ConnectRequest {
        body: Some(connect_request::Body::Reauthenticate(
            reauthenticate.clone(),
        )),
    };
    let messages: [(&str, &dyn std::fmt::Debug); 6] = [
        ("Secret", &secret),
        ("RegisterRequest", &register),
        ("AuthenticateRequest", &authenticate),
        ("AuthenticateResponse", &authenticated),
        ("Reauthenticate", &reauthenticate),
        ("ConnectRequest", &connect),
    ];
    for (name, message) in messages {
        for printed in [format!("{message:?}"), format!("{message:#?}")] {
            assert!(
                !printed.contains(SENTINEL),
                "{name} prints its secret: {printed}"
            );
            assert!(
                printed.contains("Secret(<redacted>)"),
                "{name} does not print the redaction: {printed}"
            );
        }
    }
}

/// Builds every message with full struct literals, so a field of design §8.2,
/// §9.1 or §9.2 that goes missing fails to compile.
#[test]
fn messages_carry_the_design_fields() {
    assert_eq!(register_request().public_key.len(), 32);
    let registered = RegisterResponse {
        runner_id: "runner-1".to_owned(),
        project: "mintswarm".to_owned(),
        default_tags: vec!["linux".to_owned()],
        audience: "https://plane.example".to_owned(),
    };
    assert_eq!(registered.runner_id, "runner-1");
    assert!(authenticate_request().client_assertion.is_some());
    assert!(authenticate_response().expires_at.is_some());

    let digest = "a".repeat(64);
    let stat = StatBlobsRequest {
        sha256: vec![digest.clone()],
    };
    let missing = StatBlobsResponse {
        missing: stat.sha256.clone(),
    };
    assert_eq!(missing.missing, [digest.clone()]);

    let put_header = PutBlobRequest {
        part: Some(put_blob_request::Part::Header(PutBlobHeader {
            sha256: digest.clone(),
            size: 3,
            media_type: "application/json".to_owned(),
            attempt_id: "attempt-1".to_owned(),
            lease_epoch: 4,
        })),
    };
    let put_chunk = PutBlobRequest {
        part: Some(put_blob_request::Part::Chunk(b"{}\n".to_vec())),
    };
    assert_ne!(put_header, put_chunk);
    let stored = PutBlobResponse {
        sha256: digest.clone(),
        size: 3,
    };
    assert_eq!(stored.size, 3);

    let get = GetBlobRequest {
        sha256: digest.clone(),
        attempt_id: "attempt-1".to_owned(),
        lease_epoch: 4,
    };
    assert_eq!(get.lease_epoch, 4);
    let get_header = GetBlobResponse {
        part: Some(get_blob_response::Part::Header(GetBlobHeader {
            sha256: digest,
            size: 3,
            media_type: "application/json".to_owned(),
        })),
    };
    let get_chunk = GetBlobResponse {
        part: Some(get_blob_response::Part::Chunk(b"{}\n".to_vec())),
    };
    assert_ne!(get_header, get_chunk);
}

/// Names the generated tonic client, server and service trait from outside
/// the crate, so they are proven exported.
#[test]
fn client_and_server_are_exported() {
    let _ = std::mem::size_of::<
        runner_service_client::RunnerServiceClient<tonic::transport::Channel>,
    >();
    let server = runner_service_server::RunnerServiceServer::new(Unimplemented);
    let _ = server.max_decoding_message_size(2 * 1024 * 1024);
}

/// Calls the `Connect` stub with a request stream and binds its answer to a
/// response stream, so the generated client is proven bidirectional. Never
/// run: it only has to compile.
#[allow(dead_code)]
async fn connect_is_bidirectional(
    client: &mut runner_service_client::RunnerServiceClient<tonic::transport::Channel>,
) {
    let requests = tonic::codegen::tokio_stream::iter(Vec::<ConnectRequest>::new());
    let _: Result<tonic::Response<tonic::Streaming<ConnectResponse>>, tonic::Status> =
        client.connect(requests).await;
}

/// A server that answers nothing; it only proves the trait can be
/// implemented outside the crate.
struct Unimplemented;

#[tonic::async_trait]
impl runner_service_server::RunnerService for Unimplemented {
    async fn register(
        &self,
        _: tonic::Request<RegisterRequest>,
    ) -> Result<tonic::Response<RegisterResponse>, tonic::Status> {
        Err(tonic::Status::unimplemented("register"))
    }

    async fn authenticate(
        &self,
        _: tonic::Request<AuthenticateRequest>,
    ) -> Result<tonic::Response<AuthenticateResponse>, tonic::Status> {
        Err(tonic::Status::unimplemented("authenticate"))
    }

    type ConnectStream = tonic::codegen::BoxStream<ConnectResponse>;

    async fn connect(
        &self,
        _: tonic::Request<tonic::Streaming<ConnectRequest>>,
    ) -> Result<tonic::Response<Self::ConnectStream>, tonic::Status> {
        Err(tonic::Status::unimplemented("connect"))
    }

    async fn stat_blobs(
        &self,
        _: tonic::Request<StatBlobsRequest>,
    ) -> Result<tonic::Response<StatBlobsResponse>, tonic::Status> {
        Err(tonic::Status::unimplemented("stat_blobs"))
    }

    async fn put_blob(
        &self,
        _: tonic::Request<tonic::Streaming<PutBlobRequest>>,
    ) -> Result<tonic::Response<PutBlobResponse>, tonic::Status> {
        Err(tonic::Status::unimplemented("put_blob"))
    }

    type GetBlobStream = tonic::codegen::BoxStream<GetBlobResponse>;

    async fn get_blob(
        &self,
        _: tonic::Request<GetBlobRequest>,
    ) -> Result<tonic::Response<Self::GetBlobStream>, tonic::Status> {
        Err(tonic::Status::unimplemented("get_blob"))
    }
}
