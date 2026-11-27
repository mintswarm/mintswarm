//! The GitHub tracker against a scripted local server answering with JSON in the shapes of the
//! `mintswarm-github` fixtures: field ids learned, reused, forgotten and deferred, and the other
//! capabilities over their clients, the reactions, edits and permission reads replaying the
//! recorded fixtures themselves.
// Not redundant: clippy's `allow-unwrap-in-tests` covers a `tests/` crate only under `cfg(test)`.
#![cfg(test)]

use std::sync::{Arc, Mutex};

use mintswarm_core::tracker::github::{
    GitHubClients, GitHubTracker, GitHubTrackerError, InstallationTokens,
};
use mintswarm_domain::field::{
    FieldError, FieldFinding, FieldRead, FieldRef, FieldSource, FieldValue,
};
use mintswarm_domain::ids::IssueNumber;
use mintswarm_domain::tracker::{
    Collaborators, CommentEdits, Comments, Cursor, Fields, IssueType, Issues, Links, Login,
    Reactions, RepoRole, Statuses, SubItem,
};
use mintswarm_github::{
    Clock, CommentClient, CommentError, ContentLimiter, FieldClient, GitHubReads, IssueWrites,
    Kinds, NOTE_MARKER, Permissions, RetryPolicy, StatusClient, Token, TokenError, WriteError,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

const TOKEN: &str = "ghs_tr4ckerT0kenV4lue";
const BOT: &str = "mintswarm-dev[bot]";
const ISSUE: IssueNumber = IssueNumber(7);
const VALUES_PATH: &str = "/repos/acme/widgets/issues/7/issue-field-values";
const ORG_FIELDS_PATH: &str = "/orgs/acme/issue-fields?per_page=100&page=1";
/// Issue 7's field values: Effort is `M` under id 101, Priority `P1` under id 202.
const VALUES: &str = r#"[
  {"issue_field_id":101,"issue_field_name":"Effort","data_type":"single_select","value":"M",
   "single_select_option":{"id":1003,"name":"M","color":"blue"}},
  {"issue_field_id":202,"issue_field_name":"Priority","data_type":"single_select","value":"P1",
   "single_select_option":{"id":2002,"name":"P1","color":"orange"}}
]"#;
/// GitHub's answer to writing `L` to Effort under id 101.
const WRITTEN_101: &str = r#"[
  {"issue_field_id":101,"issue_field_name":"Effort","data_type":"single_select","value":"L",
   "single_select_option":{"id":1004,"name":"L","color":"yellow"}}
]"#;
/// Issue 7's values after Effort was recreated under id 909.
const RECREATED: &str = r#"[
  {"issue_field_id":909,"issue_field_name":"Effort","data_type":"single_select","value":"M",
   "single_select_option":{"id":9003,"name":"M","color":"blue"}}
]"#;
/// GitHub's answer to writing `L` to Effort under id 909.
const WRITTEN_909: &str = r#"[
  {"issue_field_id":909,"issue_field_name":"Effort","data_type":"single_select","value":"L",
   "single_select_option":{"id":9004,"name":"L","color":"yellow"}}
]"#;
const FORBIDDEN: &str = r#"{"message":"Resource not accessible by integration"}"#;
const ISSUE_JSON: &str = r#"{
  "id": 3000007, "node_id": "I_kwDOAcmeWidgets7", "number": 7,
  "title": "feat(core): ingest tracker events", "body": "Ingest the events.", "state": "open",
  "html_url": "https://github.com/acme/widgets/issues/7",
  "user": {"login": "maintainer", "id": 1, "type": "User"},
  "labels": [{"id": 11, "name": "mintswarm"}],
  "type": {"id": 21, "name": "Task"},
  "status": {"id": "PS_kwDOAcmeQueued", "name": "Queued"},
  "sub_issues_summary": {"total": 0, "completed": 0, "percent_completed": 0},
  "issue_dependencies_summary": {"blocked_by": 1, "total_blocked_by": 1, "blocking": 0, "total_blocking": 0},
  "created_at": "2026-11-20T09:00:00Z", "updated_at": "2026-11-24T10:00:00Z"
}"#;
const COMMENTS_JSON: &str = r#"[{
  "id": 501, "user": {"login": "maintainer", "type": "User"}, "body": "Looks right.",
  "created_at": "2026-11-21T08:00:00Z", "updated_at": "2026-11-22T08:00:00Z",
  "html_url": "https://github.com/acme/widgets/issues/7#issuecomment-501"
}]"#;
const STATUSES: &str = r#"{"data":{"node":{"repository":{"planningStatuses":{"nodes":[
  {"id":"PS_kwDOAcmeQueued","name":"Queued"},{"id":"PS_kwDOAcmeDone","name":"Done"}]}}}}}"#;
