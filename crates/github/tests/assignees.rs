//! An issue's assignees and `assigned` events read, its assignees added and removed, and the
//! release rule, against a scripted local server answering with hand-written fixtures in the
//! shape of GitHub's issues, assignees and issue events API.
// Not redundant: clippy's `allow-unwrap-in-tests` covers a `tests/` crate only under `cfg(test)`.
#![cfg(test)]

mod support;

use std::sync::Arc;
use std::time::Duration;

use mintswarm_domain::ids::IssueNumber;
use mintswarm_domain::tracker::Login;
use mintswarm_github::{
    AssignedEvent, Clock, ContentLimiter, GitHubReads, IssueWrites, Kinds, ReadError, RetryPolicy,
    Token, releasable,
};
use support::{Request, Responder};

const TOKEN: &str = "ghs_ass1gneesT0kenV4lue";
const ISSUE_JSON: &str = include_str!("fixtures/assignees/issue.json");
const EVENTS: &str = include_str!("fixtures/assignees/events.json");
const ASSIGNED: &str = include_str!("fixtures/assignees/assigned.json");
const RELEASED: &str = include_str!("fixtures/assignees/released.json");
const ISSUE: IssueNumber = IssueNumber(7);
const ISSUE_PATH: &str = "/repos/acme/widgets/issues/7";
const EVENTS_PATH: &str = "/repos/acme/widgets/issues/7/events?per_page=100&page=1";
const ASSIGNEES_PATH: &str = "/repos/acme/widgets/issues/7/assignees";
/// The plane's own logins in these tests: an App's bot login, and a login without `[bot]`.
const PLANE_BOT: &str = "widgets-plane[bot]";
const PLANE_LOGIN: &str = "plane-operator";

fn token() -> Token {
    Token::new(TOKEN.to_owned(), 0)
}

fn logins(names: &[&str]) -> Vec<Login> {
    names.iter().map(|name| Login((*name).to_owned())).collect()
}

/// A single attempt per request, so each takes one scripted reply.
fn single_attempt() -> RetryPolicy {
    RetryPolicy {
        attempts: 1,
        ..RetryPolicy::default()
    }
}

/// Two attempts with millisecond waits, so a 502 is retried once.
fn two_attempts() -> RetryPolicy {
    RetryPolicy {
        attempts: 2,
        base_delay: Duration::from_millis(1),
        max_delay: Duration::from_millis(1),
        ..RetryPolicy::default()
    }
}

fn reads(responder: &Responder) -> GitHubReads {
    GitHubReads::new(
        &responder.base,
        "acme",
        "widgets",
        Kinds::labels([]),
        Vec::new(),
        Login(PLANE_BOT.into()),
    )
    .unwrap()
    .with_retry(single_attempt())
}

fn writes(responder: &Responder, policy: RetryPolicy) -> IssueWrites {
    let limiter = Arc::new(ContentLimiter::new(Clock::system()));
    IssueWrites::new(&responder.base, "acme", "widgets", limiter)
        .unwrap()
        .with_retry(policy)
}

fn body(request: &Request) -> serde_json::Value {
    serde_json::from_str(&request.body).unwrap()
}

/// The issue's assignees and `assigned` events, read from the fixtures.
async fn fixture_reads() -> (Vec<Login>, Vec<AssignedEvent>) {
    let responder = Responder::start().await;
    responder.reply(200, ISSUE_JSON);
    responder.reply(200, EVENTS);
    let reads = reads(&responder);
    let assignees = reads.read_assignees(&token(), ISSUE).await.unwrap();
    let events = reads.read_assigned_events(&token(), ISSUE).await.unwrap();
    (assignees, events)
}

fn event(assignee: &str, actor: &str, created_at: &str, id: u64) -> AssignedEvent {
    AssignedEvent {
        assignee: Login(assignee.into()),
        actor: Some(Login(actor.into())),
        assigner: Some(Login(actor.into())),
        app: None,
        created_at: Some(created_at.into()),
        id: Some(id),
    }
}

#[tokio::test]
async fn reads_the_assignees_and_the_assigned_events() {
    let responder = Responder::start().await;
    responder.reply(200, ISSUE_JSON);
    responder.reply(200, EVENTS);
    let reads = reads(&responder);
    assert_eq!(
        reads.read_assignees(&token(), ISSUE).await,
        Ok(logins(&[
            "alice", "bob", "carol", "dave", "erin", "frank", "grace"
        ]))
    );
    let events = reads.read_assigned_events(&token(), ISSUE).await.unwrap();
    // The labelled event and the assigned event without an assignee are left out.
    assert_eq!(
        events
            .iter()
            .map(|event| event.id.unwrap())
            .collect::<Vec<_>>(),
        [5002, 5003, 5004, 5005, 5006, 5007, 5008]
    );
    assert_eq!(
        events[0],
        AssignedEvent {
            assignee: Login("alice".into()),
            actor: Some(Login(PLANE_BOT.into())),
            assigner: Some(Login(PLANE_BOT.into())),
            app: Some("widgets-plane".into()),
            created_at: Some("2026-11-20T10:00:00Z".into()),
            id: Some(5002),
        }
    );
    assert_eq!(events[1].app, None);
    let requests = responder.requests();
    assert_eq!(requests.len(), 2);
    assert!(requests.iter().all(|request| request.method == "GET"));
    assert_eq!(requests[0].path, ISSUE_PATH);
    assert_eq!(requests[1].path, EVENTS_PATH);
}

