//! Installation tokens against a scripted local server: narrowed to the permissions asked for,
//! cached until five minutes before expiry, and never shown in `Debug`, `Display` or a log line.
// Not redundant: clippy's `allow-unwrap-in-tests` covers a `tests/` crate only under `cfg(test)`.
#![cfg(test)]

mod support;

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use jsonwebtoken::{Algorithm, DecodingKey, Validation};
use mintswarm_github::{Clock, Permissions, RetryPolicy, TokenClient, TokenError};
use serde::Deserialize;
use support::{Capture, Fakes, Home, Responder};

const CLIENT_ID: &str = "Iv23liTokenTest";
const SERVICE: &str = "mintswarm-dev";
const NOW: u64 = 1_800_000_000; // 2027-01-15T08:00:00Z
const EXPIRES_AT: &str = "2027-01-15T09:00:00Z";
const TOKEN: &str = "ghs_T0kenSecretValue0123456789abcdef";
const OTHER_TOKEN: &str = "ghs_0therSecretValue0123456789abcdef";

struct Setup {
    client: TokenClient,
    server: Responder,
    now: Arc<AtomicU64>,
    _fakes: Fakes,
    _home: Home,
}

/// The capturing subscriber, installed as the global default before any test in this binary
/// logs, so every callsite registers with it whichever thread hits it first. It records the
/// events of every test here.
fn capture() -> &'static Capture {
    static CAPTURE: OnceLock<Capture> = OnceLock::new();
    CAPTURE.get_or_init(|| {
        let capture = Capture::default();
        tracing::subscriber::set_global_default(capture.clone())
            .expect("no other subscriber is installed in this test binary");
        capture
    })
}

async fn setup() -> Setup {
    capture();
    let fakes = Fakes::new(false);
    let home = Home::new();
    home.put_file_key(SERVICE, CLIENT_ID, &support::key_pair().private_pem);
    let config: PathBuf = home.put_app_config(CLIENT_ID, SERVICE);
    let server = Responder::start().await;
    let now = Arc::new(AtomicU64::new(NOW));
    let clock = {
        let now = Arc::clone(&now);
        Clock::from_fn(move || now.load(Ordering::SeqCst))
    };
    let client = TokenClient::new(
        server.base.clone(),
        config,
        fakes.tools.clone(),
        home.env.clone(),
        clock,
    )
    .unwrap()
    // A single attempt per request, so each request takes one scripted reply.
    .with_retry(RetryPolicy {
        attempts: 1,
        ..RetryPolicy::default()
    });
    Setup {
        client,
        server,
        now,
        _fakes: fakes,
        _home: home,
    }
}

fn script_mint(server: &Responder, token: &str) {
    server.reply(200, r#"{"id": 4242, "account": {"login": "mintswarm"}}"#);
    server.reply(
        201,
        format!(
            r#"{{"token": "{token}", "expires_at": "{EXPIRES_AT}",
                "permissions": {{"contents": "read"}}, "repository_selection": "selected"}}"#
        ),
    );
}

fn contents_read() -> Permissions {
    Permissions::read(["contents"])
}

#[derive(Deserialize)]
struct Claims {
    iat: u64,
    exp: u64,
    iss: String,
}

#[tokio::test]
async fn a_read_token_asks_for_exactly_the_permissions_given() {
    let setup = setup().await;
    script_mint(&setup.server, TOKEN);
    let token = setup
        .client
        .token("mintswarm", "mintswarm", &contents_read())
        .await
        .unwrap();
    assert_eq!(token.expose(), TOKEN);
    assert_eq!(token.expires_at(), NOW + 3600);

    let requests = setup.server.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].method, "GET");
    assert_eq!(requests[0].path, "/repos/mintswarm/mintswarm/installation");
    assert_eq!(requests[1].method, "POST");
    assert_eq!(requests[1].path, "/app/installations/4242/access_tokens");
    let body: serde_json::Value = serde_json::from_str(&requests[1].body).unwrap();
    assert_eq!(
        body,
        serde_json::json!({"repositories": ["mintswarm"], "permissions": {"contents": "read"}})
    );
    for request in &requests {
        assert_eq!(
            request.header("accept"),
            Some("application/vnd.github+json")
        );
        assert_eq!(request.header("x-github-api-version"), Some("2022-11-28"));
    }
}

