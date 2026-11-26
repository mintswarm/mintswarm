//! Issue status reads and writes against a scripted local server answering with hand-written
//! fixtures in the shape of GitHub's REST issue and its GraphQL `planningStatuses` and
//! `setIssueStatus`.
// Not redundant: clippy's `allow-unwrap-in-tests` covers a `tests/` crate only under `cfg(test)`.
#![cfg(test)]

mod support;

use std::time::Duration;

use mintswarm_domain::ids::IssueNumber;
use mintswarm_github::{
    Exhausted, Failure, Permissions, RetryPolicy, StatusClient, StatusError, Token,
};
use support::{Request, Responder};

const TOKEN: &str = "ghs_st4tusT0kenV4lue";
const ISSUE: &str = include_str!("fixtures/status/issue.json");
const STATUSES: &str = include_str!("fixtures/status/planning-statuses.json");
const NODE_7: &str = "I_kwDOAcmeWidgets7";
const NODE_8: &str = "I_kwDOAcmeWidgets8";
const BLOCKED_ID: &str = "PS_kwDOAcmeBlocked";
const DONE_ID: &str = "PS_kwDOAcmeDone";
/// The mutation's answer.
const SET: &str = r#"{"data":{"setIssueStatus":{"clientMutationId":null}}}"#;
/// A GraphQL refusal that is not a rate limit.
const NOT_FOUND: &str = r#"{"data":null,"errors":[{"type":"NOT_FOUND","path":["node"],"message":"Could not resolve"}]}"#;
/// A GraphQL rate limit.
const RATE_LIMITED: &str =
    r#"{"data":null,"errors":[{"type":"RATE_LIMITED","message":"API rate limit exceeded"}]}"#;

fn token() -> Token {
    Token::new(TOKEN.to_owned(), 0)
}

fn single_attempt() -> RetryPolicy {
    RetryPolicy {
        attempts: 1,
        ..RetryPolicy::default()
    }
}

fn attempts(attempts: u32) -> RetryPolicy {
    RetryPolicy {
        attempts,
        base_delay: Duration::from_millis(1),
        max_delay: Duration::from_millis(1),
        ..RetryPolicy::default()
    }
}

fn client(responder: &Responder, policy: RetryPolicy) -> StatusClient {
    StatusClient::new(&responder.base, "acme", "widgets")
        .unwrap()
        .with_retry(policy)
}

/// The issue fixture as issue `number` with `node_id` and `status`.
fn issue(number: u64, node_id: &str, status: serde_json::Value) -> String {
    let mut issue: serde_json::Value = serde_json::from_str(ISSUE).unwrap();
    issue["number"] = number.into();
    issue["node_id"] = node_id.into();
    issue["status"] = status;
    issue.to_string()
}

fn body(request: &Request) -> serde_json::Value {
    serde_json::from_str(&request.body).unwrap()
}

/// Whether `request` is a GraphQL request whose query names `field`.
fn is_graphql(request: &Request, field: &str) -> bool {
    request.method == "POST"
        && request.path == "/graphql"
        && body(request)["query"]
            .as_str()
            .is_some_and(|query| query.contains(field))
}

#[test]
fn a_pass_token_asks_for_issues_write_and_metadata_read() {
    assert_eq!(
        StatusClient::permissions(),
        Permissions::write(["issues"]).and(Permissions::read(["metadata"]))
    );
}

#[tokio::test]
async fn the_read_gives_the_status_name() {
    let responder = Responder::start().await;
    responder.reply(200, ISSUE);
    responder.reply(200, issue(8, NODE_8, serde_json::Value::Null));
    let client = client(&responder, single_attempt());
    let pass = client.pass(&token());
    assert_eq!(
        pass.read_status(IssueNumber(7)).await,
        Ok(Some("Queued".to_owned()))
    );
    assert_eq!(pass.read_status(IssueNumber(8)).await, Ok(None));
    // A second read in the pass is served from it.
    assert_eq!(
        pass.read_status(IssueNumber(7)).await,
        Ok(Some("Queued".to_owned()))
    );
    let requests = responder.requests();
    let paths: Vec<_> = requests
        .iter()
        .map(|request| (request.method.as_str(), request.path.as_str()))
        .collect();
    assert_eq!(
        paths,
        [
            ("GET", "/repos/acme/widgets/issues/7"),
            ("GET", "/repos/acme/widgets/issues/8"),
        ]
    );
    assert_eq!(
        requests[0].header("authorization"),
        Some(format!("Bearer {TOKEN}").as_str())
    );
}

