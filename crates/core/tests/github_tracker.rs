//! The GitHub tracker against a scripted local server answering with JSON in the shapes of the
//! `mintswarm-github` fixtures: field ids learned, reused, forgotten and deferred, and the other
//! capabilities over their clients.
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
    Comments, Cursor, Fields, IssueType, Issues, Links, Login, Statuses, SubItem,
};
use mintswarm_github::{
    Clock, CommentClient, CommentError, ContentLimiter, FieldClient, GitHubReads, Kinds,
    NOTE_MARKER, Permissions, RetryPolicy, StatusClient, Token, TokenError,
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
            Arc::new(ContentLimiter::new(clock.clone())),
            clock,
        )
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

/// A scripted reply: its status, extra response headers and body. Status [`DROP`] closes the
/// connection without an answer.
type Reply = (u16, Vec<(String, String)>, String);

/// The scripted status that drops the connection after reading the request.
const DROP: u16 = 0;

/// An HTTP/1.1 responder on a local port that serves one connection at a time, records each
/// request and answers from a script of `(status, headers, body)` replies, one per request.
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
                let (status, headers, body) = {
                    let mut replies = replies.lock().expect("poisoned");
                    if replies.is_empty() {
                        (
                            500,
                            Vec::new(),
                            String::from("{\"message\":\"no reply scripted\"}"),
                        )
                    } else {
                        replies.remove(0)
                    }
                };
                if status == DROP {
                    drop(stream);
                    continue;
                }
                let mut response = format!("HTTP/1.1 {status} Scripted\r\n");
                for (name, value) in &headers {
                    response.push_str(&format!("{name}: {value}\r\n"));
                }
                if status == 304 {
                    // A 304 has no body and no length.
                    response.push_str("Connection: close\r\n\r\n");
                } else {
                    response.push_str(&format!(
                        "Content-Type: application/json\r\nContent-Length: {}\r\n\
                         Connection: close\r\n\r\n{body}",
                        body.len()
                    ));
                }
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

    /// Queues a reply with no extra headers.
    pub fn reply(&self, status: u16, body: impl Into<String>) {
        self.reply_with(status, &[], body);
    }

    /// Queues a reply with extra response headers, such as `ETag` or `retry-after`.
    pub fn reply_with(&self, status: u16, headers: &[(&str, &str)], body: impl Into<String>) {
        let headers = headers
            .iter()
            .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
            .collect();
        self.script
            .lock()
            .expect("poisoned")
            .push((status, headers, body.into()));
    }

    /// Queues a dropped connection: the request is read and the connection closed unanswered.
    pub fn drop_connection(&self) {
        self.reply(DROP, "");
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
