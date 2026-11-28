//! The GET-only reads against a scripted local server answering with hand-written fixtures in
//! the shape of GitHub's REST API.
// Not redundant: clippy's `allow-unwrap-in-tests` covers a `tests/` crate only under `cfg(test)`.
#![cfg(test)]

mod support;

use std::time::{Duration, Instant};

use mintswarm_domain::ids::{IssueNumber, PrNumber, RevId};
use mintswarm_domain::tracker::{
    ArtifactSummary, CheckState, Comment, CommentMark, Cursor, Dependency, Direction, IssueType,
    Link, LinkKind, Login, ParentIssue, RepoRole,
};
use mintswarm_domain::vcs::BranchName;
use mintswarm_github::{
    Exhausted, Failure, GitHubReads, IssueQuery, IssueState, Kinds, NOTE_MARKER, ReadError,
    RetryPolicy, STEER_MARKER, Token, comment_mark,
};
use support::{Request, Responder};

const TOKEN: &str = "ghs_r34dsT0kenV4lue";
const ISSUE_JSON: &str = include_str!("fixtures/reads/issue.json");
const ARTIFACTS: &str = include_str!("fixtures/reads/artifacts.json");
const ARTIFACT: &str = include_str!("fixtures/reads/artifact.json");
const PULL: &str = include_str!("fixtures/reads/pull.json");
const BLOCKED_BY: &str = include_str!("fixtures/reads/blocked-by.json");
const BLOCKING: &str = include_str!("fixtures/reads/blocking.json");
const ISSUES_LIST: &str = include_str!("fixtures/reads/issues-list.json");
const PERMISSION: &str = include_str!("fixtures/reads/permission.json");
const PERMISSION_NONE: &str = include_str!("fixtures/reads/permission-none.json");
const PARENT_NONE: &str = include_str!("fixtures/reads/parent-none.json");
const PERMISSION_PATH: &str = "/repos/acme/widgets/collaborators/zmij/permission";
const ISSUE: IssueNumber = IssueNumber(180);
const ISSUE_PATH: &str = "/repos/acme/widgets/issues/180";
const BOT: &str = "mintswarm-dev-bee[bot]";

fn token() -> Token {
    Token::new(TOKEN.to_owned(), 0)
}

fn kinds() -> Kinds {
    Kinds::issue_types([
        (IssueType::Design, "Design".to_owned()),
        (IssueType::Plan, "Plan".to_owned()),
        (IssueType::Implementation, "Task".to_owned()),
    ])
}

fn client(responder: &Responder) -> GitHubReads {
    client_with(responder, kinds())
}

/// A single attempt per read, so each read takes one scripted reply.
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

fn client_with(responder: &Responder, kinds: Kinds) -> GitHubReads {
    GitHubReads::new(
        &responder.base,
        "acme",
        "widgets",
        kinds,
        vec![Login("Maintainer".into())],
        Login(BOT.into()),
    )
    .unwrap()
    .with_retry(single_attempt())
}

/// A comment in GitHub's JSON shape.
fn comment_json(id: u64, login: &str, body: &str, updated_at: &str) -> serde_json::Value {
    serde_json::json!({
        "id": id,
        "user": { "login": login, "type": "User" },
        "body": body,
        "created_at": "2026-11-21T08:00:00Z",
        "updated_at": updated_at,
        "html_url": format!("https://github.com/acme/widgets/issues/180#issuecomment-{id}"),
    })
}

/// `count` comments with ids from `first`, as one page's JSON.
fn comments_page(first: u64, count: u64) -> String {
    let page: Vec<_> = (first..first + count)
        .map(|id| comment_json(id, "someone", "text", "2026-11-22T08:00:00Z"))
        .collect();
    serde_json::Value::from(page).to_string()
}

fn comment(author: &str, body: &str) -> Comment {
    Comment {
        id: 1,
        author: Login(author.into()),
        author_is_bot: false,
        body: body.into(),
        created_at: String::new(),
        updated_at: String::new(),
        url: String::new(),
    }
}

/// No request shows the token outside its `authorization` header.
fn assert_token_only_in_authorization(requests: &[Request]) {
    for request in requests {
        assert_eq!(
            request.header("authorization"),
            Some(format!("Bearer {TOKEN}").as_str())
        );
        assert!(!request.path.contains(TOKEN));
        for (name, value) in &request.headers {
            assert!(name == "authorization" || !value.contains(TOKEN), "{name}");
        }
    }
}

fn paths(requests: &[Request]) -> Vec<String> {
    requests
        .iter()
        .map(|request| request.path.clone())
        .collect()
}

#[tokio::test]
async fn a_101_item_list_is_read_in_two_pages() {
    let responder = Responder::start().await;
    responder.reply(200, ISSUE_JSON);
    responder.reply(200, comments_page(1, 100));
    responder.reply(200, comments_page(101, 1));
    let read = client(&responder)
        .read_issue(&token(), ISSUE, None)
        .await
        .unwrap();
    assert_eq!(read.comments.len(), 101);
    assert_eq!(read.comments[100].id, 101);
    let requests = responder.requests();
    assert_eq!(
        paths(&requests),
        [
            ISSUE_PATH.to_owned(),
            format!("{ISSUE_PATH}/comments?per_page=100&page=1"),
            format!("{ISSUE_PATH}/comments?per_page=100&page=2"),
        ]
    );
    assert!(requests.iter().all(|request| request.method == "GET"));
    assert_token_only_in_authorization(&requests);
}

#[tokio::test]
async fn a_list_needing_more_than_30_pages_is_refused() {
    let responder = Responder::start().await;
    responder.reply(200, ISSUE_JSON);
    for page in 0..30 {
        responder.reply(200, comments_page(page * 100 + 1, 100));
    }
    let error = client(&responder)
        .read_issue(&token(), ISSUE, None)
        .await
        .unwrap_err();
    assert!(matches!(error, ReadError::TooManyPages { .. }), "{error:?}");
    assert!(!error.to_string().contains(TOKEN));
    let requests = responder.requests();
    // The issue, then pages 1 to 30; page 31 is never asked for.
    assert_eq!(requests.len(), 31);
    assert!(requests[30].path.ends_with("&page=30"));
    assert!(
        !requests
            .iter()
            .any(|request| request.path.contains("page=31"))
    );
}