#[tokio::test]
async fn an_issue_without_assignees_has_none() {
    let responder = Responder::start().await;
    responder.reply(200, r#"{"number": 7, "assignees": []}"#);
    responder.reply(200, r#"{"number": 7}"#);
    let reads = reads(&responder);
    assert_eq!(reads.read_assignees(&token(), ISSUE).await, Ok(Vec::new()));
    assert_eq!(reads.read_assignees(&token(), ISSUE).await, Ok(Vec::new()));
}

#[tokio::test]
async fn events_past_the_page_cap_fail() {
    let responder = Responder::start().await;
    let url = "https://api.github.com/repositories/1/issues/7/events?per_page=100";
    for page in 1..=30 {
        let link = format!("<{url}&page={}>; rel=\"next\"", page + 1);
        responder.reply_with(200, &[("link", link.as_str())], "[]");
    }
    let error = reads(&responder)
        .read_assigned_events(&token(), ISSUE)
        .await
        .unwrap_err();
    assert!(matches!(error, ReadError::TooManyPages { .. }), "{error:?}");
    assert_eq!(responder.requests().len(), 30);
}

#[tokio::test]
async fn adds_assignees_and_answers_with_the_issue_s() {
    let responder = Responder::start().await;
    responder.reply(201, ASSIGNED);
    let added = writes(&responder, single_attempt())
        .add_assignees(&token(), ISSUE, &logins(&["heidi"]))
        .await;
    assert_eq!(
        added,
        Ok(logins(&[
            "alice", "bob", "carol", "dave", "erin", "frank", "grace", "heidi"
        ]))
    );
    let requests = responder.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].method, "POST");
    assert_eq!(requests[0].path, ASSIGNEES_PATH);
    assert_eq!(
        body(&requests[0]),
        serde_json::json!({ "assignees": ["heidi"] })
    );
}

#[tokio::test]
async fn removes_assignees_with_their_body_also_after_a_retry() {
    let responder = Responder::start().await;
    responder.reply(502, r#"{"message":"Bad Gateway"}"#);
    responder.reply(200, RELEASED);
    let removed = writes(&responder, two_attempts())
        .remove_assignees(&token(), ISSUE, &logins(&["alice", "erin", "grace"]))
        .await;
    assert_eq!(removed, Ok(logins(&["bob", "carol", "dave", "frank"])));
    let requests = responder.requests();
    assert_eq!(requests.len(), 2);
    for request in &requests {
        assert_eq!(request.method, "DELETE");
        assert_eq!(request.path, ASSIGNEES_PATH);
        assert_eq!(
            body(request),
            serde_json::json!({ "assignees": ["alice", "erin", "grace"] })
        );
    }
}

#[tokio::test]
async fn releases_the_own_assignments_and_keeps_a_person_s() {
    let (assignees, events) = fixture_reads().await;
    // alice: assigned by the plane's bot login. erin: through the App that login names, by
    // someone else, its slug in another case. grace: by the other own login, in another case.
    // Kept: bob (a person assigned him), carol (no event), dave (re-assigned by a person after
    // the plane), frank (through an App no own `[bot]` login names).
    assert_eq!(
        releasable(&assignees, &events, &logins(&[PLANE_BOT, PLANE_LOGIN])),
        logins(&["alice", "erin", "grace"])
    );
}

#[tokio::test]
async fn no_own_logins_release_none() {
    let (assignees, events) = fixture_reads().await;
    assert_eq!(releasable(&assignees, &events, &[]), Vec::<Login>::new());
}

#[tokio::test]
async fn an_own_login_without_bot_names_no_app() {
    let (assignees, events) = fixture_reads().await;
    // `widgets-plane` is no `[bot]` login, so alice's and erin's App slug is not its; nobody
    // acted as it.
    assert_eq!(
        releasable(&assignees, &events, &logins(&["widgets-plane"])),
        Vec::<Login>::new()
    );
}

#[test]
fn the_latest_event_wins_whatever_the_order_given() {
    let assignees = logins(&["dave", "ivan"]);
    // Listed out of order: dave was assigned by the plane last, ivan by a person last; ids
    // break the tie on equal times.
    let events = [
        event("dave", PLANE_BOT, "2026-11-21T08:00:00Z", 2),
        event("ivan", PLANE_BOT, "2026-11-21T08:00:00Z", 3),
        event("Dave", "zmij", "2026-11-20T08:00:00Z", 1),
        event("ivan", "zmij", "2026-11-21T08:00:00Z", 4),
    ];
    assert_eq!(
        releasable(&assignees, &events, &logins(&[PLANE_BOT])),
        logins(&["dave"])
    );
}

#[test]
fn the_result_is_sorted_without_repeats() {
    let assignees = logins(&["zed", "amy", "zed"]);
    let events = [
        event("zed", PLANE_BOT, "2026-11-21T08:00:00Z", 1),
        event("amy", PLANE_BOT, "2026-11-21T08:00:00Z", 2),
    ];
    assert_eq!(
        releasable(&assignees, &events, &logins(&[PLANE_BOT])),
        logins(&["amy", "zed"])
    );
}
