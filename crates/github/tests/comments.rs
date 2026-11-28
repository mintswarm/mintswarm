//! Note and keyed-comment creates against a scripted local server answering with hand-written
//! fixtures in the shape of GitHub's issue comments API: the probe after an ambiguous attempt,
//! whatever the comment's mark, and the content limiter's budget.
// Not redundant: clippy's `allow-unwrap-in-tests` covers a `tests/` crate only under `cfg(test)`.
#![cfg(test)]

mod support;

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use mintswarm_domain::ids::IssueNumber;
use mintswarm_domain::tracker::{Comment, Login};
use mintswarm_github::{
    Clock, CommentClient, CommentError, ContentLimiter, Exhausted, Failure, NOTE_MARKER,
    PROBE_MARGIN, Permissions, RetryPolicy, STEER_MARKER, Token,
};
use support::{Request, Responder};

const TOKEN: &str = "ghs_c0mmentT0kenV4lue";
const CREATED: &str = include_str!("fixtures/comments/created.json");
const SINCE: &str = include_str!("fixtures/comments/since.json");
const CREATED_RECORD: &str = include_str!("fixtures/comments/created-record.json");
const SINCE_RECORD: &str = include_str!("fixtures/comments/since-record.json");
/// The answer line that keys a decision record answering the command comment 8800.
const ANSWER_KEY: &str = "<!-- mintswarm: answer 8800 -->";
const BOT: &str = "mintswarm-dev[bot]";
const KEY: &str = "<!-- mintswarm: blockers open=12 unreadable= -->";
/// 2026-11-26T12:00:00Z.
const NOW: u64 = 1_795_694_400;
const COMMENTS_PATH: &str = "/repos/acme/widgets/issues/7/comments";
/// The probe's first page: comments since [`NOW`] less [`PROBE_MARGIN`].
const PROBE_PATH: &str =
    "/repos/acme/widgets/issues/7/comments?since=2026-11-26T11%3A59%3A50Z&per_page=100&page=1";

fn token() -> Token {
    Token::new(TOKEN.to_owned(), 0)
}

fn note() -> String {
    format!("{NOTE_MARKER}\n{KEY}\nBlocked by #12.")
}

/// A plan approval record answering comment 8800: it carries the steer marker, not the note
/// marker, as the fixtures' records do.
fn record() -> String {
    format!(
        "<!-- mintswarm: plan approved -->\n{STEER_MARKER}\n{ANSWER_KEY}\n\n\
         **Plan approved** by `zmij` at `2026-11-26T11:59:40Z`."
    )
}

fn record_comment(id: u64, created_at: &str) -> Comment {
    Comment {
        body: record(),
        ..comment(id, created_at)
    }
}

/// A clock the test moves by hand, starting at [`NOW`].
fn clock() -> (Clock, Arc<AtomicU64>) {
    let now = Arc::new(AtomicU64::new(NOW));
    let read = Arc::clone(&now);
    (Clock::from_fn(move || read.load(Ordering::SeqCst)), now)
}

/// Two attempts with millisecond waits, so an ambiguous first attempt is probed and retried.
fn two_attempts() -> RetryPolicy {
    RetryPolicy {
        attempts: 2,
        base_delay: Duration::from_millis(1),
        max_delay: Duration::from_millis(1),
        ..RetryPolicy::default()
    }
}

fn client(responder: &Responder, policy: RetryPolicy) -> (CommentClient, Arc<ContentLimiter>) {
    let (clock, _) = clock();
    client_on(responder, policy, clock)
}

fn client_on(
    responder: &Responder,
    policy: RetryPolicy,
    clock: Clock,
) -> (CommentClient, Arc<ContentLimiter>) {
    let limiter = Arc::new(ContentLimiter::new(clock.clone()));
    let client = CommentClient::new(
        &responder.base,
        "acme",
        "widgets",
        Login(BOT.to_owned()),
        Arc::clone(&limiter),
        clock,
    )
    .unwrap()
    .with_retry(policy);
    (client, limiter)
}

fn posts(requests: &[Request]) -> usize {
    requests
        .iter()
        .filter(|request| request.method == "POST")
        .count()
}

fn comment(id: u64, created_at: &str) -> Comment {
    Comment {
        id,
        author: Login(BOT.to_owned()),
        author_is_bot: true,
        body: note(),
        created_at: created_at.to_owned(),
        updated_at: created_at.to_owned(),
        url: format!("https://github.com/acme/widgets/issues/7#issuecomment-{id}"),
    }
}

#[test]
fn a_post_token_asks_for_issues_write_and_metadata_read() {
    assert_eq!(
        CommentClient::permissions(),
        Permissions::write(["issues"]).and(Permissions::read(["metadata"]))
    );
}

#[tokio::test]
async fn a_note_is_posted_once() {
    let responder = Responder::start().await;
    responder.reply(201, CREATED);
    let (client, _) = client(&responder, two_attempts());
    let posted = client
        .post_note(&token(), IssueNumber(7), KEY, &note())
        .await;
    assert_eq!(posted, Ok(comment(9001, "2026-11-26T12:00:02Z")));
    let requests = responder.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].method, "POST");
    assert_eq!(requests[0].path, COMMENTS_PATH);
    let body: serde_json::Value = serde_json::from_str(&requests[0].body).unwrap();
    assert_eq!(body, serde_json::json!({ "body": note() }));
}