/// A `Link` header for comments page `page`, with a `next` link when `next` is set.
fn comments_link(page: u64, next: bool) -> String {
    let url = "https://api.github.com/repositories/1/issues/180/comments?per_page=100";
    let mut link = format!("<{url}&page=1>; rel=\"first\"");
    if page > 1 {
        link.push_str(&format!(", <{url}&page={}>; rel=\"prev\"", page - 1));
    }
    if next {
        link.push_str(&format!(", <{url}&page={}>; rel=\"next\"", page + 1));
    }
    link
}

#[tokio::test]
async fn a_list_of_exactly_30_full_pages_is_read_by_its_link_header() {
    let responder = Responder::start().await;
    responder.reply(200, ISSUE_JSON);
    for page in 1..=30 {
        let link = comments_link(page, page < 30);
        responder.reply_with(
            200,
            &[("link", link.as_str())],
            comments_page((page - 1) * 100 + 1, 100),
        );
    }
    let read = client(&responder)
        .read_issue(&token(), ISSUE, None)
        .await
        .unwrap();
    assert_eq!(read.comments.len(), 3000);
    let requests = responder.requests();
    // The issue, then pages 1 to 30; the last page has no next link, so page 31 is not asked.
    assert_eq!(requests.len(), 31);
    assert!(
        !requests
            .iter()
            .any(|request| request.path.contains("page=31"))
    );
}

#[tokio::test]
async fn a_list_linking_past_page_30_is_refused() {
    let responder = Responder::start().await;
    responder.reply(200, ISSUE_JSON);
    for page in 1..=30 {
        let link = comments_link(page, true);
        responder.reply_with(
            200,
            &[("link", link.as_str())],
            comments_page((page - 1) * 100 + 1, 100),
        );
    }
    let error = client(&responder)
        .read_issue(&token(), ISSUE, None)
        .await
        .unwrap_err();
    assert!(matches!(error, ReadError::TooManyPages { .. }), "{error:?}");
    let requests = responder.requests();
    assert_eq!(requests.len(), 31);
    assert!(
        !requests
            .iter()
            .any(|request| request.path.contains("page=31"))
    );
}

#[tokio::test]
async fn a_full_page_without_a_next_link_is_the_last() {
    let responder = Responder::start().await;
    responder.reply(200, ISSUE_JSON);
    let link = comments_link(1, false);
    responder.reply_with(200, &[("link", link.as_str())], comments_page(1, 100));
    let read = client(&responder)
        .read_issue(&token(), ISSUE, None)
        .await
        .unwrap();
    assert_eq!(read.comments.len(), 100);
    // No extra request for an empty second page.
    assert_eq!(responder.requests().len(), 2);
}

#[tokio::test]
async fn a_repeated_get_revalidates_and_a_304_serves_the_cache() {
    let responder = Responder::start().await;
    let comments = serde_json::json!([comment_json(5, "someone", "hi", "2026-11-22T08:00:00Z")]);
    responder.reply_with(200, &[("etag", "\"issue-v1\"")], ISSUE_JSON);
    responder.reply_with(200, &[("etag", "W/\"comments-v1\"")], comments.to_string());
    responder.reply(304, "");
    responder.reply(304, "");
    let client = client(&responder);
    let first = client.read_issue(&token(), ISSUE, None).await.unwrap();
    let second = client.read_issue(&token(), ISSUE, None).await.unwrap();
    assert_eq!(first, second);
    assert_eq!(second.comments.len(), 1);
    let requests = responder.requests();
    assert_eq!(requests.len(), 4);
    assert_eq!(requests[0].header("if-none-match"), None);
    assert_eq!(requests[1].header("if-none-match"), None);
    assert_eq!(requests[2].header("if-none-match"), Some("\"issue-v1\""));
    assert_eq!(
        requests[3].header("if-none-match"),
        Some("W/\"comments-v1\"")
    );
    assert_token_only_in_authorization(&requests);
}

#[tokio::test]
async fn a_304_with_nothing_cached_is_a_transport_error() {
    let responder = Responder::start().await;
    responder.reply(304, "");
    let error = client(&responder)
        .read_issue(&token(), ISSUE, None)
        .await
        .unwrap_err();
    assert!(matches!(error, ReadError::Transport(_)), "{error:?}");
}

#[tokio::test]
async fn retry_after_pauses_the_client() {
    for status in [429, 403] {
        let responder = Responder::start().await;
        responder.reply_with(
            status,
            &[("retry-after", "1")],
            r#"{"message":"You have exceeded a secondary rate limit"}"#,
        );
        responder.reply(200, ISSUE_JSON);
        responder.reply(200, "[]");
        let client = client(&responder);
        let started = Instant::now();
        let error = client.read_issue(&token(), ISSUE, None).await.unwrap_err();
        assert_eq!(
            error,
            ReadError::Exhausted(Exhausted {
                endpoint: format!("GET {ISSUE_PATH}"),
                attempts: 1,
                last: Failure::RateLimited {
                    wait: Some(Duration::from_secs(1))
                },
            }),
            "status {status}"
        );
        assert_eq!(responder.requests().len(), 1, "status {status}");
        client.read_issue(&token(), ISSUE, None).await.unwrap();
        // The pause starts at the first reply, after `started`, so the second read can only
        // have reached the responder once a second has passed since `started`.
        assert!(
            started.elapsed() >= Duration::from_secs(1),
            "status {status}: {:?}",
            started.elapsed()
        );
        assert_eq!(responder.requests().len(), 3, "status {status}");
    }
}

