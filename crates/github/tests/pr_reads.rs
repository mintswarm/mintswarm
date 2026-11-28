//! The pull request reads against a scripted local server answering with replies recorded in
//! the shape of GitHub's REST and GraphQL APIs, from `fixtures/pulls/pr-400.json`.
// Not redundant: clippy's `allow-unwrap-in-tests` covers a `tests/` crate only under `cfg(test)`.
#![cfg(test)]

mod support;

use mintswarm_domain::ids::{PrNumber, RevId};
use mintswarm_domain::tracker::{
    ChangedFile, CheckRun, CheckState, CheckSuite, IssueType, Login, RequestedReviewers,
    ReviewThread,
};
use mintswarm_domain::vcs::BranchName;
use mintswarm_github::{GitHubReads, Kinds, ReadError, RetryPolicy, Token};
use serde_json::Value;
use support::{Request, Responder};

const TOKEN: &str = "ghs_pr4ead5T0kenV4lue";
const FIXTURE: &str = include_str!("fixtures/pulls/pr-400.json");
const PR: PrNumber = PrNumber(400);
const PULL_PATH: &str = "/repos/acme/widgets/pulls/400";
const HEAD: &str = "0123456789abcdef0123456789abcdef01234567";

fn token() -> Token {
    Token::new(TOKEN.to_owned(), 0)
}

/// The recorded reply under `key`, as JSON text.
fn recorded(key: &str) -> String {
    let fixture: Value = serde_json::from_str(FIXTURE).unwrap();
    fixture
        .get(key)
        .unwrap_or_else(|| panic!("no recorded reply {key}"))
        .to_string()
}

fn client(responder: &Responder) -> GitHubReads {
    GitHubReads::new(
        &responder.base,
        "acme",
        "widgets",
        Kinds::issue_types([(IssueType::Plan, "Plan".to_owned())]),
        Vec::new(),
        Login("mintswarm-dev-bee[bot]".into()),
    )
    .unwrap()
    // A single attempt per read, so each read takes one scripted reply.
    .with_retry(RetryPolicy {
        attempts: 1,
        ..RetryPolicy::default()
    })
}

fn paths(requests: &[Request]) -> Vec<String> {
    requests
        .iter()
        .map(|request| request.path.clone())
        .collect()
}

/// Every request is a GET, and no request shows the token outside its `authorization` header.
fn assert_gets_with_the_token(requests: &[Request]) {
    for request in requests {
        assert_eq!(request.method, "GET");
        assert_token_only_in_authorization(request);
    }
}

