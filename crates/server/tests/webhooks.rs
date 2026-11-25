//! Drives the webhook receiver's router in-process.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::{Duration, UNIX_EPOCH};

use axum::body::Body;
use axum::http::{HeaderMap, Request, StatusCode};
use hmac::{Hmac, KeyInit, Mac};
use mintswarm_server::webhooks::{
    BODY_LIMIT, DeliveryLog, PATH, Receiver, Secret, SecretError, SecretSource, Verdict,
    format_rfc3339_utc, load_secret, log_line, parse_signature, router, verify,
};
use serde_json::{Value, json};
use sha2::Sha256;
use tempfile::TempDir;
use tower::ServiceExt;

const SECRET: &[u8] = b"it's a secret to everybody";

fn signature(key: &[u8], body: &[u8]) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("HMAC takes keys of any length");
    mac.update(body);
    let tag = mac.finalize().into_bytes();
    let hex: String = tag.iter().map(|byte| format!("{byte:02x}")).collect();
    format!("sha256={hex}")
}

fn secret() -> Secret {
    Secret::new(SECRET.to_vec()).expect("the test secret is not empty")
}

struct Fixture {
    _dir: TempDir,
    path: PathBuf,
    receiver: Receiver,
}

fn fixture() -> Fixture {
    let dir = TempDir::new().expect("a temporary directory");
    let path = dir.path().join("deliveries.jsonl");
    let log = DeliveryLog::open(&path).expect("the log opens");
    Fixture {
        _dir: dir,
        path,
        receiver: Receiver::new(secret(), log),
    }
}

fn request(event: &str, delivery: &str, body: Vec<u8>, signature: Option<&str>) -> Request<Body> {
    let mut builder = Request::post(PATH)
        .header("content-type", "application/json")
        .header("x-github-event", event)
        .header("x-github-delivery", delivery);
    if let Some(signature) = signature {
        builder = builder.header("x-hub-signature-256", signature);
    }
    builder.body(Body::from(body)).expect("a valid request")
}

fn signed(event: &str, delivery: &str, body: &[u8]) -> Request<Body> {
    let signature = signature(SECRET, body);
    request(event, delivery, body.to_vec(), Some(&signature))
}

async fn send(receiver: &Receiver, request: Request<Body>) -> StatusCode {
    router(receiver.clone())
        .oneshot(request)
        .await
        .expect("the router is infallible")
        .status()
}

fn lines(path: &Path) -> Vec<Value> {
    let text = std::fs::read_to_string(path).expect("the log is readable");
    text.lines()
        .map(|line| serde_json::from_str(line).expect("each line is JSON"))
        .collect()
}

fn read(path: &Path) -> Vec<u8> {
    std::fs::read(path).expect("the log is readable")
}

#[tokio::test]
async fn valid_signature_is_accepted_and_logged() {
    let fx = fixture();
    let body = br#"{"action":"opened"}"#;
    assert_eq!(
        send(&fx.receiver, signed("issues", "d-1", body)).await,
        StatusCode::ACCEPTED
    );
    let lines = lines(&fx.path);
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0]["delivery"], "d-1");
}

#[tokio::test]
async fn bad_signatures_are_refused_and_not_logged() {
    let fx = fixture();
    let body = br#"{"action":"opened"}"#.to_vec();
    let wrong_key = signature(b"another secret", &body);
    let wrong_body = signature(SECRET, b"{}");
    let cases: [Option<&str>; 7] = [
        None,
        Some(&wrong_key),
        Some(&wrong_body),
        Some(wrong_key.trim_start_matches("sha256=")),
        Some("sha1=0123456789abcdef0123456789abcdef01234567"),
        Some("sha256=zz23456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"),
        Some("sha256=0123"),
    ];
    for header in cases {
        assert_eq!(
            send(&fx.receiver, request("issues", "d-1", body.clone(), header)).await,
            StatusCode::UNAUTHORIZED,
            "{header:?}"
        );
    }
    assert!(read(&fx.path).is_empty());
}

#[test]
fn verify_gives_each_verdict() {
    let secret = secret();
    let body = b"payload";
    let good = signature(SECRET, body);
    assert_eq!(verify(&secret, body, Some(good.as_bytes())), Verdict::Valid);
    assert_eq!(
        verify(
            &secret,
            body,
            Some(good.to_uppercase().replace("SHA256=", "sha256=").as_bytes())
        ),
        Verdict::Valid
    );
    assert_eq!(verify(&secret, body, None), Verdict::Missing);
    assert_eq!(
        verify(&secret, b"other", Some(good.as_bytes())),
        Verdict::Bad
    );
    assert_eq!(verify(&secret, body, Some(b"sha256=")), Verdict::Bad);
}

