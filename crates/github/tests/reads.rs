//! The GET-only reads against a scripted local server answering with hand-written fixtures in
//! the shape of GitHub's REST API.
// Not redundant: clippy's `allow-unwrap-in-tests` covers a `tests/` crate only under `cfg(test)`.
#![cfg(test)]

mod support;

use std::time::{Duration, Instant};

use mintswarm_domain::ids::{IssueNumber, PrNumber, RevId};
use mintswarm_domain::tracker::{
    ArtifactSummary, CheckState, Comment, CommentMark, Cursor, IssueType, Link, LinkKind, Login,
};
use mintswarm_domain::vcs::BranchName;
use mintswarm_github::{
    GitHubReads, Kinds, NOTE_MARKER, ReadError, STEER_MARKER, Token, comment_mark,
};
use support::{Request, Responder};

const TOKEN: &str = "ghs_r34dsT0kenV4lue";
const ISSUE_JSON: &str = include_str!("fixtures/reads/issue.json");
const ARTIFACTS: &str = include_str!("fixtures/reads/artifacts.json");
const ARTIFACT: &str = include_str!("fixtures/reads/artifact.json");
const PULL: &str = include_str!("fixtures/reads/pull.json");
const ISSUE: IssueNumber = IssueNumber(180);
const ISSUE_PATH: &str = "/repos/acme/widgets/issues/180";
const BOT: &str = "mintswarm-dev[bot]";

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
            ReadError::RateLimited {
                endpoint: format!("GET {ISSUE_PATH}"),
                retry_after: Some(Duration::from_secs(1)),
            },
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
    assert!(matches!(error, ReadError::RateLimited { .. }), "{error:?}");
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
            ReadError::Status {
                endpoint: endpoint.clone(),
                status: 502
            },
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
        mark("MintSwarm-Dev[bot]", &format!("  {STEER_MARKER}  ")),
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
    assert_eq!(read.issue.url, "https://github.com/acme/widgets/issues/180");
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