const SET_STATUS: &str = r#"{"data":{"setIssueStatus":{"clientMutationId":null}}}"#;
/// The fixtures `mintswarm-github` recorded against GitHub.
const REACTION: &str = include_str!("../../github/tests/fixtures/writes/reaction.json");
const COMMENT_EDITED: &str = include_str!("../../github/tests/fixtures/writes/comment-edited.json");
const ARTIFACT_UPDATED: &str =
    include_str!("../../github/tests/fixtures/writes/artifact-updated.json");
const REFUSED: &str = include_str!("../../github/tests/fixtures/writes/refused.json");
const PERMISSION: &str = include_str!("../../github/tests/fixtures/reads/permission.json");
const PERMISSION_NONE: &str =
    include_str!("../../github/tests/fixtures/reads/permission-none.json");
const PARENT_NONE: &str = include_str!("../../github/tests/fixtures/reads/parent-none.json");
const COMMENT_ID: u64 = 7_202_074_597;
const CREATED: &str = r#"{
  "id": 9001, "html_url": "https://github.com/acme/widgets/issues/7#issuecomment-9001",
  "user": {"login": "mintswarm-dev[bot]", "type": "Bot"},
  "body": "<!-- mintswarm: note -->\n<!-- mintswarm: blockers open=12 -->\nBlocked by #12.",
  "created_at": "2026-11-26T12:00:02Z", "updated_at": "2026-11-26T12:00:02Z"
}"#;

/// A token source that hands out one fixed token.
struct FixedTokens;

impl InstallationTokens for FixedTokens {
    async fn token(
        &self,
        _owner: &str,
        _repo: &str,
        _permissions: &Permissions,
    ) -> Result<Token, TokenError> {
        Ok(Token::new(TOKEN.to_owned(), u64::MAX))
    }
}

/// A single attempt per request, so each request takes one scripted reply.
fn single_attempt() -> RetryPolicy {
    RetryPolicy {
        attempts: 1,
        ..RetryPolicy::default()
    }
}

fn tracker(responder: &Responder) -> GitHubTracker<FixedTokens> {
    let base = responder.base.as_str();
    let clock = Clock::system();
    let limiter = Arc::new(ContentLimiter::new(clock.clone()));
    let clients = GitHubClients {
        reads: GitHubReads::new(
            base,
            "acme",
            "widgets",
            Kinds::issue_types([(IssueType::Implementation, "Task".to_owned())]),
            vec![Login("maintainer".into())],
            Login(BOT.into()),
        )
        .unwrap()
        .with_retry(single_attempt()),
        fields: FieldClient::new(base, "acme", "widgets")
            .unwrap()
            .with_retry(single_attempt()),
        statuses: StatusClient::new(base, "acme", "widgets")
            .unwrap()
            .with_retry(single_attempt()),
        comments: CommentClient::new(
            base,
            "acme",
            "widgets",
            Login(BOT.into()),
            Arc::clone(&limiter),
            clock,
        )
        .unwrap()
        .with_retry(single_attempt()),
        writes: IssueWrites::new(base, "acme", "widgets", limiter)
            .unwrap()
            .with_retry(single_attempt()),
    };
    GitHubTracker::new(base, "acme", "widgets", FixedTokens, clients)
}