fn assert_token_only_in_authorization(request: &Request) {
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
async fn the_pull_carries_its_own_fields() {
    let responder = Responder::start().await;
    responder.reply(200, recorded("pull"));
    let pull = client(&responder).read_pull(&token(), PR).await.unwrap();
    assert_eq!(pull.number, PR);
    assert_eq!(pull.url, "https://github.com/acme/widgets/pull/400");
    assert_eq!(pull.title, "feat(domain): GitHub REST reads");
    assert_eq!(pull.state, "open");
    assert!(!pull.draft);
    assert_eq!(pull.base, BranchName("master".into()));
    assert_eq!(pull.branch, BranchName("mintswarm/180-rest-reads".into()));
    assert_eq!(pull.head, RevId(HEAD.into()));
    assert_eq!(pull.head_repo.as_deref(), Some("acme/widgets"));
    assert_eq!(pull.labels, ["mintswarm", "component:domain"]);
    assert_eq!(pull.mergeable, Some(true));
    assert_eq!(pull.mergeable_state.as_deref(), Some("blocked"));
    assert!(!pull.merged);
    assert_eq!(
        pull.merge_commit,
        Some(RevId("fedcba9876543210fedcba9876543210fedcba98".into()))
    );
    assert_eq!(pull.changed_files, 3);
    let requests = responder.requests();
    assert_eq!(paths(&requests), [PULL_PATH]);
    assert_gets_with_the_token(&requests);
}

#[tokio::test]
async fn reviews_carry_state_commit_and_bot_author() {
    let responder = Responder::start().await;
    responder.reply(200, recorded("reviews"));
    let reviews = client(&responder).read_reviews(&token(), PR).await.unwrap();
    let got: Vec<_> = reviews
        .iter()
        .map(|review| {
            (
                review.id,
                review.author.0.as_str(),
                review.state.as_str(),
                review.commit.as_ref().map(|commit| commit.0.as_str()),
                review.author_is_bot,
            )
        })
        .collect();
    assert_eq!(
        got,
        [
            (77, "maintainer", "APPROVED", Some(HEAD), false),
            (
                78,
                "review-helper",
                "COMMENTED",
                Some("00112233445566778899aabbccddeeff00112233"),
                true,
            ),
            (
                79,
                "mintswarm-dev-bee[bot]",
                "CHANGES_REQUESTED",
                Some(HEAD),
                true
            ),
        ]
    );
    assert_eq!(
        reviews[0].submitted_at.as_deref(),
        Some("2026-11-25T10:00:00Z")
    );
    assert_eq!(
        reviews[0].url,
        "https://github.com/acme/widgets/pull/400#pullrequestreview-77"
    );
    let requests = responder.requests();
    assert_eq!(
        paths(&requests),
        [format!("{PULL_PATH}/reviews?per_page=100&page=1")]
    );
    assert_gets_with_the_token(&requests);
}

#[tokio::test]
async fn a_bot_is_told_by_its_type_or_its_login() {
    let responder = Responder::start().await;
    let review = |id: u64, login: &str, kind: &str| {
        serde_json::json!({
            "id": id,
            "user": {"login": login, "id": id, "type": kind},
            "state": "APPROVED",
            "submitted_at": "2026-11-25T10:00:00Z",
            "commit_id": HEAD,
            "html_url": format!("https://github.com/acme/widgets/pull/400#pullrequestreview-{id}"),
        })
    };
    responder.reply(
        200,
        serde_json::json!([
            review(1, "someone", "User"),
            review(2, "helper", "Bot"),
            // A login ending in `[bot]` is a bot whatever its type says.
            review(3, "helper[bot]", "User"),
        ])
        .to_string(),
    );
    let reviews = client(&responder).read_reviews(&token(), PR).await.unwrap();
    let bots: Vec<_> = reviews.iter().map(|review| review.author_is_bot).collect();
    assert_eq!(bots, [false, true, true]);
}

#[tokio::test]
async fn review_comments_carry_their_author_and_times() {
    let responder = Responder::start().await;
    responder.reply(200, recorded("review_comments"));
    let comments = client(&responder)
        .read_review_comments(&token(), PR)
        .await
        .unwrap();
    assert_eq!(comments.len(), 2);
    assert_eq!(comments[0].id, 901);
    assert_eq!(comments[0].review, Some(78));
    assert_eq!(comments[0].author, Login("review-helper".into()));
    assert!(comments[0].author_is_bot);
    assert_eq!(comments[0].body, "This mapper drops the app.");
    assert_eq!(comments[0].created_at, "2026-11-25T10:05:00Z");
    assert_eq!(comments[0].updated_at, "2026-11-25T10:06:00Z");
    assert_eq!(
        comments[0].url,
        "https://github.com/acme/widgets/pull/400#discussion_r901"
    );
    assert_eq!(comments[1].review, None);
    assert!(!comments[1].author_is_bot);
    // A null body reads as empty.
    assert_eq!(comments[1].body, "");
    let requests = responder.requests();
    assert_eq!(
        paths(&requests),
        [format!("{PULL_PATH}/comments?per_page=100&page=1")]
    );
    assert_gets_with_the_token(&requests);
}

#[tokio::test]
async fn requested_reviewers_are_users_and_teams() {
    let responder = Responder::start().await;
    responder.reply(200, recorded("requested_reviewers"));
    let requested = client(&responder)
        .read_requested_reviewers(&token(), PR)
        .await
        .unwrap();
    assert_eq!(
        requested,
        RequestedReviewers {
            users: vec![Login("maintainer".into())],
            teams: vec!["reviewers".into()],
        }
    );
    assert_eq!(
        paths(&responder.requests()),
        [format!("{PULL_PATH}/requested_reviewers")]
    );
}

#[tokio::test]
async fn review_threads_are_read_across_graphql_pages() {
    let responder = Responder::start().await;
    responder.reply(200, recorded("threads_page_1"));
    responder.reply(200, recorded("threads_page_2"));
    let threads = client(&responder)
        .read_review_threads(&token(), PR)
        .await
        .unwrap();
    assert_eq!(
        threads,
        [
            ReviewThread {
                id: "PRRT_kwDOAbCdEs5AAAAB".into(),
                resolved: true,
            },
            ReviewThread {
                id: "PRRT_kwDOAbCdEs5AAAAC".into(),
                resolved: false,
            },
            ReviewThread {
                id: "PRRT_kwDOAbCdEs5AAAAD".into(),
                resolved: true,
            },
        ]
    );
    let requests = responder.requests();
    assert_eq!(paths(&requests), ["/graphql", "/graphql"]);
    let bodies: Vec<Value> = requests
        .iter()
        .map(|request| {
            assert_eq!(request.method, "POST");
            assert_token_only_in_authorization(request);
            serde_json::from_str(&request.body).unwrap()
        })
        .collect();
    for body in &bodies {
        let query = body["query"].as_str().unwrap();
        assert!(query.trim_start().starts_with("query("), "{query}");
        assert!(query.contains("isResolved"), "{query}");
        assert_eq!(body["variables"]["owner"], "acme");
        assert_eq!(body["variables"]["name"], "widgets");
        assert_eq!(body["variables"]["number"], 400);
    }
    assert_eq!(bodies[0]["variables"]["after"], Value::Null);
    assert_eq!(
        bodies[1]["variables"]["after"],
        "Y3Vyc29yOnYyOpK0MjAyNi0xMS0yNVQxMDowNTowMFo"
    );
}

#[tokio::test]
async fn graphql_errors_are_typed() {
    let responder = Responder::start().await;
    responder.reply(200, recorded("threads_errors"));
    let error = client(&responder)
        .read_review_threads(&token(), PR)
        .await
        .unwrap_err();
    match &error {
        ReadError::GraphQl { types, .. } => assert_eq!(types, &["FORBIDDEN"]),
        other => panic!("{other:?}"),
    }
    // Only our own text, never GitHub's message.
    assert!(!error.to_string().contains("Resource not accessible"));

    let responder = Responder::start().await;
    responder.reply(200, recorded("threads_no_pull"));
    let error = client(&responder)
        .read_review_threads(&token(), PR)
        .await
        .unwrap_err();
    assert!(matches!(error, ReadError::NotFound { .. }), "{error:?}");
}

#[tokio::test]
async fn check_runs_suites_and_statuses_for_a_sha() {
    let responder = Responder::start().await;
    responder.reply(200, recorded("check_runs"));
    responder.reply(200, recorded("check_suites"));
    responder.reply(200, recorded("status"));
    let reads = client(&responder);
    let sha = RevId(HEAD.into());
    let runs = reads.read_check_runs(&token(), &sha).await.unwrap();
    let suites = reads.read_check_suites(&token(), &sha).await.unwrap();
    let status = reads.read_statuses(&token(), &sha).await.unwrap();
    assert_eq!(
        runs,
        [
            CheckRun {
                id: 3001,
                name: "rust-test".into(),
                status: "completed".into(),
                conclusion: Some("success".into()),
                app: Some("github-actions".into()),
                suite: Some(2001),
            },
            CheckRun {
                id: 3002,
                name: "rust-lint".into(),
                status: "in_progress".into(),
                conclusion: None,
                app: Some("github-actions".into()),
                suite: Some(2001),
            },
        ]
    );
    assert_eq!(
        suites,
        [
            CheckSuite {
                id: 2001,
                app: Some("github-actions".into()),
                status: Some("in_progress".into()),
                conclusion: None,
                latest_check_runs_count: 2,
            },
            CheckSuite {
                id: 2002,
                app: Some("dependency-checker".into()),
                status: Some("completed".into()),
                conclusion: Some("neutral".into()),
                latest_check_runs_count: 0,
            },
        ]
    );
    assert_eq!(status.state, "failure");
    assert_eq!(status.total_count, 2);
    assert_eq!(
        status.statuses,
        [
            CheckState {
                name: "mintswarm/gates".into(),
                // A pending status has not concluded.
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
    let commit = format!("/repos/acme/widgets/commits/{HEAD}");
    let requests = responder.requests();
    assert_eq!(
        paths(&requests),
        [
            format!("{commit}/check-runs?filter=latest&per_page=100&page=1"),
            format!("{commit}/check-suites?per_page=100&page=1"),
            format!("{commit}/status?per_page=100&page=1"),
        ]
    );
    assert_gets_with_the_token(&requests);
}

/// The `Link` header of page `page` of the PR's files, with a `next` link when `next` is set.
fn files_link(page: u64, next: bool) -> String {
    let url = "https://api.github.com/repositories/1/pulls/400/files?per_page=100";
    let mut link = format!("<{url}&page=1>; rel=\"first\"");
    if next {
        link.push_str(&format!(", <{url}&page={}>; rel=\"next\"", page + 1));
    } else {
        link.push_str(&format!(", <{url}&page={}>; rel=\"prev\"", page - 1));
    }
    link
}

#[tokio::test]
async fn changed_files_span_pages_and_keep_both_rename_paths() {
    let responder = Responder::start().await;
    responder.reply_with(
        200,
        &[("link", &files_link(1, true))],
        recorded("files_page_1"),
    );
    responder.reply_with(
        200,
        &[("link", &files_link(2, false))],
        recorded("files_page_2"),
    );
    let files = client(&responder)
        .read_changed_files(&token(), PR)
        .await
        .unwrap();
    assert_eq!(
        files,
        [
            ChangedFile {
                path: "crates/github/src/pulls.rs".into(),
                previous_path: None,
                status: "modified".into(),
            },
            ChangedFile {
                path: "crates/github/src/merges.rs".into(),
                previous_path: None,
                status: "added".into(),
            },
            ChangedFile {
                path: "docs/architecture/github.md".into(),
                previous_path: Some("docs/github.md".into()),
                status: "renamed".into(),
            },
        ]
    );
    let every: Vec<&str> = files.iter().flat_map(ChangedFile::paths).collect();
    assert_eq!(
        every,
        [
            "crates/github/src/pulls.rs",
            "crates/github/src/merges.rs",
            "docs/architecture/github.md",
            "docs/github.md",
        ]
    );
    let requests = responder.requests();
    assert_eq!(
        paths(&requests),
        [
            format!("{PULL_PATH}/files?per_page=100&page=1"),
            format!("{PULL_PATH}/files?per_page=100&page=2"),
        ]
    );
    assert_gets_with_the_token(&requests);
}

#[tokio::test]
async fn a_changed_file_list_at_the_3000_cap_is_refused() {
    let responder = Responder::start().await;
    let file = |n: u64| {
        serde_json::json!({
            "sha": "1111111111111111111111111111111111111111",
            "filename": format!("src/file-{n}.rs"),
            "status": "added",
        })
    };
    for page in 1..=30u64 {
        let items: Vec<_> = (0..100).map(|n| file(page * 100 + n)).collect();
        // GitHub stops at 3000 files with no page after the 30th.
        responder.reply_with(
            200,
            &[("link", &files_link(page, page < 30))],
            Value::from(items).to_string(),
        );
    }
    let error = client(&responder)
        .read_changed_files(&token(), PR)
        .await
        .unwrap_err();
    assert!(matches!(error, ReadError::TooManyPages { .. }), "{error:?}");
    assert_eq!(responder.requests().len(), 30);
}