#[test]
fn parse_signature_reads_only_sha256_hex() {
    let hex = "00ff".repeat(16);
    let parsed = parse_signature(format!("sha256={hex}").as_bytes()).expect("parses");
    assert_eq!(parsed[0], 0x00);
    assert_eq!(parsed[1], 0xff);
    assert_eq!(parse_signature(hex.as_bytes()), None);
    assert_eq!(parse_signature(format!("sha256={hex}0").as_bytes()), None);
    assert_eq!(
        parse_signature(format!("sha256= {}", &hex[1..]).as_bytes()),
        None
    );
}

#[tokio::test]
async fn oversize_body_is_refused_and_not_logged() {
    let fx = fixture();
    let body = vec![b'a'; BODY_LIMIT + 1];
    assert_eq!(
        send(&fx.receiver, signed("push", "d-big", &body)).await,
        StatusCode::PAYLOAD_TOO_LARGE
    );
    assert!(read(&fx.path).is_empty());
}

#[tokio::test]
async fn body_at_the_limit_is_accepted() {
    let fx = fixture();
    let body = vec![b'a'; BODY_LIMIT];
    assert_eq!(
        send(&fx.receiver, signed("push", "d-max", &body)).await,
        StatusCode::ACCEPTED
    );
    assert_eq!(lines(&fx.path).len(), 1);
}

#[tokio::test]
async fn ping_and_unknown_events_are_logged() {
    let fx = fixture();
    for event in ["ping", "made_up_event"] {
        assert_eq!(
            send(
                &fx.receiver,
                signed(event, event, br#"{"zen":"Keep it simple."}"#)
            )
            .await,
            StatusCode::ACCEPTED
        );
    }
    let events: Vec<Value> = lines(&fx.path)
        .into_iter()
        .map(|line| line["event"].clone())
        .collect();
    assert_eq!(events, [json!("ping"), json!("made_up_event")]);
}

#[tokio::test]
async fn delivery_is_appended_after_existing_lines() {
    let dir = TempDir::new().expect("a temporary directory");
    let path = dir.path().join("deliveries.jsonl");
    std::fs::write(&path, "{\"earlier\":true}\n").expect("the log is writable");
    let receiver = Receiver::new(secret(), DeliveryLog::open(&path).expect("the log opens"));
    assert_eq!(
        send(&receiver, signed("issues", "d-2", b"{}")).await,
        StatusCode::ACCEPTED
    );
    let lines = lines(&path);
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0], json!({"earlier": true}));
    assert_eq!(lines[1]["delivery"], "d-2");
}

#[tokio::test]
async fn concurrent_deliveries_give_whole_lines() {
    let fx = fixture();
    let big = json!({"blob": "x".repeat(1 << 20)}).to_string();
    let (first, second) = tokio::join!(
        send(&fx.receiver, signed("push", "d-a", big.as_bytes())),
        send(&fx.receiver, signed("push", "d-b", big.as_bytes())),
    );
    assert_eq!(
        (first, second),
        (StatusCode::ACCEPTED, StatusCode::ACCEPTED)
    );
    let mut deliveries: Vec<Value> = lines(&fx.path)
        .into_iter()
        .map(|line| line["delivery"].clone())
        .collect();
    deliveries.sort_by_key(|value| value.to_string());
    assert_eq!(deliveries, [json!("d-a"), json!("d-b")]);
}