fn effort() -> FieldRef {
    FieldRef {
        source: FieldSource::Field {
            name: "Effort".into(),
        },
        options: ["XS", "S", "M", "L"].map(String::from).to_vec(),
    }
}

fn value(field_id: u64, option_id: u64, name: &str) -> FieldRead {
    FieldRead::Value(FieldValue {
        field_id: Some(field_id),
        option_id: Some(option_id),
        name: name.into(),
    })
}

fn summary(requests: &[Request]) -> Vec<String> {
    requests
        .iter()
        .map(|request| format!("{} {}", request.method, request.path))
        .collect()
}

fn writes(requests: &[Request]) -> Vec<String> {
    summary(
        &requests
            .iter()
            .filter(|request| ["POST", "PATCH", "PUT", "DELETE"].contains(&request.method.as_str()))
            .cloned()
            .collect::<Vec<_>>(),
    )
}

#[tokio::test]
async fn learns_the_field_id_from_a_values_read() {
    let responder = Responder::start().await;
    responder.reply(200, VALUES);
    let tracker = tracker(&responder);
    assert_eq!(
        tracker.read_field(ISSUE, &effort()).await,
        Ok(value(101, 1003, "M"))
    );
    let requests = responder.requests();
    assert_eq!(summary(&requests), [format!("GET {VALUES_PATH}")]);
    assert!(
        requests
            .iter()
            .all(|request| !request.path.starts_with("/orgs/"))
    );
    assert_eq!(
        requests[0].header("authorization"),
        Some(format!("Bearer {TOKEN}").as_str())
    );
}