#[tokio::test]
async fn pause_remaining_follows_retry_after() {
    let responder = Responder::start().await;
    responder.reply_with(429, &[("retry-after", "120")], "{}");
    let client = client(&responder);
    assert_eq!(client.pause_remaining(), Duration::ZERO);
    let error = client.read_issue(&token(), ISSUE, None).await.unwrap_err();
    assert!(
        matches!(
            error,
            ReadError::Exhausted(Exhausted {
                attempts: 1,
                last: Failure::RateLimited { .. },
                ..
            })
        ),
        "{error:?}"
    );
    let remaining = client.pause_remaining();
    assert!(
        remaining > Duration::from_secs(119) && remaining <= Duration::from_secs(120),
        "{remaining:?}"
    );
}

#[tokio::test]
async fn other_failures_are_classified_without_the_token() {
    let responder = Responder::start().await;
    responder.reply(404, r#"{"message":"Not Found"}"#);
    responder.reply(502, "<html>Bad Gateway</html>");
    responder.reply(200, r#"{"number":"not a number"}"#);
    let client = client(&responder);
    let mut errors = Vec::new();
    for _ in 0..3 {
        errors.push(client.read_issue(&token(), ISSUE, None).await.unwrap_err());
    }
    let endpoint = format!("GET {ISSUE_PATH}");
    assert_eq!(
        errors,
        [
            ReadError::NotFound {
                endpoint: endpoint.clone()
            },
            ReadError::Exhausted(Exhausted {
                endpoint: endpoint.clone(),
                attempts: 1,
                last: Failure::Status(502),
            }),
            ReadError::Unexpected { endpoint },
        ]
    );
    for error in &errors {
        assert!(!error.to_string().contains(TOKEN));
        assert!(!format!("{error:?}").contains(TOKEN));
    }
    // A 502 is not a rate limit: no pause follows it.
    assert_eq!(client.pause_remaining(), Duration::ZERO);
}

#[tokio::test]
async fn a_dropped_connection_is_retried() {
    let responder = Responder::start().await;
    responder.drop_connection();
    responder.reply(200, ISSUE_JSON);
    responder.reply(200, "[]");
    let client = client(&responder).with_retry(fast_retries());
    let read = client.read_issue(&token(), ISSUE, None).await.unwrap();
    assert_eq!(read.issue.number, ISSUE);
    let requests = responder.requests();
    assert_eq!(requests.len(), 3);
    assert_eq!(requests[0].path, requests[1].path);
    assert_token_only_in_authorization(&requests);
}

#[tokio::test]
async fn a_server_error_then_success_reads() {
    let responder = Responder::start().await;
    responder.reply(500, "");
    responder.reply(503, "");
    responder.reply(200, ISSUE_JSON);
    responder.reply(200, "[]");
    let client = client(&responder).with_retry(fast_retries());
    let read = client.read_issue(&token(), ISSUE, None).await.unwrap();
    assert_eq!(read.issue.number, ISSUE);
    assert_eq!(responder.requests().len(), 4);
    // Server errors are no rate limit: no pause follows them.
    assert_eq!(client.pause_remaining(), Duration::ZERO);

    // Four server errors use up the four attempts.
    let responder = Responder::start().await;
    for _ in 0..4 {
        responder.reply(502, "");
    }
    let error = client_with(&responder, kinds())
        .with_retry(fast_retries())
        .read_issue(&token(), ISSUE, None)
        .await
        .unwrap_err();
    assert!(
        matches!(
            error,
            ReadError::Exhausted(Exhausted {
                attempts: 4,
                last: Failure::Status(502),
                ..
            })
        ),
        "{error:?}"
    );
    assert_eq!(responder.requests().len(), 4);
}

#[test]
fn steers_and_notes_are_classified() {
    let trusted = [Login("Maintainer".into())];
    let bot = Login(BOT.into());
    let mark = |author: &str, body: &str| comment_mark(&comment(author, body), &trusted, &bot);
    let steer = format!("Relaying the maintainer.\n{STEER_MARKER}\n");
    let note = format!("{NOTE_MARKER}\r\nThe session's record.");

    assert_eq!(
        mark("maintainer", "Please split it."),
        Some(CommentMark::Steer)
    );
    assert_eq!(mark("MAINTAINER", "any text"), Some(CommentMark::Steer));
    assert_eq!(mark(BOT, &steer), Some(CommentMark::Steer));
    assert_eq!(
        mark("MintSwarm-Dev-Bee[bot]", &format!("  {STEER_MARKER}  ")),
        Some(CommentMark::Steer)
    );
    assert_eq!(mark(BOT, &note), Some(CommentMark::Note));

    assert_eq!(mark(BOT, "A plain bot comment."), None);
    assert_eq!(mark(BOT, &format!("see {STEER_MARKER} here")), None);
    assert_eq!(mark("someone", &steer), None);
    assert_eq!(mark("someone", &note), None);
    assert_eq!(mark("other-app[bot]", &steer), None);
    assert_eq!(mark("other-app[bot]", &note), None);
}

#[tokio::test]
async fn the_client_marks_comments_with_its_logins() {
    let responder = Responder::start().await;
    let client = client(&responder);
    assert_eq!(
        client.mark(&comment(BOT, NOTE_MARKER)),
        Some(CommentMark::Note)
    );
    assert_eq!(client.mark(&comment("someone", STEER_MARKER)), None);
}

#[tokio::test]
async fn artifacts_are_listed_and_read() {
    let responder = Responder::start().await;
    responder.reply(200, ARTIFACTS);
    responder.reply(200, ARTIFACT);
    let client = client(&responder);
    let list = client.list_artifacts(&token(), ISSUE).await.unwrap();
    assert_eq!(
        list,
        [
            ArtifactSummary {
                id: 9101,
                number: 1,
                name: "analysis".into(),
                url: "https://github.com/acme/widgets/issues/180/artifacts/1".into(),
            },
            ArtifactSummary {
                id: 9102,
                number: 2,
                name: "plan".into(),
                url: "https://github.com/acme/widgets/issues/180/artifacts/2".into(),
            },
        ]
    );
    let artifact = client
        .read_artifact(&token(), ISSUE, list[1].number)
        .await
        .unwrap();
    assert_eq!(artifact.name, "plan");
    assert_eq!(artifact.body, "# Plan\n\nA GET-only client.");
    let requests = responder.requests();
    assert_eq!(
        paths(&requests),
        [
            format!("{ISSUE_PATH}/artifacts?per_page=100&page=1"),
            format!("{ISSUE_PATH}/artifacts/2"),
        ]
    );
    assert_token_only_in_authorization(&requests);
}

#[tokio::test]
async fn read_issue_maps_type_and_comments_since_cursor() {
    let responder = Responder::start().await;
    let comments = serde_json::json!([
        comment_json(7, "maintainer", "Steer.", "2026-11-24T10:00:00Z"),
        comment_json(8, BOT, "Noted.", "2026-11-24T11:30:00Z"),
    ]);
    responder.reply(200, ISSUE_JSON);
    responder.reply(200, comments.to_string());
    let since = Cursor("2026-11-24T10:00:00Z".into());
    let read = client(&responder)
        .read_issue(&token(), ISSUE, Some(since))
        .await
        .unwrap();
    assert_eq!(read.issue.number, ISSUE);
    assert_eq!(
        read.issue.title,
        "feat(runner): GitHub REST reads with paging, ETags and rate limiting"
    );
    assert_eq!(read.issue.body, "M1 reads issues as the runner App.");
    assert_eq!(read.issue.state, "open");
    assert_eq!(read.issue.labels, ["mintswarm", "component:domain"]);
    assert_eq!(read.issue.issue_type, Some(IssueType::Implementation));
    assert_eq!(read.issue.author, Login("maintainer".into()));
    assert!(!read.issue.author_is_bot);
    assert_eq!(read.issue.url, "https://github.com/acme/widgets/issues/180");
    assert_eq!(read.issue.sub_issues, 3);
    assert_eq!(read.issue.blocked_by, 2);
    assert_eq!(read.comments.len(), 2);
    assert_eq!(read.comments[1].id, 8);
    assert_eq!(read.comments[1].author, Login(BOT.into()));
    assert_eq!(read.comments[1].body, "Noted.");
    assert_eq!(read.cursor, Cursor("2026-11-24T11:30:00Z".into()));
    assert_eq!(
        responder.requests()[1].path,
        format!("{ISSUE_PATH}/comments?since=2026-11-24T10%3A00%3A00Z&per_page=100&page=1")
    );
}

#[tokio::test]
async fn without_new_comments_the_cursor_stays_or_starts_at_the_issue() {
    let responder = Responder::start().await;
    for _ in 0..2 {
        responder.reply(200, ISSUE_JSON);
        responder.reply(200, "[]");
    }
    let client = client(&responder);
    let since = Cursor("2026-11-24T12:00:00Z".into());
    let read = client
        .read_issue(&token(), ISSUE, Some(since.clone()))
        .await
        .unwrap();
    assert_eq!(read.cursor, since);
    let read = client.read_issue(&token(), ISSUE, None).await.unwrap();
    assert_eq!(read.cursor, Cursor("2026-11-20T09:00:00Z".into()));
}

#[tokio::test]
async fn missing_summaries_read_as_zero_and_a_summary_without_its_total_is_refused() {
    let responder = Responder::start().await;
    let mut issue: serde_json::Value = serde_json::from_str(ISSUE_JSON).unwrap();
    let fields = issue.as_object_mut().unwrap();
    fields.remove("sub_issues_summary");
    fields.insert("issue_dependencies_summary".into(), serde_json::Value::Null);
    responder.reply(200, issue.to_string());
    responder.reply(200, "[]");
    let read = client(&responder)
        .read_issue(&token(), ISSUE, None)
        .await
        .unwrap();
    assert_eq!(read.issue.sub_issues, 0);
    assert_eq!(read.issue.blocked_by, 0);

    issue["issue_dependencies_summary"] = serde_json::json!({ "blocked_by": 2 });
    responder.reply(200, issue.to_string());
    let refused = client(&responder).read_issue(&token(), ISSUE, None).await;
    assert!(matches!(refused, Err(ReadError::Unexpected { .. })));
}

#[tokio::test]
async fn an_issue_author_is_a_bot_by_type_or_by_login() {
    let responder = Responder::start().await;
    let mut issue: serde_json::Value = serde_json::from_str(ISSUE_JSON).unwrap();
    for user in [
        serde_json::json!({ "login": "helper", "type": "Bot" }),
        serde_json::json!({ "login": "relay[BOT]", "type": "User" }),
    ] {
        issue["user"] = user;
        responder.reply(200, issue.to_string());
        responder.reply(200, "[]");
        let read = client(&responder)
            .read_issue(&token(), ISSUE, None)
            .await
            .unwrap();
        assert!(read.issue.author_is_bot, "{}", issue["user"]);
    }
}

#[tokio::test]
async fn a_comment_author_is_a_bot_by_type_or_by_login() {
    let responder = Responder::start().await;
    let mut bot_typed = comment_json(9, "zmij", "Approve", "2026-11-24T10:00:00Z");
    bot_typed["user"] = serde_json::json!({ "login": "zmij", "type": "Bot" });
    let comments = serde_json::json!([
        bot_typed,
        comment_json(10, BOT, "Noted.", "2026-11-24T10:00:00Z"),
        comment_json(11, "maintainer", "Approve", "2026-11-24T10:00:00Z"),
    ]);
    responder.reply(200, ISSUE_JSON);
    responder.reply(200, comments.to_string());
    let read = client(&responder)
        .read_issue(&token(), ISSUE, None)
        .await
        .unwrap();
    let flags: Vec<bool> = read
        .comments
        .iter()
        .map(|comment| comment.author_is_bot)
        .collect();
    assert_eq!(flags, [true, true, false]);
    assert_eq!(read.comments[0].author, Login("zmij".into()));
}

#[tokio::test]
async fn labels_mode_takes_the_kind_from_the_first_matching_label() {
    let responder = Responder::start().await;
    let mut issue: serde_json::Value = serde_json::from_str(ISSUE_JSON).unwrap();
    issue["type"] = serde_json::Value::Null;
    issue["labels"] =
        serde_json::json!([{"name": "bug"}, {"name": "Kind:Plan"}, {"name": "kind:design"}]);
    responder.reply(200, issue.to_string());
    responder.reply(200, "[]");
    responder.reply(200, issue.to_string());
    responder.reply(200, "[]");
    let labels = Kinds::labels([
        (IssueType::Design, "kind:design".to_owned()),
        (IssueType::Plan, "kind:plan".to_owned()),
    ]);
    let read = client_with(&responder, labels)
        .read_issue(&token(), ISSUE, None)
        .await
        .unwrap();
    assert_eq!(read.issue.issue_type, Some(IssueType::Plan));
    // In issue-types mode an issue without a type has no kind, whatever its labels.
    let read = client(&responder)
        .read_issue(&token(), ISSUE, None)
        .await
        .unwrap();
    assert_eq!(read.issue.issue_type, None);
}

#[tokio::test]
async fn read_links_returns_parent_and_sub_issues() {
    let responder = Responder::start().await;
    // The parent's answer names no user: the link needs only its number.
    responder.reply(200, r#"{"number":152,"title":"Runner design"}"#);
    responder.reply(200, r#"[{"number":181},{"number":182}]"#);
    responder.reply(404, r#"{"message":"Not Found"}"#);
    responder.reply(200, "[]");
    let client = client(&responder);
    let links = client.read_links(&token(), ISSUE).await.unwrap();
    assert_eq!(
        links,
        [
            Link {
                kind: LinkKind::Parent,
                number: 152
            },
            Link {
                kind: LinkKind::SubItem,
                number: 181
            },
            Link {
                kind: LinkKind::SubItem,
                number: 182
            },
        ]
    );
    // A 404 for the parent means the issue has none.
    assert_eq!(client.read_links(&token(), ISSUE).await.unwrap(), []);
    assert_eq!(
        paths(&responder.requests()[..2]),
        [
            format!("{ISSUE_PATH}/parent"),
            format!("{ISSUE_PATH}/sub_issues?per_page=100&page=1"),
        ]
    );
}

/// The recorded permission fixture with `role_name` and `permission` set, or removed for `None`.
fn permission(role_name: Option<&str>, permission: Option<&str>) -> String {
    let mut body: serde_json::Value = serde_json::from_str(PERMISSION).unwrap();
    let object = body.as_object_mut().unwrap();
    for (key, value) in [("role_name", role_name), ("permission", permission)] {
        match value {
            Some(value) => object.insert(key.to_owned(), value.into()),
            None => object.remove(key),
        };
    }
    body.to_string()
}

fn zmij() -> Login {
    Login("zmij".into())
}

#[tokio::test]
async fn read_permission_maps_every_role() {
    let responder = Responder::start().await;
    // The recorded answer as it stands, then with each other role.
    responder.reply(200, PERMISSION);
    let roles = [
        ("maintain", "write", RepoRole::Maintain),
        ("write", "write", RepoRole::Write),
        ("triage", "read", RepoRole::Triage),
        ("read", "read", RepoRole::Read),
    ];
    for (role_name, granted, _) in roles {
        responder.reply(200, permission(Some(role_name), Some(granted)));
    }
    responder.reply(404, PERMISSION_NONE);
    let client = client(&responder);
    assert_eq!(
        client.read_permission(&token(), &zmij()).await,
        Ok(RepoRole::Admin)
    );
    for (_, _, role) in roles {
        assert_eq!(client.read_permission(&token(), &zmij()).await, Ok(role));
    }
    assert_eq!(
        client.read_permission(&token(), &zmij()).await,
        Ok(RepoRole::NoAccess)
    );
    let requests = responder.requests();
    assert_eq!(requests.len(), 6);
    for request in &requests {
        assert_eq!(request.method, "GET");
        assert_eq!(request.path, PERMISSION_PATH);
    }
    assert_token_only_in_authorization(&requests);
}

#[tokio::test]
async fn read_permission_falls_back_to_the_permission_field() {
    let responder = Responder::start().await;
    let cases = [
        (None, "admin", RepoRole::Admin),
        (None, "write", RepoRole::Write),
        (None, "read", RepoRole::Read),
        (None, "none", RepoRole::NoAccess),
        (Some("release-manager"), "write", RepoRole::Write),
        (Some("auditor"), "read", RepoRole::Read),
    ];
    for (role_name, granted, _) in cases {
        responder.reply(200, permission(role_name, Some(granted)));
    }
    let client = client(&responder);
    for (role_name, granted, role) in cases {
        assert_eq!(
            client.read_permission(&token(), &zmij()).await,
            Ok(role),
            "{role_name:?} {granted}"
        );
    }
}

#[tokio::test]
async fn read_permission_of_an_unknown_login_is_no_access() {
    let responder = Responder::start().await;
    responder.reply(404, PERMISSION_NONE);
    let client = client(&responder);
    let login = Login("no-such-user-msw-0001".into());
    assert_eq!(
        client.read_permission(&token(), &login).await,
        Ok(RepoRole::NoAccess)
    );
    // A login is sent as one path segment.
    responder.reply(404, PERMISSION_NONE);
    let login = Login("a/b c".into());
    assert_eq!(
        client.read_permission(&token(), &login).await,
        Ok(RepoRole::NoAccess)
    );
    assert_eq!(
        paths(&responder.requests()),
        [
            "/repos/acme/widgets/collaborators/no-such-user-msw-0001/permission",
            "/repos/acme/widgets/collaborators/a%2Fb%20c/permission",
        ]
    );
}

#[tokio::test]
async fn an_unrecognised_permission_is_unexpected() {
    let responder = Responder::start().await;
    responder.reply(200, permission(None, Some("superuser")));
    responder.reply(200, permission(Some("custom"), None));
    let client = client(&responder);
    let expected = Err(ReadError::Unexpected {
        endpoint: format!("GET {PERMISSION_PATH}"),
    });
    assert_eq!(client.read_permission(&token(), &zmij()).await, expected);
    assert_eq!(client.read_permission(&token(), &zmij()).await, expected);
}

/// The parent `ISSUE_JSON` reads as: filed by `maintainer`, a person.
fn maintainers_parent() -> ParentIssue {
    ParentIssue {
        number: ISSUE,
        author: Some(Login("maintainer".into())),
        author_is_bot: false,
    }
}

#[tokio::test]
async fn a_parent_author_is_read_with_its_bot_flag_or_as_none() {
    let responder = Responder::start().await;
    responder.reply(
        200,
        r#"{"number":7,"user":{"login":"helper","type":"Bot"}}"#,
    );
    responder.reply(200, r#"{"number":7}"#);
    responder.reply(200, r#"{"number":7,"user":null}"#);
    responder.reply(200, r#"{"number":7,"user":{"login":42,"type":"User"}}"#);
    responder.reply(200, r#"{"user":{"login":"erin","type":"User"}}"#);
    let client = client(&responder);
    assert_eq!(
        client.read_parent(&token(), IssueNumber(593)).await,
        Ok(Some(ParentIssue {
            number: IssueNumber(7),
            author: Some(Login("helper".into())),
            author_is_bot: true,
        }))
    );
    let no_author = Ok(Some(ParentIssue {
        number: IssueNumber(7),
        author: None,
        author_is_bot: false,
    }));
    for _ in 0..3 {
        assert_eq!(
            client.read_parent(&token(), IssueNumber(593)).await,
            no_author
        );
    }
    assert!(matches!(
        client.read_parent(&token(), IssueNumber(593)).await,
        Err(ReadError::Unexpected { .. })
    ));
}

#[tokio::test]
async fn read_parent_returns_the_parent_or_none() {
    let responder = Responder::start().await;
    responder.reply(200, ISSUE_JSON);
    responder.reply(404, PARENT_NONE);
    let client = client(&responder);
    assert_eq!(
        client.read_parent(&token(), IssueNumber(593)).await,
        Ok(Some(maintainers_parent()))
    );
    assert_eq!(
        client.read_parent(&token(), IssueNumber(593)).await,
        Ok(None)
    );
    let requests = responder.requests();
    assert_eq!(
        paths(&requests),
        [
            "/repos/acme/widgets/issues/593/parent",
            "/repos/acme/widgets/issues/593/parent",
        ]
    );
    assert!(requests.iter().all(|request| request.method == "GET"));
}

#[tokio::test]
async fn read_permission_and_read_parent_are_retried() {
    let responder = Responder::start().await;
    responder.reply(502, "");
    responder.reply(200, PERMISSION);
    responder.reply(502, "");
    responder.reply(200, ISSUE_JSON);
    let client = client(&responder).with_retry(fast_retries());
    assert_eq!(
        client.read_permission(&token(), &zmij()).await,
        Ok(RepoRole::Admin)
    );
    assert_eq!(
        client.read_parent(&token(), IssueNumber(593)).await,
        Ok(Some(maintainers_parent()))
    );
    assert_eq!(
        paths(&responder.requests()),
        [
            PERMISSION_PATH,
            PERMISSION_PATH,
            "/repos/acme/widgets/issues/593/parent",
            "/repos/acme/widgets/issues/593/parent",
        ]
    );
}

#[tokio::test]
async fn read_pr_maps_reviews_and_checks() {
    let responder = Responder::start().await;
    let sha = "0123456789abcdef0123456789abcdef01234567";
    responder.reply(200, PULL);
    responder.reply(
        200,
        serde_json::json!([{
            "id": 77,
            "user": {"login": "maintainer"},
            "state": "APPROVED",
            "submitted_at": "2026-11-24T12:00:00Z",
            "commit_id": sha,
            "html_url": "https://github.com/acme/widgets/pull/400#pullrequestreview-77",
        }])
        .to_string(),
    );
    responder.reply(
        200,
        serde_json::json!({
            "total_count": 2,
            "check_runs": [
                {"name": "rust-test", "conclusion": "success", "app": {"slug": "github-actions"}},
                {"name": "rust-lint", "conclusion": null, "app": {"slug": "github-actions"}},
            ],
        })
        .to_string(),
    );
    responder.reply(
        200,
        serde_json::json!({
            "state": "pending",
            "statuses": [
                {"context": "mintswarm/gates", "state": "pending"},
                {"context": "mintswarm/review", "state": "failure"},
            ],
        })
        .to_string(),
    );
    let pr = client(&responder)
        .read_pr(&token(), PrNumber(400))
        .await
        .unwrap();
    assert_eq!(pr.number, PrNumber(400));
    assert_eq!(pr.url, "https://github.com/acme/widgets/pull/400");
    assert_eq!(pr.branch, BranchName("mintswarm/180-rest-reads".into()));
    assert_eq!(pr.base, BranchName("master".into()));
    assert_eq!(pr.head, RevId(sha.into()));
    assert_eq!(pr.title, "feat(domain): GitHub REST reads");
    assert_eq!(pr.state, "open");
    assert!(!pr.draft);
    assert_eq!(pr.mergeable_state.as_deref(), Some("blocked"));
    assert_eq!(pr.requested_reviewers, [Login("maintainer".into())]);
    assert_eq!(pr.reviews.len(), 1);
    assert_eq!(pr.reviews[0].id, 77);
    assert_eq!(pr.reviews[0].state, "APPROVED");
    assert_eq!(
        pr.reviews[0].submitted_at.as_deref(),
        Some("2026-11-24T12:00:00Z")
    );
    assert_eq!(pr.reviews[0].commit, Some(RevId(sha.into())));
    assert_eq!(
        pr.reviews[0].url,
        "https://github.com/acme/widgets/pull/400#pullrequestreview-77"
    );
    assert_eq!(
        pr.checks,
        [
            CheckState {
                name: "rust-test".into(),
                conclusion: Some("success".into()),
                source: "check-run".into(),
                app: Some("github-actions".into()),
            },
            CheckState {
                name: "rust-lint".into(),
                conclusion: None,
                source: "check-run".into(),
                app: Some("github-actions".into()),
            },
            CheckState {
                name: "mintswarm/gates".into(),
                // A pending status has not concluded, like the running check run above.
                conclusion: None,
                source: "status".into(),
                app: None,
            },
            CheckState {
                name: "mintswarm/review".into(),
                conclusion: Some("failure".into()),
                source: "status".into(),
                app: None,
            },
        ]
    );
    let requests = responder.requests();
    let commit = format!("/repos/acme/widgets/commits/{sha}");
    assert_eq!(
        paths(&requests),
        [
            "/repos/acme/widgets/pulls/400".to_owned(),
            "/repos/acme/widgets/pulls/400/reviews?per_page=100&page=1".to_owned(),
            format!("{commit}/check-runs?filter=latest&per_page=100&page=1"),
            format!("{commit}/status?per_page=100&page=1"),
        ]
    );
    assert_token_only_in_authorization(&requests);
}

#[test]
fn the_reads_ask_for_read_permissions_only() {
    assert_eq!(
        GitHubReads::permissions(),
        mintswarm_github::Permissions::read([
            "checks",
            "issues",
            "metadata",
            "pull_requests",
            "statuses"
        ])
    );
}

/// The `Link` header of page `page` of the list at `url`, which already carries a query.
fn page_link(url: &str, page: u64, next: bool) -> String {
    let mut link = format!("<{url}&page=1>; rel=\"first\"");
    if next {
        link.push_str(&format!(", <{url}&page={}>; rel=\"next\"", page + 1));
    }
    link
}

/// An issue of `owner/repo` as a dependency list carries it.
fn dependency_json(owner: &str, repo: &str, number: u64, state: &str) -> serde_json::Value {
    serde_json::json!({
        "id": 5_000_000 + number,
        "number": number,
        "title": "A dependency",
        "state": state,
        "url": format!("https://api.github.com/repos/{owner}/{repo}/issues/{number}"),
        "repository_url": format!("https://api.github.com/repos/{owner}/{repo}"),
        "html_url": format!("https://github.com/{owner}/{repo}/issues/{number}"),
        "user": { "login": "someone", "id": 2, "type": "User" },
        "labels": [],
        "created_at": "2026-11-10T09:00:00Z",
        "updated_at": "2026-11-21T10:00:00Z",
    })
}

fn dependency(repository: &str, number: u64, open: bool) -> Dependency {
    Dependency {
        repository: repository.to_owned(),
        number: IssueNumber(number),
        open,
    }
}

#[tokio::test]
async fn both_directions_follow_pages_and_a_repeat_is_served_from_the_cache_on_304() {
    let cases = [
        (
            Direction::BlockedBy,
            "blocked_by",
            BLOCKED_BY,
            vec![
                dependency("acme/widgets", 175, false),
                dependency("acme-tools/gadgets", 42, true),
            ],
        ),
        (
            Direction::Blocking,
            "blocking",
            BLOCKING,
            vec![dependency("acme/widgets", 190, true)],
        ),
    ];
    for (direction, segment, page_one, mut expected) in cases {
        let responder = Responder::start().await;
        let url = format!(
            "https://api.github.com/repositories/1/issues/180/dependencies/{segment}?per_page=100"
        );
        let first = page_link(&url, 1, true);
        let second = page_link(&url, 2, false);
        let page_two = serde_json::json!([dependency_json("acme", "widgets", 160, "open")]);
        let (etag_one, etag_two) = (format!("\"{segment}-1\""), format!("\"{segment}-2\""));
        responder.reply_with(
            200,
            &[("etag", etag_one.as_str()), ("link", first.as_str())],
            page_one,
        );
        responder.reply_with(
            200,
            &[("etag", etag_two.as_str()), ("link", second.as_str())],
            page_two.to_string(),
        );
        responder.reply_with(304, &[("link", first.as_str())], "");
        responder.reply_with(304, &[("link", second.as_str())], "");
        let client = client(&responder);
        let read = client
            .read_dependencies(&token(), ISSUE, direction)
            .await
            .unwrap();
        expected.push(dependency("acme/widgets", 160, true));
        assert_eq!(read, expected, "{segment}");
        let again = client
            .read_dependencies(&token(), ISSUE, direction)
            .await
            .unwrap();
        assert_eq!(again, read, "{segment}");

        let requests = responder.requests();
        let page = |n: u64| format!("{ISSUE_PATH}/dependencies/{segment}?per_page=100&page={n}");
        assert_eq!(
            paths(&requests),
            vec![page(1), page(2), page(1), page(2)],
            "{segment}"
        );
        assert_eq!(requests[0].header("if-none-match"), None, "{segment}");
        assert_eq!(requests[1].header("if-none-match"), None, "{segment}");
        assert_eq!(
            requests[2].header("if-none-match"),
            Some(etag_one.as_str()),
            "{segment}"
        );
        assert_eq!(
            requests[3].header("if-none-match"),
            Some(etag_two.as_str()),
            "{segment}"
        );
        assert_token_only_in_authorization(&requests);
    }
}

#[tokio::test]
async fn a_dependency_without_a_repository_it_names_is_refused() {
    for url in [
        serde_json::Value::from("https://api.github.com/repositories/77"),
        serde_json::Value::from("https://api.github.com/repos/acme"),
        serde_json::Value::from("https://api.github.com/repos/acme/wid gets"),
        serde_json::Value::from("https://api.github.com/repos/acme/widgets/extra"),
        serde_json::Value::Null,
    ] {
        let responder = Responder::start().await;
        let mut item = dependency_json("acme", "widgets", 160, "open");
        item["repository_url"] = url.clone();
        responder.reply(200, serde_json::json!([item]).to_string());
        let error = client(&responder)
            .read_dependencies(&token(), ISSUE, Direction::BlockedBy)
            .await
            .unwrap_err();
        assert!(
            matches!(error, ReadError::Unexpected { .. }),
            "{url}: {error:?}"
        );
    }
}

const LIST_PATH: &str = "/repos/acme/widgets/issues";
const FIRST_DATE: &str = "Thu, 26 Nov 2026 10:00:00 GMT";
const SECOND_DATE: &str = "Thu, 26 Nov 2026 10:00:02 GMT";

/// One more issue, as the listing's second page.
fn listed_issue(number: u64) -> serde_json::Value {
    serde_json::json!({
        "id": 3_000_000 + number,
        "number": number,
        "title": "Another issue",
        "body": "",
        "state": "open",
        "html_url": format!("https://github.com/acme/widgets/issues/{number}"),
        "user": { "login": "maintainer", "id": 1, "type": "User" },
        "labels": [{ "id": 11, "name": "mintswarm" }],
        "created_at": "2026-11-25T09:00:00Z",
        "updated_at": "2026-11-26T09:50:00Z",
    })
}

fn full_query() -> IssueQuery {
    IssueQuery {
        state: IssueState::All,
        since: Some("2026-11-26T09:00:00Z".to_owned()),
        labels: vec!["mintswarm".to_owned(), "component:domain".to_owned()],
        creator: Some(Login("mintswarm-dev-bee[bot]".into())),
    }
}

#[tokio::test]
async fn the_listing_sends_its_query_follows_pages_and_drops_pull_requests() {
    let responder = Responder::start().await;
    let url = "https://api.github.com/repositories/1/issues?state=all&per_page=100";
    let first = page_link(url, 1, true);
    let second = page_link(url, 2, false);
    responder.reply_with(
        200,
        &[("link", first.as_str()), ("date", FIRST_DATE)],
        ISSUES_LIST,
    );
    responder.reply_with(
        200,
        &[("link", second.as_str()), ("date", SECOND_DATE)],
        serde_json::json!([listed_issue(191)]).to_string(),
    );
    let list = client(&responder)
        .list_issues(&token(), &full_query())
        .await
        .unwrap();
    let numbers: Vec<u64> = list.issues.iter().map(|issue| issue.number.0).collect();
    assert_eq!(numbers, vec![180, 170, 191]);
    assert_eq!(list.issues[0].issue_type, Some(IssueType::Implementation));
    assert_eq!(list.issues[0].blocked_by, 1);
    assert_eq!(list.issues[1].state, "closed");
    assert_eq!(list.issues[1].issue_type, Some(IssueType::Design));
    // The first reply's clock, not the second's.
    assert_eq!(list.date, "2026-11-26T10:00:00Z");

    let requests = responder.requests();
    let query = "state=all&since=2026-11-26T09%3A00%3A00Z&labels=mintswarm,component%3Adomain\
                 &creator=mintswarm-dev-bee%5Bbot%5D";
    assert_eq!(
        paths(&requests),
        vec![
            format!("{LIST_PATH}?{query}&per_page=100&page=1"),
            format!("{LIST_PATH}?{query}&per_page=100&page=2"),
        ]
    );
    assert_token_only_in_authorization(&requests);
}

#[tokio::test]
async fn a_listing_with_only_a_state_sends_only_the_state() {
    let responder = Responder::start().await;
    responder.reply_with(200, &[("date", FIRST_DATE)], "[]");
    let query = IssueQuery {
        state: IssueState::Open,
        since: None,
        labels: Vec::new(),
        creator: None,
    };
    let list = client(&responder)
        .list_issues(&token(), &query)
        .await
        .unwrap();
    assert!(list.issues.is_empty());
    assert_eq!(list.date, "2026-11-26T10:00:00Z");
    assert_eq!(
        paths(&responder.requests()),
        vec![format!("{LIST_PATH}?state=open&per_page=100&page=1")]
    );
}

#[tokio::test]
async fn a_listing_answered_304_takes_the_date_of_that_reply_not_the_cache() {
    let responder = Responder::start().await;
    responder.reply_with(
        200,
        &[("etag", "\"list-v1\""), ("date", FIRST_DATE)],
        ISSUES_LIST,
    );
    responder.reply_with(
        304,
        &[
            ("etag", "\"list-v1\""),
            ("date", "Fri, 27 Nov 2026 08:15:00 GMT"),
        ],
        "",
    );
    let client = client(&responder);
    let first = client.list_issues(&token(), &full_query()).await.unwrap();
    let second = client.list_issues(&token(), &full_query()).await.unwrap();
    assert_eq!(first.date, "2026-11-26T10:00:00Z");
    assert_eq!(second.date, "2026-11-27T08:15:00Z");
    assert_eq!(second.issues, first.issues);
    let requests = responder.requests();
    assert_eq!(requests[1].header("if-none-match"), Some("\"list-v1\""));
}

#[tokio::test]
async fn a_listing_without_a_date_it_can_read_is_refused() {
    for headers in [
        Vec::new(),
        vec![("date", "Thursday, 26-Nov-26 10:00:00 GMT")],
    ] {
        let responder = Responder::start().await;
        responder.reply_with(200, &headers, "[]");
        let error = client(&responder)
            .list_issues(&token(), &full_query())
            .await
            .unwrap_err();
        assert!(
            matches!(error, ReadError::Unexpected { .. }),
            "{headers:?}: {error:?}"
        );
    }
}

#[tokio::test]
async fn a_listing_for_a_label_holding_a_comma_is_refused_before_anything_is_sent() {
    let responder = Responder::start().await;
    let mut query = full_query();
    query.labels.push("needs input, urgent".to_owned());
    let error = client(&responder)
        .list_issues(&token(), &query)
        .await
        .unwrap_err();
    assert!(
        matches!(&error, ReadError::CommaInLabel { label } if label == "needs input, urgent"),
        "{error:?}"
    );
    assert!(responder.requests().is_empty());
}
