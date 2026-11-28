//! The `github.webhook` normaliser on recorded deliveries, with personal data replaced, and on
//! values built in the test for inputs no recording has, such as the `_removed` dependency
//! actions; and the sender classifier on a loaded config.

#![cfg(test)]

use std::path::PathBuf;

use mintswarm_config::load_repo_config;
use mintswarm_core::watch::{
    BODY_LIMIT, Normalised, Senders, WEBHOOK_KIND, WatchError, normalise, untrusted,
};
use mintswarm_domain::tracker::Login;
use mintswarm_proto::watch::v1::{
    ActorClass, ActorType, GitHubWebhook, IssueDependency, ItemKind, ItemRef, Party, WatchEvent,
    watch_event,
};
use serde_json::{Value, json};
use tonic::Code;

const DELIVERY: &str = "00000000-0000-4000-8000-000000000001";
const MARKER: &str = "MARKER-7f3c";

/// The smallest config the loader accepts that trusts `maintainer`.
const CONFIG: &str = "tracker:
  ingest:
    trusted:
      authors: [maintainer]
components:
  core:
    paths: [\"x/**\"]
pipeline:
  plan_approval:
    mode: always
    limits: { files: 10, lines: 500, new_files: 3 }
  landing:
    mode: always
    footprint: { lines_factor: 1.5 }
";

/// The senders of [`CONFIG`], with `mintswarm-dev[bot]` as the plane.
fn senders() -> Senders {
    let config = load_repo_config(CONFIG).expect("the minimal config loads");
    Senders::new(config.trust(), vec![Login("mintswarm-dev[bot]".to_owned())])
}

fn fixture(name: &str) -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/webhooks")
        .join(format!("{name}.json"));
    std::fs::read(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

fn json_of(bytes: &[u8]) -> Value {
    serde_json::from_slice(bytes).expect("the fixture is JSON")
}

/// Normalises `body` as delivery [`DELIVERY`] of `event`.
fn run(event: &str, body: Vec<u8>) -> Normalised {
    normalise(DELIVERY, event, body, &senders()).expect("the delivery normalises")
}

fn run_value(event: &str, payload: &Value) -> Normalised {
    run(
        event,
        serde_json::to_vec(payload).expect("a JSON value serialises"),
    )
}

fn recorded(event: &str, name: &str) -> (Normalised, Value) {
    let body = fixture(name);
    let payload = json_of(&body);
    (run(event, body), payload)
}

fn webhook(event: &WatchEvent) -> &GitHubWebhook {
    match &event.body {
        Some(watch_event::Body::GithubWebhook(webhook)) => webhook,
        None => panic!("no github.webhook body"),
        Some(other) => panic!("not a github.webhook body: {other:?}"),
    }
}

fn detail<'a>(event: &'a WatchEvent, key: &str) -> Option<&'a str> {
    webhook(event).detail.get(key).map(|t| t.text.as_str())
}

fn roles<'a>(event: &'a WatchEvent, role: &str) -> Vec<&'a str> {
    event
        .parties
        .iter()
        .filter(|p| p.role == role)
        .map(|p| p.login.as_str())
        .collect()
}

fn str_at<'a>(payload: &'a Value, pointer: &str) -> &'a str {
    payload
        .pointer(pointer)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("no string at {pointer}"))
}