#[test]
fn line_has_the_seven_keys_in_order() {
    let mut headers = HeaderMap::new();
    headers.insert("x-github-delivery", "d-3".parse().expect("a header value"));
    headers.insert("x-github-event", "issues".parse().expect("a header value"));
    let payload = json!({
        "action": "opened",
        "installation": {"id": 42, "node_id": "n"},
        "repository": {"full_name": "owner/name", "id": 7},
        "issue": {"body": "two\nlines"},
    });
    let body = serde_json::to_vec_pretty(&payload).expect("serialises");
    let line = log_line("2024-02-29T12:34:56.789Z", &headers, &body);

    assert_eq!(line.last(), Some(&b'\n'));
    assert_eq!(line.iter().filter(|&&byte| byte == b'\n').count(), 1);
    let text = std::str::from_utf8(&line).expect("UTF-8");
    let value: Value = serde_json::from_str(text).expect("JSON");
    assert_eq!(
        value,
        json!({
            "received_at": "2024-02-29T12:34:56.789Z",
            "delivery": "d-3",
            "event": "issues",
            "action": "opened",
            "installation": 42,
            "repository": "owner/name",
            "payload": payload,
        })
    );
    let keys = [
        "received_at",
        "delivery",
        "event",
        "action",
        "installation",
        "repository",
        "payload",
    ];
    let positions: Vec<usize> = keys
        .iter()
        .map(|key| {
            text.find(&format!("\"{key}\":"))
                .expect("the key is present")
        })
        .collect();
    assert!(
        positions.windows(2).all(|pair| pair[0] < pair[1]),
        "{positions:?}"
    );
}

#[test]
fn line_has_nulls_for_what_is_absent() {
    let line = log_line(
        "1970-01-01T00:00:00.000Z",
        &HeaderMap::new(),
        b"{\"zen\":\"z\"}",
    );
    let value: Value = serde_json::from_slice(&line).expect("JSON");
    assert_eq!(
        value,
        json!({
            "received_at": "1970-01-01T00:00:00.000Z",
            "delivery": null,
            "event": null,
            "action": null,
            "installation": null,
            "repository": null,
            "payload": {"zen": "z"},
        })
    );
}

#[test]
fn line_keeps_a_body_that_is_not_json_as_a_string() {
    let line = log_line(
        "1970-01-01T00:00:00.000Z",
        &HeaderMap::new(),
        b"payload=%7B%7D\n",
    );
    assert_eq!(line.iter().filter(|&&byte| byte == b'\n').count(), 1);
    let value: Value = serde_json::from_slice(&line).expect("JSON");
    assert_eq!(value["payload"], "payload=%7B%7D\n");
    assert_eq!(value["action"], Value::Null);
}

#[test]
fn received_at_is_rfc3339_utc_with_milliseconds() {
    let at = |secs: u64, millis: u64| {
        format_rfc3339_utc(UNIX_EPOCH + Duration::from_secs(secs) + Duration::from_millis(millis))
    };
    assert_eq!(at(0, 0), "1970-01-01T00:00:00.000Z");
    assert_eq!(at(1_709_210_096, 789), "2024-02-29T12:34:56.789Z");
    assert_eq!(at(4_107_542_399, 999), "2100-02-28T23:59:59.999Z");
    assert_eq!(at(4_107_542_400, 1), "2100-03-01T00:00:00.001Z");
    assert_eq!(at(13_574_563_200, 0), "2400-02-29T00:00:00.000Z");
    assert_eq!(
        format_rfc3339_utc(UNIX_EPOCH - Duration::from_secs(1)),
        "1970-01-01T00:00:00.000Z"
    );
}

fn env_source() -> SecretSource {
    SecretSource::Env {
        var: "MSW_TEST_SECRET".into(),
    }
}

#[tokio::test]
async fn load_secret_reads_the_variable_without_its_newline() {
    let lookup = |var: &str| (var == "MSW_TEST_SECRET").then(|| OsString::from("s3cret\n"));
    let loaded = load_secret(&env_source(), lookup).await.expect("loads");
    let header = signature(b"s3cret", b"body");
    assert_eq!(
        verify(&loaded, b"body", Some(header.as_bytes())),
        Verdict::Valid
    );
}

#[tokio::test]
async fn load_secret_refuses_an_unset_variable() {
    let result = load_secret(&env_source(), |_: &str| None).await;
    assert!(matches!(result, Err(SecretError::Unset { var }) if var == "MSW_TEST_SECRET"));
}

#[tokio::test]
async fn load_secret_refuses_an_empty_value() {
    for value in ["", "\n", "\r\n"] {
        let result = load_secret(&env_source(), |_: &str| Some(OsString::from(value))).await;
        assert!(
            matches!(result, Err(SecretError::Empty { .. })),
            "{value:?}"
        );
    }
}

#[test]
fn debug_output_hides_the_secret() {
    let fx = fixture();
    let text = format!("{:?}", fx.receiver);
    let secret = std::str::from_utf8(SECRET).expect("UTF-8");
    assert!(!text.contains(secret));
    assert!(!text.contains("secret to everybody"));
}