#[tokio::test]
async fn an_ambiguous_create_is_answered_by_the_probe() {
    let responder = Responder::start().await;
    responder.drop_connection();
    responder.reply(200, SINCE);
    let (client, _) = client(&responder, two_attempts());
    let posted = client
        .post_note(&token(), IssueNumber(7), KEY, &note())
        .await;
    // The newest bot note with the key since the call: not the older one, not someone else's.
    assert_eq!(posted, Ok(comment(9002, "2026-11-26T12:00:01Z")));
    let requests = responder.requests();
    assert_eq!(posts(&requests), 1);
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[1].method, "GET");
    assert_eq!(requests[1].path, PROBE_PATH);
    assert_eq!(requests[1].header("if-none-match"), None);
    assert_eq!(PROBE_MARGIN, Duration::from_secs(10));
}

#[tokio::test]
async fn a_probe_that_finds_nothing_sends_again() {
    let responder = Responder::start().await;
    responder.drop_connection();
    responder.reply(200, "[]");
    responder.reply(201, CREATED);
    let (client, _) = client(&responder, two_attempts());
    let posted = client
        .post_note(&token(), IssueNumber(7), KEY, &note())
        .await;
    assert_eq!(posted, Ok(comment(9001, "2026-11-26T12:00:02Z")));
    let requests = responder.requests();
    assert_eq!(posts(&requests), 2);
    assert_eq!(requests.len(), 3);
}

#[tokio::test]
async fn a_failed_probe_ends_the_call() {
    let responder = Responder::start().await;
    responder.drop_connection();
    responder.reply(404, r#"{"message":"Not Found"}"#);
    let (client, _) = client(&responder, two_attempts());
    let posted = client
        .post_note(&token(), IssueNumber(7), KEY, &note())
        .await;
    assert!(matches!(
        posted,
        Err(CommentError::Exhausted(Exhausted { attempts: 1, .. }))
    ));
    assert_eq!(posts(&responder.requests()), 1);
}

#[tokio::test]
async fn a_refused_create_is_its_status() {
    let responder = Responder::start().await;
    responder.reply(404, r#"{"message":"Not Found"}"#);
    responder.reply(422, r#"{"message":"Validation Failed"}"#);
    let (client, _) = client(&responder, two_attempts());
    assert_eq!(
        client
            .post_note(&token(), IssueNumber(7), KEY, &note())
            .await,
        Err(CommentError::NotFound {
            endpoint: format!("POST {COMMENTS_PATH}")
        })
    );
    assert_eq!(
        client
            .post_note(&token(), IssueNumber(7), KEY, &note())
            .await,
        Err(CommentError::Status {
            endpoint: format!("POST {COMMENTS_PATH}"),
            status: 422,
        })
    );
}

#[tokio::test]
async fn a_rate_limited_create_pauses_the_limiter() {
    let responder = Responder::start().await;
    responder.reply_with(429, &[("retry-after", "30")], "{}");
    let (client, limiter) = client(
        &responder,
        RetryPolicy {
            attempts: 1,
            ..RetryPolicy::default()
        },
    );
    let posted = client
        .post_note(&token(), IssueNumber(7), KEY, &note())
        .await;
    assert!(matches!(
        posted,
        Err(CommentError::Exhausted(Exhausted {
            last: Failure::RateLimited {
                wait: Some(wait)
            },
            ..
        })) if wait == Duration::from_secs(30)
    ));
    assert_eq!(limiter.try_acquire(), Err(Duration::from_secs(30)));
}

#[tokio::test]
async fn an_unmarked_body_is_refused_before_any_request() {
    let responder = Responder::start().await;
    let (client, _) = client(&responder, two_attempts());
    for (key, body) in [
        (KEY, format!("{KEY}\nNo marker.")),
        (KEY, format!("{NOTE_MARKER}\nNo key.")),
        (KEY, format!("{NOTE_MARKER} {KEY}\nBoth on one line.")),
        ("", format!("{NOTE_MARKER}\n\nAn empty key.")),
        (
            NOTE_MARKER,
            format!("{NOTE_MARKER}\nThe note marker as the key."),
        ),
        (
            STEER_MARKER,
            format!("{NOTE_MARKER}\n{STEER_MARKER}\nThe steer marker as the key."),
        ),
    ] {
        assert_eq!(
            client.post_note(&token(), IssueNumber(7), key, &body).await,
            Err(CommentError::Unmarked)
        );
    }
    assert!(responder.requests().is_empty());
}

#[tokio::test]
async fn a_refused_create_waits_for_the_limiter() {
    let responder = Responder::start().await;
    responder.reply(201, CREATED);
    let (clock, now) = clock();
    let (client, limiter) = client_on(&responder, two_attempts(), clock);
    while limiter.try_acquire().is_ok() {}
    let client = Arc::new(client);
    let task = tokio::spawn({
        let client = Arc::clone(&client);
        async move {
            client
                .post_note(&token(), IssueNumber(7), KEY, &note())
                .await
        }
    });
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(responder.requests().is_empty());
    assert!(!task.is_finished());

    now.store(NOW + 60, Ordering::SeqCst);
    let posted = tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .expect("the post never got the limiter's grant")
        .unwrap();
    assert_eq!(posted, Ok(comment(9001, "2026-11-26T12:00:02Z")));
    assert_eq!(posts(&responder.requests()), 1);
}

#[tokio::test]
async fn a_create_sent_again_waits_for_a_second_grant() {
    let responder = Responder::start().await;
    responder.drop_connection();
    responder.reply(200, "[]");
    responder.reply(201, CREATED);
    let (clock, now) = clock();
    let (client, limiter) = client_on(&responder, two_attempts(), clock);
    while limiter.try_acquire().is_ok() {}
    // A second's refill is one grant a minute's bucket holds, and less than two.
    now.store(NOW + 1, Ordering::SeqCst);
    let client = Arc::new(client);
    let task = tokio::spawn({
        let client = Arc::clone(&client);
        async move {
            client
                .post_note(&token(), IssueNumber(7), KEY, &note())
                .await
        }
    });
    tokio::time::sleep(Duration::from_millis(300)).await;
    // The first attempt and its probe went out; the second attempt waits for its grant.
    let requests = responder.requests();
    assert_eq!(posts(&requests), 1);
    assert_eq!(requests.len(), 2);
    assert!(!task.is_finished());

    now.store(NOW + 2, Ordering::SeqCst);
    let posted = tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .expect("the second attempt never got the limiter's grant")
        .unwrap();
    assert_eq!(posted, Ok(comment(9001, "2026-11-26T12:00:02Z")));
    assert_eq!(posts(&responder.requests()), 2);
}

#[tokio::test]
async fn a_steer_record_is_posted_once() {
    let responder = Responder::start().await;
    responder.reply(201, CREATED_RECORD);
    let (client, _) = client(&responder, two_attempts());
    let posted = client
        .post_keyed(&token(), IssueNumber(7), ANSWER_KEY, &record())
        .await;
    assert_eq!(posted, Ok(record_comment(9101, "2026-11-26T12:00:02Z")));
    let requests = responder.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].method, "POST");
    assert_eq!(requests[0].path, COMMENTS_PATH);
    let body: serde_json::Value = serde_json::from_str(&requests[0].body).unwrap();
    assert_eq!(body, serde_json::json!({ "body": record() }));
}

#[tokio::test]
async fn an_ambiguous_record_create_is_answered_by_the_probe() {
    let responder = Responder::start().await;
    responder.drop_connection();
    responder.reply(200, SINCE_RECORD);
    let (client, _) = client(&responder, two_attempts());
    let posted = client
        .post_keyed(&token(), IssueNumber(7), ANSWER_KEY, &record())
        .await;
    // The landed record, steer-marked: not the older one, not someone else's copy.
    assert_eq!(posted, Ok(record_comment(9102, "2026-11-26T12:00:01Z")));
    let requests = responder.requests();
    assert_eq!(posts(&requests), 1);
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[1].method, "GET");
    assert_eq!(requests[1].path, PROBE_PATH);
}

