//! Reactions, comment edits and artifact updates against a scripted local server answering with
//! fixtures trimmed from live recordings of the steward App changing `mintswarm-dev`'s comment
//! and artifact; only the refusal is hand-written, as none was recorded.
// Not redundant: clippy's `allow-unwrap-in-tests` covers a `tests/` crate only under `cfg(test)`.
#![cfg(test)]

mod support;

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use mintswarm_domain::ids::IssueNumber;
use mintswarm_domain::tracker::{Artifact, Comment, Login};
use mintswarm_github::{
    Clock, ContentLimiter, Exhausted, Failure, IssueWrites, Permissions, RetryPolicy, Token,
    WriteError,
};
use support::{Request, Responder};

const TOKEN: &str = "ghs_wr1tesT0kenV4lue";
const REACTION: &str = include_str!("fixtures/writes/reaction.json");
const COMMENT_EDITED: &str = include_str!("fixtures/writes/comment-edited.json");
const ARTIFACT_UPDATED: &str = include_str!("fixtures/writes/artifact-updated.json");
const REFUSED: &str = include_str!("fixtures/writes/refused.json");
const COMMENT_ID: u64 = 7_202_074_597;
const ISSUE: IssueNumber = IssueNumber(593);
const REACTIONS_PATH: &str = "/repos/mintswarm/mintswarm/issues/comments/7202074597/reactions";
const COMMENT_PATH: &str = "/repos/mintswarm/mintswarm/issues/comments/7202074597";
const ARTIFACT_PATH: &str = "/repos/mintswarm/mintswarm/issues/593/artifacts/1";
const EDITED: &str =
    "<!-- mintswarm: note -->\nScratch comment by mintswarm-dev, edited by the steward.";
const UPDATED: &str = "# Scratch artefact\n\nVersion 2, by the steward.";
/// 2026-11-26T12:00:00Z.
const NOW: u64 = 1_795_694_400;

fn token() -> Token {
    Token::new(TOKEN.to_owned(), 0)
}

/// A single attempt per write.
fn single_attempt() -> RetryPolicy {
    RetryPolicy {
        attempts: 1,
        ..RetryPolicy::default()
    }
}

/// Four attempts with millisecond waits, for the retries on a real socket.
fn fast_retries() -> RetryPolicy {
    RetryPolicy {
        base_delay: Duration::from_millis(1),
        max_delay: Duration::from_millis(1),
        ..RetryPolicy::default()
    }
}

/// A clock the test moves by hand, starting at [`NOW`].
fn clock() -> (Clock, Arc<AtomicU64>) {
    let now = Arc::new(AtomicU64::new(NOW));
    let read = Arc::clone(&now);
    (Clock::from_fn(move || read.load(Ordering::SeqCst)), now)
}

fn client(responder: &Responder, policy: RetryPolicy) -> (IssueWrites, Arc<ContentLimiter>) {
    let (clock, _) = clock();
    client_on(responder, policy, clock)
}

fn client_on(
    responder: &Responder,
    policy: RetryPolicy,
    clock: Clock,
) -> (IssueWrites, Arc<ContentLimiter>) {
    let limiter = Arc::new(ContentLimiter::new(clock));
    let client = IssueWrites::new(
        &responder.base,
        "mintswarm",
        "mintswarm",
        Arc::clone(&limiter),
    )
    .unwrap()
    .with_retry(policy);
    (client, limiter)
}

fn body(request: &Request) -> serde_json::Value {
    serde_json::from_str(&request.body).unwrap()
}

fn edited_comment() -> Comment {
    Comment {
        id: COMMENT_ID,
        author: Login("mintswarm-dev[bot]".to_owned()),
        author_is_bot: true,
        body: EDITED.to_owned(),
        created_at: "2026-11-26T19:53:51Z".to_owned(),
        updated_at: "2026-11-26T19:54:10Z".to_owned(),
        url: "https://github.com/mintswarm/mintswarm/issues/593#issuecomment-7202074597".to_owned(),
    }
}