#[tokio::test]
async fn reuses_the_learned_id_on_a_later_write() {
    let responder = Responder::start().await;
    responder.reply(200, VALUES);
    responder.reply(200, WRITTEN_101);
    let tracker = tracker(&responder);
    tracker.read_field(ISSUE, &effort()).await.unwrap();
    tracker
        .set_field(ISSUE, &effort(), Some("L"))
        .await
        .unwrap();
    let requests = responder.requests();
    assert_eq!(
        summary(&requests),
        [format!("GET {VALUES_PATH}"), format!("POST {VALUES_PATH}")]
    );
    assert!(
        requests[1].body.contains(r#""field_id":101"#),
        "{}",
        requests[1].body
    );
    assert!(
        requests[1].body.contains(r#""value":"L""#),
        "{}",
        requests[1].body
    );
}

#[tokio::test]
async fn forgets_the_id_after_field_recreated() {
    let responder = Responder::start().await;
    responder.reply(200, VALUES);
    responder.reply(200, RECREATED);
    responder.reply(200, RECREATED);
    responder.reply(200, WRITTEN_909);
    let tracker = tracker(&responder);
    tracker.read_field(ISSUE, &effort()).await.unwrap();
    let error = tracker.read_field(ISSUE, &effort()).await.unwrap_err();
    assert_eq!(error, FieldError::Field(FieldFinding::FieldRecreated));
    assert_eq!(
        tracker.read_field(ISSUE, &effort()).await,
        Ok(value(909, 9003, "M"))
    );
    tracker
        .set_field(ISSUE, &effort(), Some("L"))
        .await
        .unwrap();
    let requests = responder.requests();
    assert_eq!(
        summary(&requests),
        [
            format!("GET {VALUES_PATH}"),
            format!("GET {VALUES_PATH}"),
            format!("GET {VALUES_PATH}"),
            format!("POST {VALUES_PATH}"),
        ]
    );
    assert!(
        requests[3].body.contains(r#""field_id":909"#),
        "{}",
        requests[3].body
    );
}

#[tokio::test]
async fn a_refused_catalogue_read_defers_and_writes_nothing() {
    let responder = Responder::start().await;
    for _ in 0..3 {
        responder.reply(200, "[]");
        responder.reply(403, FORBIDDEN);
    }
    let tracker = tracker(&responder);
    let error = tracker
        .set_field(ISSUE, &effort(), Some("L"))
        .await
        .unwrap_err();
    assert_eq!(error, FieldError::Field(FieldFinding::FieldMissing));
    assert!(error.should_resolve_again());
    let error = tracker.read_field(ISSUE, &effort()).await.unwrap_err();
    assert_eq!(error, FieldError::Field(FieldFinding::FieldMissing));
    // Nothing was cached: a later write goes to the catalogue again.
    let error = tracker
        .set_field(ISSUE, &effort(), Some("L"))
        .await
        .unwrap_err();
    assert_eq!(error, FieldError::Field(FieldFinding::FieldMissing));
    let requests = responder.requests();
    let deferred = [
        format!("GET {VALUES_PATH}"),
        format!("GET {ORG_FIELDS_PATH}"),
    ];
    assert_eq!(
        summary(&requests),
        deferred.iter().cycle().take(6).cloned().collect::<Vec<_>>()
    );
    assert!(writes(&requests).is_empty(), "{:?}", writes(&requests));
}

#[tokio::test]
async fn a_catalogue_id_is_used_and_kept_when_the_issue_shows_none() {
    let responder = Responder::start().await;
    responder.reply(200, "[]");
    responder.reply(
        200,
        r#"[{"id":202,"name":"Priority"},{"id":101,"name":"Effort"}]"#,
    );
    responder.reply(200, WRITTEN_101);
    responder.reply(200, WRITTEN_101);
    let tracker = tracker(&responder);
    tracker
        .set_field(ISSUE, &effort(), Some("L"))
        .await
        .unwrap();
    tracker
        .set_field(ISSUE, &effort(), Some("L"))
        .await
        .unwrap();
    let requests = responder.requests();
    assert_eq!(
        summary(&requests),
        [
            format!("GET {VALUES_PATH}"),
            format!("GET {ORG_FIELDS_PATH}"),
            format!("POST {VALUES_PATH}"),
            format!("POST {VALUES_PATH}"),
        ]
    );
    assert!(
        requests[2].body.contains(r#""field_id":101"#),
        "{}",
        requests[2].body
    );
}

#[tokio::test]
async fn a_labels_role_needs_no_id() {
    let responder = Responder::start().await;
    responder.reply(200, r#"{"number":7,"labels":[{"name":"effort:M"}]}"#);
    let field = FieldRef {
        source: FieldSource::Labels {
            label: "effort:{option}".into(),
        },
        options: ["S", "M", "L"].map(String::from).to_vec(),
    };
    let read = tracker(&responder).read_field(ISSUE, &field).await;
    assert_eq!(
        read,
        Ok(FieldRead::Value(FieldValue {
            field_id: None,
            option_id: None,
            name: "M".into(),
        }))
    );
    assert_eq!(
        summary(&responder.requests()),
        ["GET /repos/acme/widgets/issues/7"]
    );
}

#[tokio::test]
async fn unsupported_operations_send_nothing() {
    let responder = Responder::start().await;
    let tracker = tracker(&responder);
    let item = SubItem {
        title: "A child".into(),
        body: String::new(),
        labels: Vec::new(),
        issue_type: None,
    };
    let errors = [
        tracker.fetch_ready().await.map(|_| ()).unwrap_err(),
        tracker
            .create_sub_item(ISSUE, item)
            .await
            .map(|_| ())
            .unwrap_err(),
        tracker
            .set_issue_type(ISSUE, IssueType::Plan)
            .await
            .unwrap_err(),
        tracker
            .link_dependency(ISSUE, IssueNumber(8))
            .await
            .unwrap_err(),
    ];
    let operations: Vec<_> = errors
        .iter()
        .map(|error| match error {
            GitHubTrackerError::Unsupported { operation } => *operation,
            other => panic!("not unsupported: {other:?}"),
        })
        .collect();
    assert_eq!(
        operations,
        [
            "fetch_ready",
            "create_sub_item",
            "set_issue_type",
            "link_dependency"
        ]
    );
    assert!(responder.requests().is_empty());
}

#[tokio::test]
async fn a_comment_without_a_key_is_refused_before_any_request() {
    let responder = Responder::start().await;
    let error = tracker(&responder)
        .post_comment(ISSUE, format!("{NOTE_MARKER}\nNo key here."))
        .await
        .unwrap_err();
    assert!(
        matches!(error, GitHubTrackerError::Comment(CommentError::Unmarked)),
        "{error:?}"
    );
    assert!(responder.requests().is_empty());
}

#[tokio::test]
async fn a_keyed_comment_is_posted_once() {
    let responder = Responder::start().await;
    responder.reply(201, CREATED);
    let body = format!("{NOTE_MARKER}\n<!-- mintswarm: blockers open=12 -->\nBlocked by #12.");
    let comment = tracker(&responder)
        .post_comment(ISSUE, body.clone())
        .await
        .unwrap();
    assert_eq!(comment.id, 9001);
    let requests = responder.requests();
    assert_eq!(
        summary(&requests),
        ["POST /repos/acme/widgets/issues/7/comments"]
    );
    assert!(
        requests[0].body.contains("blockers open=12"),
        "{}",
        requests[0].body
    );
}

#[tokio::test]
async fn reads_an_issue_with_its_comments() {
    let responder = Responder::start().await;
    responder.reply(200, ISSUE_JSON);
    responder.reply(200, COMMENTS_JSON);
    let read = tracker(&responder).read_issue(ISSUE, None).await.unwrap();
    assert_eq!(read.issue.number, ISSUE);
    assert_eq!(read.issue.title, "feat(core): ingest tracker events");
    assert_eq!(read.issue.issue_type, Some(IssueType::Implementation));
    assert_eq!(read.issue.blocked_by, 1);
    assert_eq!(read.comments.len(), 1);
    assert_eq!(read.comments[0].id, 501);
    assert_eq!(read.cursor, Cursor("2026-11-22T08:00:00Z".into()));
}

#[tokio::test]
async fn reads_links_through_the_reads_client() {
    let responder = Responder::start().await;
    responder.reply(404, r#"{"message":"Not Found"}"#);
    responder.reply(200, r#"[{"id":1,"number":8}]"#);
    let links = tracker(&responder).read_links(ISSUE).await.unwrap();
    assert_eq!(links.len(), 1);
    assert_eq!(links[0].number, 8);
}

#[tokio::test]
async fn each_status_call_opens_its_own_pass() {
    let responder = Responder::start().await;
    responder.reply(200, ISSUE_JSON);
    for _ in 0..2 {
        responder.reply(200, ISSUE_JSON);
        responder.reply(200, STATUSES);
        responder.reply(200, SET_STATUS);
    }
    let tracker = tracker(&responder);
    assert_eq!(
        tracker.read_status(ISSUE).await.unwrap().as_deref(),
        Some("Queued")
    );
    tracker.set_status(ISSUE, "Done".into()).await.unwrap();
    tracker.set_status(ISSUE, "Done".into()).await.unwrap();
    let set = [
        String::from("GET /repos/acme/widgets/issues/7"),
        String::from("POST /graphql"),
        String::from("POST /graphql"),
    ];
    let mut expected = vec![String::from("GET /repos/acme/widgets/issues/7")];
    expected.extend(set.iter().cycle().take(6).cloned());
    assert_eq!(summary(&responder.requests()), expected);
}

/// Asserts that `request` is `method path`, sent with the tracker's token.
fn assert_sent(request: &Request, method: &str, path: &str) {
    assert_eq!(
        (request.method.as_str(), request.path.as_str()),
        (method, path)
    );
    assert_eq!(
        request.header("authorization"),
        Some(format!("Bearer {TOKEN}").as_str())
    );
}

#[tokio::test]
async fn reacts_plus_one_to_a_comment() {
    let responder = Responder::start().await;
    responder.reply(201, REACTION);
    tracker(&responder)
        .react_plus_one(COMMENT_ID)
        .await
        .unwrap();
    let requests = responder.requests();
    assert_eq!(requests.len(), 1);
    assert_sent(
        &requests[0],
        "POST",
        &format!("/repos/acme/widgets/issues/comments/{COMMENT_ID}/reactions"),
    );
    assert!(
        requests[0].body.contains(r#""content":"+1""#),
        "{}",
        requests[0].body
    );
}

#[tokio::test]
async fn edits_a_comment_and_returns_it() {
    let responder = Responder::start().await;
    responder.reply(200, COMMENT_EDITED);
    let body = "<!-- mintswarm: note -->\nScratch comment by mintswarm-dev, edited by the steward.";
    let comment = tracker(&responder)
        .edit_comment(COMMENT_ID, body.to_owned())
        .await
        .unwrap();
    assert_eq!(comment.id, COMMENT_ID);
    assert_eq!(comment.body, body);
    assert_eq!(comment.updated_at, "2026-11-26T19:54:10Z");
    let requests = responder.requests();
    assert_eq!(requests.len(), 1);
    assert_sent(
        &requests[0],
        "PATCH",
        &format!("/repos/acme/widgets/issues/comments/{COMMENT_ID}"),
    );
    assert!(
        requests[0].body.contains("edited by the steward"),
        "{}",
        requests[0].body
    );
}

#[tokio::test]
async fn updates_an_artifact_by_its_number() {
    let responder = Responder::start().await;
    responder.reply(200, ARTIFACT_UPDATED);
    let body = "# Scratch artefact\n\nVersion 2, by the steward.";
    let artifact = tracker(&responder)
        .update_artifact(ISSUE, 1, body.to_owned())
        .await
        .unwrap();
    assert_eq!(artifact.name, "Scratch artefact");
    assert_eq!(artifact.body, body);
    let requests = responder.requests();
    assert_eq!(requests.len(), 1);
    assert_sent(
        &requests[0],
        "PATCH",
        "/repos/acme/widgets/issues/7/artifacts/1",
    );
}

#[tokio::test]
async fn a_refused_write_is_kept_as_refused() {
    let responder = Responder::start().await;
    responder.reply(403, REFUSED);
    responder.reply(403, REFUSED);
    responder.reply(403, REFUSED);
    let tracker = tracker(&responder);
    let errors = [
        tracker.react_plus_one(COMMENT_ID).await.unwrap_err(),
        tracker
            .edit_comment(COMMENT_ID, "Edited.".into())
            .await
            .map(|_| ())
            .unwrap_err(),
        tracker
            .update_artifact(ISSUE, 1, "Updated.".into())
            .await
            .map(|_| ())
            .unwrap_err(),
    ];
    for error in &errors {
        assert!(
            matches!(error, GitHubTrackerError::Write(WriteError::Refused { .. })),
            "{error:?}"
        );
    }
    assert_eq!(responder.requests().len(), 3);
}

#[tokio::test]
async fn reads_a_collaborator_permission() {
    let responder = Responder::start().await;
    responder.reply(200, PERMISSION);
    responder.reply(404, PERMISSION_NONE);
    let tracker = tracker(&responder);
    assert_eq!(
        tracker.read_permission(Login("zmij".into())).await.unwrap(),
        RepoRole::Admin
    );
    assert_eq!(
        tracker
            .read_permission(Login("no-such-user-msw-0001".into()))
            .await
            .unwrap(),
        RepoRole::NoAccess
    );
    let requests = responder.requests();
    assert_eq!(requests.len(), 2);
    assert_sent(
        &requests[0],
        "GET",
        "/repos/acme/widgets/collaborators/zmij/permission",
    );
    assert_sent(
        &requests[1],
        "GET",
        "/repos/acme/widgets/collaborators/no-such-user-msw-0001/permission",
    );
}

#[tokio::test]
async fn reads_an_issue_parent() {
    let responder = Responder::start().await;
    responder.reply(404, PARENT_NONE);
    responder.reply(
        200,
        r#"{"id": 3000548, "number": 548, "title": "The parent"}"#,
    );
    let tracker = tracker(&responder);
    assert_eq!(tracker.read_parent(ISSUE).await.unwrap(), None);
    assert_eq!(
        tracker.read_parent(ISSUE).await.unwrap(),
        Some(IssueNumber(548))
    );
    let requests = responder.requests();
    assert_eq!(requests.len(), 2);
    for request in &requests {
        assert_sent(request, "GET", "/repos/acme/widgets/issues/7/parent");
    }
}

// The scripted responder, as in `crates/github/tests/support/mod.rs`.

/// A request the responder received.
#[derive(Clone, Debug)]
pub struct Request {
    pub method: String,
    pub path: String,
    /// Header names in lower case, with their values.
    pub headers: Vec<(String, String)>,
    pub body: String,
}

impl Request {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }
}

/// A scripted reply: its status and body.
type Reply = (u16, String);

/// An HTTP/1.1 responder on a local port that serves one connection at a time, records each
/// request and answers from a script of `(status, body)` replies, one per request.
pub struct Responder {
    pub base: String,
    requests: Arc<Mutex<Vec<Request>>>,
    script: Arc<Mutex<Vec<Reply>>>,
}

impl Responder {
    pub async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("cannot bind the responder");
        let base = format!(
            "http://{}",
            listener.local_addr().expect("the responder has no address")
        );
        let requests = Arc::new(Mutex::new(Vec::new()));
        let script: Arc<Mutex<Vec<Reply>>> = Arc::new(Mutex::new(Vec::new()));
        let (recorded, replies) = (Arc::clone(&requests), Arc::clone(&script));
        tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                let Some(request) = read_request(&mut stream).await else {
                    continue;
                };
                recorded.lock().expect("poisoned").push(request);
                let (status, body) = {
                    let mut replies = replies.lock().expect("poisoned");
                    if replies.is_empty() {
                        (500, String::from("{\"message\":\"no reply scripted\"}"))
                    } else {
                        replies.remove(0)
                    }
                };
                let response = format!(
                    "HTTP/1.1 {status} Scripted\r\nContent-Type: application/json\r\n\
                     Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes()).await;
                let _ = stream.shutdown().await;
            }
        });
        Self {
            base,
            requests,
            script,
        }
    }

    /// Queues a reply.
    pub fn reply(&self, status: u16, body: impl Into<String>) {
        self.script
            .lock()
            .expect("poisoned")
            .push((status, body.into()));
    }

    pub fn requests(&self) -> Vec<Request> {
        self.requests.lock().expect("poisoned").clone()
    }
}

async fn read_request(stream: &mut tokio::net::TcpStream) -> Option<Request> {
    let mut data = Vec::new();
    let mut buffer = [0u8; 4096];
    let head_end = loop {
        if let Some(at) = data.windows(4).position(|w| w == b"\r\n\r\n") {
            break at;
        }
        let read = stream.read(&mut buffer).await.ok()?;
        if read == 0 {
            return None;
        }
        data.extend_from_slice(&buffer[..read]);
    };
    let head = String::from_utf8(data[..head_end].to_vec()).ok()?;
    let mut lines = head.split("\r\n");
    let mut start = lines.next()?.split(' ');
    let method = start.next()?.to_owned();
    let path = start.next()?.to_owned();
    let headers: Vec<(String, String)> = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.trim().to_ascii_lowercase(), value.trim().to_owned()))
        .collect();
    let length: usize = headers
        .iter()
        .find(|(name, _)| name == "content-length")
        .and_then(|(_, value)| value.parse().ok())
        .unwrap_or(0);
    let mut body = data[head_end + 4..].to_vec();
    while body.len() < length {
        let read = stream.read(&mut buffer).await.ok()?;
        if read == 0 {
            break;
        }
        body.extend_from_slice(&buffer[..read]);
    }
    Some(Request {
        method,
        path,
        headers,
        body: String::from_utf8_lossy(&body).into_owned(),
    })
}