#[test]
fn issue_comment_created_maps_envelope() {
    let (out, payload) = recorded("issue_comment", "issue_comment.created");
    let event = &out.event;
    assert_eq!(event.kind, WEBHOOK_KIND);
    assert!(event.id.is_empty() && event.cursor.is_empty());

    let subject = event.subject.as_ref().expect("a subject");
    assert_eq!(
        subject.repository,
        str_at(&payload, "/repository/full_name")
    );
    assert_eq!(subject.number, payload["issue"]["number"].as_u64().unwrap());
    assert_eq!(subject.item_kind(), ItemKind::Issue);
    assert_eq!(subject.url, str_at(&payload, "/issue/html_url"));
    let title = subject.title.as_ref().expect("a title");
    assert_eq!(title.text, str_at(&payload, "/issue/title"));

    let actor = event.actor.as_ref().expect("an actor");
    assert_eq!(actor.login, "maintainer");
    assert_eq!(actor.id, payload["sender"]["id"].to_string());
    assert_eq!(actor.r#type(), ActorType::User);
    assert_eq!(actor.class(), ActorClass::Maintainer);

    assert_eq!(
        roles(event, "author"),
        [str_at(&payload, "/issue/user/login")]
    );
    assert_eq!(
        roles(event, "assignee"),
        [str_at(&payload, "/issue/assignees/0/login")]
    );
    assert!(roles(event, "requested_reviewer").is_empty());

    assert_eq!(event.attributes["event"], "issue_comment");
    assert_eq!(event.attributes["action"], "created");
    let body = webhook(event).body.as_ref().expect("a body");
    assert_eq!(body.text, str_at(&payload, "/comment/body"));
    assert!(!body.truncated);
}

#[test]
fn issue_comment_created_fills_webhook_body() {
    let bytes = fixture("issue_comment.created");
    let out = run("issue_comment", bytes.clone());
    let webhook = webhook(&out.event);
    assert_eq!(webhook.delivery, DELIVERY);
    assert_eq!(webhook.event, "issue_comment");
    assert_eq!(webhook.action, "created");
    assert!(webhook.payload.is_empty());
    assert_eq!(out.payload, bytes);
}

#[test]
fn pull_request_assigned_has_requested_reviewer() {
    let (out, payload) = recorded("pull_request", "pull_request.assigned");
    let event = &out.event;
    assert_eq!(
        event.subject.as_ref().unwrap().item_kind(),
        ItemKind::PullRequest
    );
    assert_eq!(
        roles(event, "author"),
        [str_at(&payload, "/pull_request/user/login")]
    );
    assert_eq!(
        roles(event, "assignee"),
        [str_at(&payload, "/pull_request/assignees/0/login")]
    );
    assert_eq!(
        roles(event, "requested_reviewer"),
        [str_at(
            &payload,
            "/pull_request/requested_reviewers/0/login"
        )]
    );
    let actor = event.actor.as_ref().unwrap();
    assert_eq!(actor.r#type(), ActorType::Bot);
    assert_eq!(actor.class(), ActorClass::Self_);
}

#[test]
fn pull_request_review_approved_gives_state() {
    let (out, _) = recorded(
        "pull_request_review",
        "pull_request_review.submitted.approved",
    );
    let webhook = webhook(&out.event);
    assert_eq!(webhook.event, "pull_request_review");
    assert_eq!(webhook.action, "submitted");
    assert_eq!(detail(&out.event, "review_state"), Some("approved"));
    assert!(webhook.body.is_none());
}

#[test]
fn approval_without_text_leaves_body_unset() {
    let (out, payload) = recorded(
        "pull_request_review",
        "pull_request_review.submitted.approved",
    );
    assert!(payload["review"]["body"].is_null());
    assert!(!str_at(&payload, "/pull_request/body").is_empty());
    assert!(webhook(&out.event).body.is_none());
}

#[test]
fn pull_request_review_commented_gives_body() {
    let (out, payload) = recorded(
        "pull_request_review",
        "pull_request_review.submitted.commented",
    );
    let webhook = webhook(&out.event);
    assert_eq!(webhook.event, "pull_request_review");
    assert_eq!(webhook.action, "submitted");
    let body = webhook.body.as_ref().expect("a body");
    assert_eq!(body.text, str_at(&payload, "/review/body"));
    assert_eq!(detail(&out.event, "review_state"), Some("commented"));
}

#[test]
fn pull_request_closed_gives_merged() {
    let (out, payload) = recorded("pull_request", "pull_request.closed");
    assert_eq!(payload["pull_request"]["merged"], json!(true));
    assert_eq!(detail(&out.event, "merged"), Some("true"));
    assert_eq!(webhook(&out.event).detail.len(), 1);
}

#[test]
fn issues_labeled_gives_label() {
    let (out, payload) = recorded("issues", "issues.labeled");
    assert_eq!(
        detail(&out.event, "label"),
        Some(str_at(&payload, "/label/name"))
    );
    let body = webhook(&out.event).body.as_ref().expect("the issue's body");
    assert_eq!(body.text, str_at(&payload, "/issue/body"));
}

#[test]
fn check_suite_completed_gives_conclusion() {
    let (out, payload) = recorded("check_suite", "check_suite.completed");
    assert_eq!(
        detail(&out.event, "check_conclusion"),
        Some(str_at(&payload, "/check_suite/conclusion"))
    );
    let subject = out.event.subject.as_ref().unwrap();
    assert_eq!(subject.item_kind(), ItemKind::None);
    assert_eq!(subject.number, 0);
    assert!(subject.url.is_empty() && subject.title.is_none());
    assert!(out.event.parties.is_empty());
    assert!(webhook(&out.event).body.is_none());
}

#[test]
fn two_assignees_and_reviewers_give_two_parties_each() {
    let user = |login: &str| json!({"login": login, "type": "User"});
    let payload = json!({
        "action": "review_requested",
        "pull_request": {
            "number": 7,
            "title": "A change",
            "user": user("contributor-1"),
            "assignees": [user("contributor-2"), user("contributor-3")],
            "requested_reviewers": [user("contributor-4"), user("contributor-5")],
        },
        "repository": {"full_name": "owner/name"},
        "sender": user("contributor-1"),
    });
    let out = run_value("pull_request", &payload);
    let party = |login: &str, role: &str| Party {
        login: login.to_owned(),
        role: role.to_owned(),
    };
    assert_eq!(
        out.event.parties,
        [
            party("contributor-1", "author"),
            party("contributor-2", "assignee"),
            party("contributor-3", "assignee"),
            party("contributor-4", "requested_reviewer"),
            party("contributor-5", "requested_reviewer"),
        ]
    );
}

#[test]
fn payload_without_action_gives_empty_action() {
    let (out, payload) = recorded("push", "push");
    assert!(payload.get("action").is_none());
    assert!(webhook(&out.event).action.is_empty());
    assert_eq!(out.event.attributes["action"], "");
    assert_eq!(out.event.attributes["event"], "push");
    assert_eq!(
        out.event.summary,
        format!(
            "github.webhook push {} by {}",
            str_at(&payload, "/repository/full_name"),
            str_at(&payload, "/sender/login")
        )
    );
}

/// A comment payload with `body`.
fn comment(body: &str) -> Value {
    json!({
        "action": "created",
        "issue": {"number": 1, "title": "An issue"},
        "comment": {"body": body},
        "repository": {"full_name": "owner/name"},
        "sender": {"login": "contributor-1", "type": "User"},
    })
}

fn comment_body(body: &str) -> (String, bool, u32) {
    let out = run_value("issue_comment", &comment(body));
    let text = webhook(&out.event).body.clone().expect("a body");
    (text.text, text.truncated, text.original_length)
}

#[test]
fn long_body_is_cut_to_64_kib() {
    let (text, truncated, length) = comment_body(&"a".repeat(70_000));
    assert_eq!(text.len(), BODY_LIMIT);
    assert!(truncated);
    assert_eq!(length, 70_000);

    let whole = "b".repeat(BODY_LIMIT);
    let (text, truncated, length) = comment_body(&whole);
    assert_eq!(text, whole);
    assert!(!truncated);
    assert_eq!(length, 65_536);

    // A three-byte character straddles the limit: the cut falls before it.
    let wide = format!("{}€€", "c".repeat(BODY_LIMIT - 1));
    let (text, truncated, length) = comment_body(&wide);
    assert_eq!(text, "c".repeat(BODY_LIMIT - 1));
    assert!(truncated);
    assert_eq!(length as usize, wide.len());

    let short = untrusted("short");
    assert_eq!(short.text, "short");
    assert!(!short.truncated);
    assert_eq!(short.original_length, 5);
}

/// `payload` with the string at each of `pointers` replaced by [`MARKER`].
fn marked(mut payload: Value, pointers: &[&str]) -> Value {
    for pointer in pointers {
        *payload
            .pointer_mut(pointer)
            .unwrap_or_else(|| panic!("no {pointer}")) = json!(MARKER);
    }
    payload
}

#[test]
fn summary_never_carries_untrusted_text() {
    let cases = [
        (
            "issue_comment",
            "issue_comment.created",
            vec!["/issue/title", "/comment/body"],
        ),
        (
            "pull_request_review",
            "pull_request_review.submitted.commented",
            vec!["/review/body", "/review/state"],
        ),
        ("issues", "issues.labeled", vec!["/label/name"]),
        (
            "check_suite",
            "check_suite.completed",
            vec!["/check_suite/conclusion"],
        ),
        (
            "issue_dependencies",
            "issue_dependencies.blocked_by_added",
            vec!["/blocked_issue/title"],
        ),
        (
            "issue_dependencies",
            "issue_dependencies.blocking_added",
            vec!["/blocking_issue/title"],
        ),
    ];
    for (event, name, pointers) in cases {
        let payload = marked(json_of(&fixture(name)), &pointers);
        let out = run_value(event, &payload).event;
        let webhook = webhook(&out);
        let reached = out
            .subject
            .as_ref()
            .and_then(|s| s.title.as_ref())
            .is_some_and(|t| t.text == MARKER)
            || webhook.body.as_ref().is_some_and(|b| b.text == MARKER)
            || webhook.detail.values().any(|d| d.text == MARKER);
        assert!(reached, "{name}: the marker reached no untrusted field");
        assert!(!out.summary.contains(MARKER), "{name}: {}", out.summary);
        assert!(out.summary.starts_with(&format!("{WEBHOOK_KIND} {event}")));
    }
}

#[test]
fn summary_filters_unexpected_values() {
    let mut payload = comment("text");
    payload["action"] = json!("created now");
    payload["sender"]["login"] = json!(MARKER.to_owned() + " x");
    let out = run_value("issue_comment", &payload).event;
    assert_eq!(
        out.summary,
        "github.webhook issue_comment.? owner/name#1 by ?"
    );
}

fn dependency(event: &WatchEvent) -> &IssueDependency {
    webhook(event)
        .dependency
        .as_ref()
        .expect("an issue dependency")
}

fn item_ref(payload: &Value, repository: &str, issue: &str) -> ItemRef {
    ItemRef {
        repository: str_at(payload, &format!("/{repository}/full_name")).to_owned(),
        number: payload[issue]["number"].as_u64().expect("an issue number"),
    }
}

/// Checks a recorded `issue_dependencies` delivery whose subject is the issue at `subject`:
/// both sides, the subject, the author party, the attributes and the summary.
fn assert_recorded_link(
    action: &str,
    subject_issue: &str,
    blocked_repo: &str,
    blocking_repo: &str,
) {
    let (out, payload) = recorded(
        "issue_dependencies",
        &format!("issue_dependencies.{action}"),
    );
    let event = &out.event;
    assert_eq!(str_at(&payload, "/action"), action);
    assert_eq!(
        dependency(event),
        &IssueDependency {
            blocked: Some(item_ref(&payload, blocked_repo, "blocked_issue")),
            blocking: Some(item_ref(&payload, blocking_repo, "blocking_issue")),
        }
    );

    let subject = event.subject.as_ref().expect("a subject");
    let number = payload[subject_issue]["number"].as_u64().unwrap();
    assert_eq!(subject.item_kind(), ItemKind::Issue);
    assert_eq!(
        subject.repository,
        str_at(&payload, "/repository/full_name")
    );
    assert_eq!(subject.number, number);
    assert_eq!(
        subject.url,
        str_at(&payload, &format!("/{subject_issue}/html_url"))
    );
    assert_eq!(
        subject.title.as_ref().expect("a title").text,
        str_at(&payload, &format!("/{subject_issue}/title"))
    );
    assert_eq!(
        roles(event, "author"),
        [str_at(&payload, &format!("/{subject_issue}/user/login"))]
    );

    assert_eq!(event.attributes["event"], "issue_dependencies");
    assert_eq!(event.attributes["action"], action);
    assert_eq!(webhook(event).action, action);
    assert!(webhook(event).body.is_none());
    assert!(
        event.summary.ends_with(&format!(
            "#{number} by {}",
            str_at(&payload, "/sender/login")
        )),
        "{}",
        event.summary
    );
}

#[test]
fn blocked_by_added_names_both_sides() {
    assert_recorded_link(
        "blocked_by_added",
        "blocked_issue",
        "repository",
        "blocking_issue_repo",
    );
}

#[test]
fn blocking_added_names_both_sides() {
    assert_recorded_link(
        "blocking_added",
        "blocking_issue",
        "blocked_issue_repo",
        "repository",
    );
}

#[test]
fn removed_actions_map_like_added() {
    for (added, removed) in [
        ("blocked_by_added", "blocked_by_removed"),
        ("blocking_added", "blocking_removed"),
    ] {
        let payload = json_of(&fixture(&format!("issue_dependencies.{added}")));
        let mut changed = payload.clone();
        changed["action"] = json!(removed);
        let added_event = run_value("issue_dependencies", &payload).event;
        let removed_event = run_value("issue_dependencies", &changed).event;
        assert_eq!(dependency(&removed_event), dependency(&added_event));
        assert_eq!(removed_event.subject, added_event.subject);
        assert_eq!(removed_event.parties, added_event.parties);
        assert_eq!(removed_event.attributes["action"], removed);
        assert_eq!(webhook(&removed_event).action, removed);
    }
}

#[test]
fn cross_repository_link_names_both_repositories() {
    let issue = |number: u64| json!({"number": number, "title": "An issue", "user": {"login": "contributor-1"}});
    let payload = json!({
        "action": "blocked_by_added",
        "blocked_issue": issue(5),
        "blocking_issue": issue(9),
        "blocking_issue_repo": {"full_name": "other/repo"},
        "repository": {"full_name": "owner/name"},
        "sender": {"login": "contributor-1", "type": "User"},
    });
    let out = run_value("issue_dependencies", &payload).event;
    let item = |repository: &str, number: u64| ItemRef {
        repository: repository.to_owned(),
        number,
    };
    assert_eq!(
        dependency(&out),
        &IssueDependency {
            blocked: Some(item("owner/name", 5)),
            blocking: Some(item("other/repo", 9)),
        }
    );
    let subject = out.subject.as_ref().unwrap();
    assert_eq!(
        (subject.repository.as_str(), subject.number),
        ("owner/name", 5)
    );
}

#[test]
fn far_side_without_its_repo_never_takes_the_delivery_repository() {
    let issue = |number: u64, repository_url: Option<&str>| {
        let mut issue = json!({"number": number, "title": "An issue"});
        if let Some(url) = repository_url {
            issue["repository_url"] = json!(url);
        }
        issue
    };
    let item = |repository: &str, number: u64| {
        Some(ItemRef {
            repository: repository.to_owned(),
            number,
        })
    };
    let far_url = Some("https://api.github.com/repos/other/repo");
    let own_url = Some("https://api.github.com/repos/owner/name");
    let cases = [
        // A blocked_by_* delivery is sent for the blocked side; the blocking side is far.
        (
            "blocked_by_added",
            issue(5, own_url),
            issue(9, far_url),
            item("owner/name", 5),
            item("other/repo", 9),
        ),
        (
            "blocked_by_removed",
            issue(5, None),
            issue(9, None),
            item("owner/name", 5),
            item("", 9),
        ),
        // A blocking_* delivery is sent for the blocking side; the blocked side is far.
        (
            "blocking_added",
            issue(5, far_url),
            issue(9, own_url),
            item("other/repo", 5),
            item("owner/name", 9),
        ),
        (
            "blocking_removed",
            issue(5, Some("https://api.github.com/repos/malformed")),
            issue(9, None),
            item("", 5),
            item("owner/name", 9),
        ),
    ];
    for (action, blocked_issue, blocking_issue, blocked, blocking) in cases {
        let payload = json!({
            "action": action,
            "blocked_issue": blocked_issue,
            "blocking_issue": blocking_issue,
            "repository": {"full_name": "owner/name"},
            "sender": {"login": "contributor-1", "type": "User"},
        });
        let out = run_value("issue_dependencies", &payload).event;
        assert_eq!(
            dependency(&out),
            &IssueDependency { blocked, blocking },
            "{action}"
        );
    }
}

#[test]
fn one_sided_blocking_added_leaves_blocked_unset() {
    let mut payload = json_of(&fixture("issue_dependencies.blocking_added"));
    let object = payload.as_object_mut().unwrap();
    for key in ["blocked_issue", "blocked_issue_id", "blocked_issue_repo"] {
        assert!(object.remove(key).is_some(), "the recording has {key}");
    }
    let out = run_value("issue_dependencies", &payload).event;
    let dependency = dependency(&out);
    assert_eq!(dependency.blocked, None);
    assert_eq!(
        dependency.blocking,
        Some(item_ref(&payload, "repository", "blocking_issue"))
    );
    assert_eq!(
        out.subject.as_ref().unwrap().number,
        payload["blocking_issue"]["number"].as_u64().unwrap()
    );
}

#[test]
fn other_events_have_no_dependency() {
    for (event, name) in [
        ("issue_comment", "issue_comment.created"),
        ("issues", "issues.labeled"),
        ("pull_request", "pull_request.assigned"),
        ("pull_request", "pull_request.closed"),
        (
            "pull_request_review",
            "pull_request_review.submitted.approved",
        ),
        (
            "pull_request_review",
            "pull_request_review.submitted.commented",
        ),
        ("check_suite", "check_suite.completed"),
        ("push", "push"),
    ] {
        let (out, _) = recorded(event, name);
        assert_eq!(webhook(&out.event).dependency, None, "{name}");
    }
}

#[test]
fn classifier_classes_senders() {
    let senders = senders();
    assert_eq!(
        senders.classify("maintainer", false),
        ActorClass::Maintainer
    );
    assert_eq!(
        senders.classify("Maintainer", false),
        ActorClass::Maintainer
    );
    assert_eq!(
        senders.classify("mintswarm-dev[bot]", true),
        ActorClass::Self_
    );
    assert_eq!(
        senders.classify("github-actions[bot]", true),
        ActorClass::App
    );
    assert_eq!(senders.classify("contributor-1", false), ActorClass::Person);

    let (out, _) = recorded(
        "pull_request_review",
        "pull_request_review.submitted.commented",
    );
    assert_eq!(out.event.actor.unwrap().class(), ActorClass::Self_);
}

#[test]
fn malformed_deliveries_are_invalid_arguments() {
    let senders = senders();
    let cases = [
        ("", "push", b"{}".to_vec()),
        (DELIVERY, "", b"{}".to_vec()),
        (DELIVERY, "push", b"not json".to_vec()),
        (DELIVERY, "push", b"[1, 2]".to_vec()),
    ];
    for (delivery, event, body) in cases {
        let error = normalise(delivery, event, body, &senders).expect_err("a malformed delivery");
        assert!(matches!(error, WatchError::Payload(_)));
        assert_eq!(tonic::Status::from(error).code(), Code::InvalidArgument);
    }
}