fn updated_artifact() -> Artifact {
    Artifact {
        name: "Scratch artefact".to_owned(),
        body: UPDATED.to_owned(),
    }
}

#[test]
fn a_write_token_asks_for_issues_write_and_metadata_read() {
    assert_eq!(
        IssueWrites::permissions(),
        Permissions::write(["issues"]).and(Permissions::read(["metadata"]))
    );
}

#[tokio::test]
async fn react_plus_one_succeeds_on_a_new_and_an_existing_reaction() {
    let responder = Responder::start().await;
    // A new reaction, then GitHub's answer to the same POST again: the reaction it holds.
    responder.reply(201, REACTION);
    responder.reply(200, REACTION);
    let (client, _) = client(&responder, fast_retries());
    assert_eq!(client.react_plus_one(&token(), COMMENT_ID).await, Ok(()));
    assert_eq!(responder.requests().len(), 1);
    assert_eq!(client.react_plus_one(&token(), COMMENT_ID).await, Ok(()));
    let requests = responder.requests();
    assert_eq!(requests.len(), 2);
    for request in &requests {
        assert_eq!(request.method, "POST");
        assert_eq!(request.path, REACTIONS_PATH);
        assert_eq!(body(request), serde_json::json!({ "content": "+1" }));
        assert_eq!(
            request.header("authorization"),
            Some(format!("Bearer {TOKEN}").as_str())
        );
    }
}

#[tokio::test]
async fn edit_comment_as_another_app_returns_the_edited_comment() {
    let responder = Responder::start().await;
    responder.reply(200, COMMENT_EDITED);
    let (client, _) = client(&responder, single_attempt());
    // The comment is mintswarm-dev's; the recording's token was the steward's.
    assert_eq!(
        client.edit_comment(&token(), COMMENT_ID, EDITED).await,
        Ok(edited_comment())
    );
    let requests = responder.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].method, "PATCH");
    assert_eq!(requests[0].path, COMMENT_PATH);
    assert_eq!(body(&requests[0]), serde_json::json!({ "body": EDITED }));
}

#[tokio::test]
async fn update_artifact_as_another_app_returns_the_new_body() {
    let responder = Responder::start().await;
    responder.reply(200, ARTIFACT_UPDATED);
    let (client, _) = client(&responder, single_attempt());
    assert_eq!(
        client.update_artifact(&token(), ISSUE, 1, UPDATED).await,
        Ok(updated_artifact())
    );
    let requests = responder.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].method, "PATCH");
    assert_eq!(requests[0].path, ARTIFACT_PATH);
    assert_eq!(body(&requests[0]), serde_json::json!({ "body": UPDATED }));
}

#[tokio::test]
async fn a_refused_edit_or_update_is_refused() {
    let responder = Responder::start().await;
    for _ in 0..3 {
        responder.reply(403, REFUSED);
    }
    let (client, _) = client(&responder, fast_retries());
    let refused = |path: &str, method: &str| {
        Err::<(), _>(WriteError::Refused {
            endpoint: format!("{method} {path}"),
        })
    };
    assert_eq!(
        client.react_plus_one(&token(), COMMENT_ID).await,
        refused(REACTIONS_PATH, "POST")
    );
    assert_eq!(responder.requests().len(), 1);
    assert_eq!(
        client
            .edit_comment(&token(), COMMENT_ID, EDITED)
            .await
            .map(drop),
        refused(COMMENT_PATH, "PATCH")
    );
    assert_eq!(responder.requests().len(), 2);
    assert_eq!(
        client
            .update_artifact(&token(), ISSUE, 1, UPDATED)
            .await
            .map(drop),
        refused(ARTIFACT_PATH, "PATCH")
    );
    assert_eq!(responder.requests().len(), 3);
}