#[tokio::test]
async fn an_issue_without_a_node_id_is_unexpected() {
    let responder = Responder::start().await;
    responder.reply(200, r#"{"number":7,"status":null}"#);
    let client = client(&responder, single_attempt());
    assert!(matches!(
        client.pass(&token()).read_status(IssueNumber(7)).await,
        Err(StatusError::Unexpected { .. })
    ));
}

#[tokio::test]
async fn two_writes_in_a_pass_read_planning_statuses_once() {
    let responder = Responder::start().await;
    responder.reply(200, ISSUE);
    responder.reply(200, STATUSES);
    responder.reply(200, SET);
    responder.reply(
        200,
        issue(8, NODE_8, serde_json::json!({"name": "Blocked"})),
    );
    responder.reply(200, SET);
    let client = client(&responder, single_attempt());
    let pass = client.pass(&token());
    assert_eq!(pass.set_status(IssueNumber(7), "Blocked").await, Ok(()));
    assert_eq!(pass.set_status(IssueNumber(8), "Done").await, Ok(()));

    let requests = responder.requests();
    assert_eq!(requests.len(), 5);
    assert_eq!(
        (requests[0].method.as_str(), requests[0].path.as_str()),
        ("GET", "/repos/acme/widgets/issues/7")
    );
    assert!(is_graphql(&requests[1], "planningStatuses"));
    assert_eq!(
        body(&requests[1])["variables"],
        serde_json::json!({"id": NODE_7})
    );
    assert!(is_graphql(&requests[2], "setIssueStatus"));
    assert_eq!(
        body(&requests[2])["variables"],
        serde_json::json!({"issue": NODE_7, "status": BLOCKED_ID})
    );
    assert_eq!(
        (requests[3].method.as_str(), requests[3].path.as_str()),
        ("GET", "/repos/acme/widgets/issues/8")
    );
    assert!(is_graphql(&requests[4], "setIssueStatus"));
    assert_eq!(
        body(&requests[4])["variables"],
        serde_json::json!({"issue": NODE_8, "status": DONE_ID})
    );
    let queries = requests
        .iter()
        .filter(|request| is_graphql(request, "planningStatuses"))
        .count();
    assert_eq!(queries, 1);
}

#[tokio::test]
async fn a_write_makes_the_next_read_fetch_the_issue_again() {
    let responder = Responder::start().await;
    responder.reply_with(200, &[("etag", "\"i7\"")], ISSUE);
    responder.reply(200, STATUSES);
    responder.reply(200, SET);
    responder.reply(200, issue(7, NODE_7, serde_json::json!({"name": "Done"})));
    let client = client(&responder, single_attempt());
    let pass = client.pass(&token());
    pass.set_status(IssueNumber(7), "Done").await.unwrap();
    assert_eq!(
        pass.read_status(IssueNumber(7)).await,
        Ok(Some("Done".to_owned()))
    );
    let requests = responder.requests();
    assert_eq!(requests.len(), 4);
    assert_eq!(requests[3].header("if-none-match"), None);
}

#[tokio::test]
async fn a_new_pass_reads_planning_statuses_again() {
    let responder = Responder::start().await;
    responder.reply(200, ISSUE);
    responder.reply(200, STATUSES);
    responder.reply(200, SET);
    responder.reply(200, ISSUE);
    responder.reply(200, STATUSES);
    responder.reply(200, SET);
    let client = client(&responder, single_attempt());
    client
        .pass(&token())
        .set_status(IssueNumber(7), "Blocked")
        .await
        .unwrap();
    client
        .pass(&token())
        .set_status(IssueNumber(7), "Done")
        .await
        .unwrap();
    let queries = responder
        .requests()
        .iter()
        .filter(|request| is_graphql(request, "planningStatuses"))
        .count();
    assert_eq!(queries, 2);
}

#[tokio::test]
async fn an_unknown_status_name_sends_no_mutation() {
    let responder = Responder::start().await;
    responder.reply(200, ISSUE);
    responder.reply(200, STATUSES);
    let client = client(&responder, single_attempt());
    assert_eq!(
        client
            .pass(&token())
            .set_status(IssueNumber(7), "blocked")
            .await,
        Err(StatusError::UnknownStatus {
            name: "blocked".to_owned()
        })
    );
    let requests = responder.requests();
    assert_eq!(requests.len(), 2);
    assert!(
        !requests
            .iter()
            .any(|request| is_graphql(request, "setIssueStatus"))
    );
}

#[tokio::test]
async fn a_graphql_error_reply_is_a_failure_and_not_retried() {
    let responder = Responder::start().await;
    responder.reply(200, ISSUE);
    responder.reply(200, NOT_FOUND);
    let client = client(&responder, attempts(3));
    let set = client
        .pass(&token())
        .set_status(IssueNumber(7), "Blocked")
        .await;
    assert_eq!(
        set,
        Err(StatusError::GraphQl {
            endpoint: "POST /graphql".to_owned(),
            types: vec!["NOT_FOUND".to_owned()],
        })
    );
    let graphql = responder
        .requests()
        .iter()
        .filter(|request| request.path == "/graphql")
        .count();
    assert_eq!(graphql, 1);
}

#[tokio::test]
async fn a_graphql_reply_without_data_is_unexpected() {
    let responder = Responder::start().await;
    responder.reply(200, ISSUE);
    responder.reply(200, r#"{"data":null}"#);
    let client = client(&responder, single_attempt());
    assert!(matches!(
        client
            .pass(&token())
            .set_status(IssueNumber(7), "Blocked")
            .await,
        Err(StatusError::Unexpected { .. })
    ));
}

#[tokio::test]
async fn a_graphql_rate_limit_is_a_rate_limit() {
    let responder = Responder::start().await;
    responder.reply(200, ISSUE);
    responder.reply_with(200, &[("retry-after", "1")], RATE_LIMITED);
    let client = client(&responder, single_attempt());
    let set = client
        .pass(&token())
        .set_status(IssueNumber(7), "Blocked")
        .await;
    assert_eq!(
        set,
        Err(StatusError::Exhausted(Exhausted {
            endpoint: "POST /graphql".to_owned(),
            attempts: 1,
            last: Failure::RateLimited {
                wait: Some(Duration::from_secs(1))
            },
        }))
    );
}

#[tokio::test]
async fn a_graphql_rate_limit_is_retried() {
    let responder = Responder::start().await;
    responder.reply(200, ISSUE);
    responder.reply_with(200, &[("retry-after", "1")], RATE_LIMITED);
    responder.reply(200, STATUSES);
    responder.reply(200, SET);
    let client = client(&responder, attempts(2));
    assert_eq!(
        client
            .pass(&token())
            .set_status(IssueNumber(7), "Blocked")
            .await,
        Ok(())
    );
    let requests = responder.requests();
    assert_eq!(requests.len(), 4);
    assert!(is_graphql(&requests[1], "planningStatuses"));
    assert!(is_graphql(&requests[2], "planningStatuses"));
    assert!(is_graphql(&requests[3], "setIssueStatus"));
}

#[tokio::test]
async fn a_refused_graphql_status_is_its_status() {
    let responder = Responder::start().await;
    responder.reply(200, ISSUE);
    responder.reply(401, r#"{"message":"Bad credentials"}"#);
    let client = client(&responder, single_attempt());
    assert_eq!(
        client
            .pass(&token())
            .set_status(IssueNumber(7), "Blocked")
            .await,
        Err(StatusError::Status {
            endpoint: "POST /graphql".to_owned(),
            status: 401,
        })
    );
}