#[tokio::test]
async fn a_write_token_asks_for_issues_write_and_metadata_read_on_one_repository() {
    let setup = setup().await;
    script_mint(&setup.server, TOKEN);
    script_mint(&setup.server, OTHER_TOKEN);
    let write = Permissions::write(["issues"]).and(Permissions::read(["metadata"]));
    let token = setup
        .client
        .token("mintswarm", "mintswarm", &write)
        .await
        .unwrap();
    assert_eq!(token.expose(), TOKEN);

    let requests = setup.server.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[1].path, "/app/installations/4242/access_tokens");
    let body: serde_json::Value = serde_json::from_str(&requests[1].body).unwrap();
    assert_eq!(
        body,
        serde_json::json!({
            "repositories": ["mintswarm"],
            "permissions": {"issues": "write", "metadata": "read"},
        })
    );

    // The same names at read level are another set, with a token of their own.
    let read = Permissions::read(["issues", "metadata"]);
    let token = setup
        .client
        .token("mintswarm", "mintswarm", &read)
        .await
        .unwrap();
    assert_eq!(token.expose(), OTHER_TOKEN);
    let requests = setup.server.requests();
    assert_eq!(requests.len(), 4);
    let body: serde_json::Value = serde_json::from_str(&requests[3].body).unwrap();
    assert_eq!(
        body["permissions"],
        serde_json::json!({"issues": "read", "metadata": "read"})
    );
}

#[tokio::test]
async fn the_requests_carry_a_jwt_that_verifies() {
    let setup = setup().await;
    script_mint(&setup.server, TOKEN);
    setup
        .client
        .token("mintswarm", "mintswarm", &contents_read())
        .await
        .unwrap();
    let mut validation = Validation::new(Algorithm::RS256);
    validation.validate_exp = false;
    validation.set_issuer(&[CLIENT_ID]);
    let key = DecodingKey::from_rsa_pem(&support::key_pair().public_pem).unwrap();
    for request in setup.server.requests() {
        let jwt = request
            .header("authorization")
            .and_then(|value| value.strip_prefix("Bearer "))
            .unwrap();
        let claims = jsonwebtoken::decode::<Claims>(jwt, &key, &validation)
            .unwrap()
            .claims;
        assert_eq!(claims.iss, CLIENT_ID);
        assert_eq!((claims.iat, claims.exp), (NOW - 60, NOW + 540));
    }
}

#[tokio::test]
async fn an_empty_permission_set_is_refused_before_any_request() {
    let setup = setup().await;
    script_mint(&setup.server, TOKEN);
    let error = setup
        .client
        .token("mintswarm", "mintswarm", &Permissions::read([]))
        .await
        .unwrap_err();
    assert!(matches!(error, TokenError::NoPermissions), "{error}");
    assert!(setup.server.requests().is_empty());
}

#[tokio::test]
async fn invalid_permission_and_repository_names_are_refused_before_any_request() {
    let setup = setup().await;
    for (owner, repo, permission) in [
        ("mintswarm", "mintswarm", "Contents"),
        ("mintswarm", "mintswarm", "contents\":\"write"),
        ("mint/swarm", "mintswarm", "contents"),
        ("mintswarm", "..", "contents"),
    ] {
        let permissions = Permissions::read([permission]);
        let error = setup
            .client
            .token(owner, repo, &permissions)
            .await
            .unwrap_err();
        assert!(matches!(error, TokenError::InvalidName { .. }), "{error}");
    }
    assert!(setup.server.requests().is_empty());
}

#[tokio::test]
async fn a_token_is_reused_until_five_minutes_before_expiry() {
    let setup = setup().await;
    script_mint(&setup.server, TOKEN);
    let first = setup
        .client
        .token("mintswarm", "mintswarm", &contents_read())
        .await
        .unwrap();

    setup.now.store(NOW + 60, Ordering::SeqCst);
    let second = setup
        .client
        .token("mintswarm", "mintswarm", &contents_read())
        .await
        .unwrap();
    assert_eq!(second, first);
    assert_eq!(setup.server.requests().len(), 2);

    setup.now.store(NOW + 3600 - 240, Ordering::SeqCst);
    script_mint(&setup.server, OTHER_TOKEN);
    let third = setup
        .client
        .token("mintswarm", "mintswarm", &contents_read())
        .await
        .unwrap();
    assert_eq!(third.expose(), OTHER_TOKEN);
    assert_eq!(setup.server.requests().len(), 4);
}

#[tokio::test]
async fn another_permission_set_gets_its_own_token() {
    let setup = setup().await;
    script_mint(&setup.server, TOKEN);
    script_mint(&setup.server, OTHER_TOKEN);
    let contents = setup
        .client
        .token("mintswarm", "mintswarm", &contents_read())
        .await
        .unwrap();
    let issues = Permissions::read(["issues", "contents"]);
    let both = setup
        .client
        .token("mintswarm", "mintswarm", &issues)
        .await
        .unwrap();
    assert_eq!(contents.expose(), TOKEN);
    assert_eq!(both.expose(), OTHER_TOKEN);
    let requests = setup.server.requests();
    assert_eq!(requests.len(), 4);
    let body: serde_json::Value = serde_json::from_str(&requests[3].body).unwrap();
    assert_eq!(
        body["permissions"],
        serde_json::json!({"contents": "read", "issues": "read"})
    );
}