#[tokio::test]
async fn other_failures_are_classed_by_status() {
    let responder = Responder::start().await;
    responder.reply(404, r#"{"message":"Not Found"}"#);
    responder.reply(422, r#"{"message":"Validation Failed"}"#);
    responder.reply(200, "not json");
    let (client, _) = client(&responder, single_attempt());
    assert_eq!(
        client.edit_comment(&token(), COMMENT_ID, EDITED).await,
        Err(WriteError::NotFound {
            endpoint: format!("PATCH {COMMENT_PATH}")
        })
    );
    assert_eq!(
        client.update_artifact(&token(), ISSUE, 1, UPDATED).await,
        Err(WriteError::Status {
            endpoint: format!("PATCH {ARTIFACT_PATH}"),
            status: 422,
        })
    );
    assert_eq!(
        client.update_artifact(&token(), ISSUE, 1, UPDATED).await,
        Err(WriteError::Unexpected {
            endpoint: format!("PATCH {ARTIFACT_PATH}")
        })
    );
}

#[tokio::test]
async fn a_rate_limited_403_is_not_a_refusal() {
    let responder = Responder::start().await;
    let reset = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
        + 30;
    responder.reply_with(
        403,
        &[
            ("x-ratelimit-remaining", "0"),
            ("x-ratelimit-reset", &reset.to_string()),
        ],
        REFUSED,
    );
    let (client, limiter) = client(&responder, single_attempt());
    let error = client
        .edit_comment(&token(), COMMENT_ID, EDITED)
        .await
        .unwrap_err();
    assert!(
        matches!(
            error,
            WriteError::Exhausted(Exhausted {
                last: Failure::RateLimited { wait: Some(_) },
                ..
            })
        ),
        "{error:?}"
    );
    // The named wait pauses the limiter too.
    assert!(limiter.try_acquire().is_err());
}

#[tokio::test]
async fn every_write_is_retried_and_takes_a_grant_per_attempt() {
    for write in ["react", "edit", "update"] {
        let responder = Responder::start().await;
        responder.reply(502, "");
        responder.reply(
            200,
            match write {
                "react" => REACTION,
                "edit" => COMMENT_EDITED,
                _ => ARTIFACT_UPDATED,
            },
        );
        let (clock, now) = clock();
        let (client, limiter) = client_on(&responder, fast_retries(), clock);
        while limiter.try_acquire().is_ok() {}
        // Two seconds refill a minute's bucket by two grants and less than three.
        now.store(NOW + 2, Ordering::SeqCst);
        let written = match write {
            "react" => client.react_plus_one(&token(), COMMENT_ID).await,
            "edit" => client
                .edit_comment(&token(), COMMENT_ID, EDITED)
                .await
                .map(drop),
            _ => client
                .update_artifact(&token(), ISSUE, 1, UPDATED)
                .await
                .map(drop),
        };
        assert_eq!(written, Ok(()), "{write}");
        let requests = responder.requests();
        assert_eq!(requests.len(), 2, "{write}");
        assert_eq!(requests[0].path, requests[1].path);
        // Both grants went to the two attempts, so a third request waits.
        assert!(limiter.try_acquire().is_err(), "{write}");
    }
}

#[tokio::test]
async fn a_write_waits_for_the_limiter() {
    let responder = Responder::start().await;
    responder.reply(200, COMMENT_EDITED);
    let (clock, now) = clock();
    let (client, limiter) = client_on(&responder, single_attempt(), clock);
    while limiter.try_acquire().is_ok() {}
    let client = Arc::new(client);
    let task = tokio::spawn({
        let client = Arc::clone(&client);
        async move { client.edit_comment(&token(), COMMENT_ID, EDITED).await }
    });
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(responder.requests().is_empty());
    assert!(!task.is_finished());

    now.store(NOW + 60, Ordering::SeqCst);
    let edited = tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .expect("the edit never got the limiter's grant")
        .unwrap();
    assert_eq!(edited, Ok(edited_comment()));
}
