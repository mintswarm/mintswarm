//! The merge call against a scripted local server answering with replies recorded in the shape
//! of GitHub's REST API, from `fixtures/pulls/pr-400.json`.
// Not redundant: clippy's `allow-unwrap-in-tests` covers a `tests/` crate only under `cfg(test)`.
#![cfg(test)]

mod support;

use std::time::Duration;

use mintswarm_domain::ids::{PrNumber, RevId};
use mintswarm_domain::tracker::Merged;
use mintswarm_github::{Failure, MergeClient, MergeError, Permissions, RetryPolicy, Token};
use serde_json::Value;
use support::Responder;

const TOKEN: &str = "ghs_m3rgeT0kenV4lue";
const FIXTURE: &str = include_str!("fixtures/pulls/pr-400.json");
const PR: PrNumber = PrNumber(400);
const HEAD: &str = "0123456789abcdef0123456789abcdef01234567";
const MERGE_PATH: &str = "/repos/acme/widgets/pulls/400/merge";
const TITLE: &str = "feat(domain): GitHub REST reads (#400)";

fn token() -> Token {
    Token::new(TOKEN.to_owned(), 0)
}

fn head() -> RevId {
    RevId(HEAD.to_owned())
}

/// The recorded reply under `key`, as JSON text.
fn recorded(key: &str) -> String {
    let fixture: Value = serde_json::from_str(FIXTURE).unwrap();
    fixture
        .get(key)
        .unwrap_or_else(|| panic!("no recorded reply {key}"))
        .to_string()
}

/// A client with four attempts and millisecond waits, so a retry would show as a second
/// request.
fn client(responder: &Responder) -> MergeClient {
    MergeClient::new(&responder.base, "acme", "widgets")
        .unwrap()
        .with_retry(RetryPolicy {
            base_delay: Duration::from_millis(1),
            max_delay: Duration::from_millis(1),
            ..RetryPolicy::default()
        })
}

#[test]
fn merge_permissions_are_exact() {
    let expected =
        Permissions::write(["contents", "pull_requests"]).and(Permissions::read(["metadata"]));
    assert_eq!(Permissions::merge(), expected);
    assert_eq!(MergeClient::permissions(), expected);
}

#[tokio::test]
async fn merge_sends_squash_sha_and_title() {
    let responder = Responder::start().await;
    responder.reply(200, recorded("merge_ok"));
    let merged = client(&responder)
        .merge(&token(), PR, &head(), TITLE)
        .await
        .unwrap();
    assert_eq!(
        merged,
        Merged {
            sha: RevId("6dcb09b5b57875f334f61aebed695e2e4193db5e".into()),
        }
    );
    let requests = responder.requests();
    assert_eq!(requests.len(), 1);
    let request = &requests[0];
    assert_eq!(request.method, "PUT");
    assert_eq!(request.path, MERGE_PATH);
    let body: Value = serde_json::from_str(&request.body).unwrap();
    assert_eq!(
        body,
        serde_json::json!({
            "merge_method": "squash",
            "sha": HEAD,
            "commit_title": TITLE,
        })
    );
    assert_eq!(
        request.header("authorization"),
        Some(format!("Bearer {TOKEN}").as_str())
    );
    assert!(!request.path.contains(TOKEN));
    assert!(!request.body.contains(TOKEN));
    for (name, value) in &request.headers {
        assert!(name == "authorization" || !value.contains(TOKEN), "{name}");
    }
}

#[tokio::test]
async fn refusals_are_typed() {
    for (status, key) in [(409, "merge_head_moved"), (405, "merge_not_mergeable")] {
        let responder = Responder::start().await;
        responder.reply(status, recorded(key));
        let error = client(&responder)
            .merge(&token(), PR, &head(), TITLE)
            .await
            .unwrap_err();
        match status {
            409 => assert!(matches!(error, MergeError::HeadMoved { .. }), "{error:?}"),
            _ => assert!(
                matches!(error, MergeError::NotMergeable { .. }),
                "{error:?}"
            ),
        }
        assert!(!error.may_have_landed(), "{error:?}");
        // One attempt's refusal: it is never sent again.
        assert_eq!(responder.requests().len(), 1);
    }
}

#[tokio::test]
async fn other_statuses_keep_their_class() {
    for (status, body) in [
        (404, r#"{"message":"Not Found","status":"404"}"#),
        (422, r#"{"message":"Validation Failed","status":"422"}"#),
    ] {
        let responder = Responder::start().await;
        responder.reply(status, body);
        let error = client(&responder)
            .merge(&token(), PR, &head(), TITLE)
            .await
            .unwrap_err();
        match status {
            404 => assert!(matches!(error, MergeError::NotFound { .. }), "{error:?}"),
            _ => assert!(
                matches!(error, MergeError::Status { status: 422, .. }),
                "{error:?}"
            ),
        }
    }
}

#[tokio::test]
async fn a_success_without_a_merge_is_unexpected() {
    let responder = Responder::start().await;
    responder.reply(200, r#"{"merged":false,"sha":null}"#);
    let error = client(&responder)
        .merge(&token(), PR, &head(), TITLE)
        .await
        .unwrap_err();
    assert!(matches!(error, MergeError::Unexpected { .. }), "{error:?}");
}

#[tokio::test]
async fn a_possibly_sent_attempt_is_not_repeated() {
    let responder = Responder::start().await;
    responder.reply(502, r#"{"message":"Server Error"}"#);
    responder.reply(200, recorded("merge_ok"));
    let error = client(&responder)
        .merge(&token(), PR, &head(), TITLE)
        .await
        .unwrap_err();
    assert!(
        matches!(
            &error,
            MergeError::Exhausted(exhausted)
                if exhausted.attempts == 1 && exhausted.last == Failure::Status(502)
        ),
        "{error:?}"
    );
    assert!(error.may_have_landed());
    assert_eq!(responder.requests().len(), 1);
}

#[tokio::test]
async fn a_rate_limited_attempt_is_sent_again() {
    let responder = Responder::start().await;
    responder.reply_with(
        429,
        &[("retry-after", "0")],
        r#"{"message":"API rate limit exceeded"}"#,
    );
    responder.reply(200, recorded("merge_ok"));
    client(&responder)
        .merge(&token(), PR, &head(), TITLE)
        .await
        .unwrap();
    assert_eq!(responder.requests().len(), 2);
}

#[tokio::test]
async fn rate_limits_to_the_end_never_landed() {
    let responder = Responder::start().await;
    for _ in 0..4 {
        responder.reply_with(
            429,
            &[("retry-after", "0")],
            r#"{"message":"API rate limit exceeded"}"#,
        );
    }
    let error = client(&responder)
        .merge(&token(), PR, &head(), TITLE)
        .await
        .unwrap_err();
    assert!(matches!(error, MergeError::Exhausted(_)), "{error:?}");
    assert!(!error.may_have_landed());
    assert_eq!(responder.requests().len(), 4);
}

#[tokio::test]
async fn a_sha_that_is_not_a_commit_id_is_refused_unsent() {
    let responder = Responder::start().await;
    for sha in ["master", "0123456", &HEAD.to_uppercase()] {
        let error = client(&responder)
            .merge(&token(), PR, &RevId(sha.to_owned()), TITLE)
            .await
            .unwrap_err();
        assert!(matches!(error, MergeError::InvalidSha { .. }), "{error:?}");
        assert!(!error.may_have_landed());
    }
    assert!(responder.requests().is_empty());
}