#[tokio::test]
async fn a_token_that_is_not_an_installation_token_is_refused() {
    let setup = setup().await;
    script_mint(&setup.server, "gho_notAnInstallationToken");
    let error = setup
        .client
        .token("mintswarm", "mintswarm", &contents_read())
        .await
        .unwrap_err();
    assert!(matches!(error, TokenError::Response { .. }), "{error}");
    assert!(!format!("{error} {error:?}").contains("notAnInstallationToken"));
}

#[tokio::test]
async fn no_log_line_holds_the_token() {
    let capture = capture();
    let setup = setup().await;
    script_mint(&setup.server, TOKEN);
    let token = setup
        .client
        .token("mintswarm", "mintswarm", &contents_read())
        .await
        .unwrap();
    setup
        .client
        .token("mintswarm", "mintswarm", &contents_read())
        .await
        .unwrap();
    assert_eq!(token.expose(), TOKEN);

    let lines = capture.lines();
    assert!(
        lines
            .iter()
            .any(|line| line.contains("minted an installation token")),
        "{lines:#?}"
    );
    assert!(
        lines.iter().any(|line| line.contains("using the cached")),
        "{lines:#?}"
    );
    assert!(
        lines.iter().any(|line| line.contains("unencrypted")),
        "{lines:#?}"
    );
    let jwt_prefix = "eyJ";
    for line in &lines {
        assert!(!line.contains(TOKEN), "{line}");
        assert!(!line.contains("T0kenSecret"), "{line}");
        assert!(!line.contains("0therSecret"), "{line}");
        assert!(!line.contains(jwt_prefix), "{line}");
        assert!(!line.contains("PRIVATE KEY"), "{line}");
    }
}

#[tokio::test]
async fn no_debug_or_display_shows_the_token() {
    let setup = setup().await;
    script_mint(&setup.server, TOKEN);
    let token = setup
        .client
        .token("mintswarm", "mintswarm", &contents_read())
        .await
        .unwrap();
    for text in [
        format!("{token:?}"),
        format!("{token:#?}"),
        format!("{:?}", setup.client),
        format!("{:#?}", setup.client),
    ] {
        assert!(!text.contains(TOKEN), "{text}");
        assert!(!text.contains("T0kenSecret"), "{text}");
    }
    assert!(format!("{:?}", setup.client).contains("tokens: 1"));
}

#[tokio::test]
async fn an_error_response_is_reported_without_its_body() {
    let setup = setup().await;
    setup.server.reply(200, r#"{"id": 4242}"#);
    setup.server.reply(
        422,
        format!(r#"{{"message": "Validation failed {TOKEN}", "token": "{TOKEN}"}}"#),
    );
    let error = setup
        .client
        .token("mintswarm", "mintswarm", &contents_read())
        .await
        .unwrap_err();
    match &error {
        TokenError::Status { endpoint, status } => {
            assert_eq!(status.as_u16(), 422);
            assert_eq!(endpoint, "POST /app/installations/4242/access_tokens");
        }
        other => panic!("unexpected error: {other}"),
    }
    for text in [
        error.to_string(),
        format!("{error:?}"),
        format!("{error:#?}"),
    ] {
        assert!(!text.contains("ghs_"), "{text}");
        assert!(!text.contains("Validation failed"), "{text}");
    }
}

#[tokio::test]
async fn a_server_error_on_mint_is_retried() {
    let setup = setup().await;
    let client = setup.client.with_retry(RetryPolicy {
        base_delay: Duration::from_millis(1),
        max_delay: Duration::from_millis(1),
        ..RetryPolicy::default()
    });
    setup.server.reply(500, "");
    setup.server.reply(200, r#"{"id": 4242}"#);
    setup.server.reply(502, "");
    setup.server.reply(
        201,
        format!(r#"{{"token": "{TOKEN}", "expires_at": "{EXPIRES_AT}"}}"#),
    );
    let token = client
        .token("mintswarm", "mintswarm", &contents_read())
        .await
        .unwrap();
    assert_eq!(token.expose(), TOKEN);
    let requests: Vec<_> = setup
        .server
        .requests()
        .into_iter()
        .map(|request| (request.method, request.path))
        .collect();
    let lookup = (
        String::from("GET"),
        String::from("/repos/mintswarm/mintswarm/installation"),
    );
    let mint = (
        String::from("POST"),
        String::from("/app/installations/4242/access_tokens"),
    );
    assert_eq!(requests, [lookup.clone(), lookup, mint.clone(), mint]);
}