#[tokio::test]
async fn a_probe_ignores_other_authors_and_older_records() {
    let responder = Responder::start().await;
    responder.drop_connection();
    // The fixture less its landed record: an older own record and someone else's copy.
    let mut items: Vec<serde_json::Value> = serde_json::from_str(SINCE_RECORD).unwrap();
    items.retain(|item| item["id"] != 9102);
    assert_eq!(items.len(), 2);
    responder.reply(200, serde_json::Value::Array(items).to_string());
    responder.reply(201, CREATED_RECORD);
    let (client, _) = client(&responder, two_attempts());
    let posted = client
        .post_keyed(&token(), IssueNumber(7), ANSWER_KEY, &record())
        .await;
    assert_eq!(posted, Ok(record_comment(9101, "2026-11-26T12:00:02Z")));
    let requests = responder.requests();
    assert_eq!(posts(&requests), 2);
    assert_eq!(requests.len(), 3);
}

#[tokio::test]
async fn a_key_that_is_no_marker_line_is_refused() {
    let responder = Responder::start().await;
    let (client, _) = client(&responder, two_attempts());
    for (key, body) in [
        ("", format!("{STEER_MARKER}\n\nAn empty key.")),
        ("answer 8800", "answer 8800\nNo marker line.".to_owned()),
        (
            NOTE_MARKER,
            format!("{NOTE_MARKER}\nThe note marker as the key."),
        ),
        (
            STEER_MARKER,
            format!("{STEER_MARKER}\nThe steer marker as the key."),
        ),
        (ANSWER_KEY, format!("{STEER_MARKER}\nNo key line.")),
    ] {
        assert_eq!(
            client
                .post_keyed(&token(), IssueNumber(7), key, &body)
                .await,
            Err(CommentError::Unmarked),
            "{key:?}"
        );
    }
    assert!(responder.requests().is_empty());
}
