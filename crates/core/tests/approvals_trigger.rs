//! The approvals and PR triggers: design #548 §5's table of deliveries, the `SELF` filter and
//! its package exception, the label filters, the recorded deliveries through the normaliser,
//! and the merge with a state-sync cause.
// Not redundant: clippy's `allow-unwrap-in-tests` covers a `tests/` crate only under `cfg(test)`.
#![cfg(test)]

use std::collections::HashMap;
use std::path::PathBuf;

use mintswarm_config::load_repo_config;
use mintswarm_core::approvals::trigger::{ApprovalCause, reduce};
use mintswarm_core::keeper::checks::HOLD_LABEL;
use mintswarm_core::state_sync::queue::Entry;
use mintswarm_core::state_sync::trigger::reduce as state_sync_reduce;
use mintswarm_core::watch::{Senders, WEBHOOK_KIND, normalise, untrusted};
use mintswarm_domain::ids::IssueNumber;
use mintswarm_domain::tracker::Login;
use mintswarm_proto::watch::v1::{
    Actor, ActorClass, GitHubWebhook, ItemKind, Subject, WatchEvent, watch_event,
};
use mintswarm_rules::command::PackageKind;
use serde_json::{Value, json};

const REPO: &str = "mintswarm/mintswarm";
const OTHER: &str = "someone/else";
const LABEL: &str = "msw-state:plan approved";
const SHA: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

/// One delivery, built in the test.
struct Delivery {
    event: &'static str,
    action: &'static str,
    kind: ItemKind,
    number: u64,
    repository: &'static str,
    class: ActorClass,
    body: Option<String>,
    detail: Vec<(&'static str, String)>,
}

fn delivery(event: &'static str, action: &'static str, kind: ItemKind, number: u64) -> Delivery {
    Delivery {
        event,
        action,
        kind,
        number,
        repository: REPO,
        class: ActorClass::Person,
        body: None,
        detail: Vec::new(),
    }
}

impl Delivery {
    fn by(mut self, class: ActorClass) -> Self {
        self.class = class;
        self
    }

    fn body(mut self, body: &str) -> Self {
        self.body = Some(body.to_owned());
        self
    }

    fn detail(mut self, key: &'static str, value: &str) -> Self {
        self.detail.push((key, value.to_owned()));
        self
    }

    fn in_repository(mut self, repository: &'static str) -> Self {
        self.repository = repository;
        self
    }

    fn event(&self) -> WatchEvent {
        WatchEvent {
            kind: WEBHOOK_KIND.to_owned(),
            subject: Some(Subject {
                repository: self.repository.to_owned(),
                item_kind: self.kind.into(),
                number: self.number,
                ..Subject::default()
            }),
            actor: Some(Actor {
                login: "someone".to_owned(),
                class: self.class.into(),
                ..Actor::default()
            }),
            body: Some(watch_event::Body::GithubWebhook(GitHubWebhook {
                event: self.event.to_owned(),
                action: self.action.to_owned(),
                body: self.body.as_deref().map(untrusted),
                detail: self
                    .detail
                    .iter()
                    .map(|(key, value)| ((*key).to_owned(), untrusted(value)))
                    .collect::<HashMap<_, _>>(),
                ..GitHubWebhook::default()
            })),
            ..WatchEvent::default()
        }
    }

    fn reduce(&self) -> Vec<(IssueNumber, Entry)> {
        reduce(&self.event(), REPO, Some(LABEL))
    }
}

fn issue(number: u64, cause: ApprovalCause) -> Vec<(IssueNumber, Entry)> {
    vec![(IssueNumber(number), Entry::approvals(cause))]
}

fn pull(number: u64) -> Vec<(IssueNumber, Entry)> {
    vec![(IssueNumber(number), Entry::pull())]
}

fn package(kind: PackageKind) -> String {
    format!(
        "{}\n{}\nPlease approve.",
        kind.marker(),
        kind.digest_line(SHA)
    )
}

const TRIGGERED: ApprovalCause = ApprovalCause::Triggered;

#[test]
fn every_row_of_the_table_queues_its_issue_or_pull_request() {
    let cases: Vec<(Delivery, Vec<(IssueNumber, Entry)>)> = vec![
        (
            delivery("issue_comment", "created", ItemKind::Issue, 7).body("Approve"),
            issue(7, TRIGGERED),
        ),
        (
            delivery("issue_comment", "edited", ItemKind::Issue, 7).body("Approve"),
            issue(7, TRIGGERED),
        ),
        (
            delivery("issue_comment", "deleted", ItemKind::Issue, 7),
            vec![],
        ),
        (
            delivery("issue_comment", "created", ItemKind::PullRequest, 8),
            pull(8),
        ),
        (
            delivery("issue_comment", "edited", ItemKind::PullRequest, 8),
            pull(8),
        ),
        (
            delivery("issues", "closed", ItemKind::Issue, 7),
            issue(7, TRIGGERED),
        ),
        (
            delivery("issues", "reopened", ItemKind::Issue, 7),
            issue(7, TRIGGERED),
        ),
        (delivery("issues", "opened", ItemKind::Issue, 7), vec![]),
        (
            delivery("issues", "labeled", ItemKind::Issue, 7).detail("label", LABEL),
            issue(7, TRIGGERED),
        ),
        (
            delivery("issues", "unlabeled", ItemKind::Issue, 7)
                .detail("label", "MSW-State:Plan Approved"),
            issue(7, TRIGGERED),
        ),
        (
            delivery("pull_request_review", "submitted", ItemKind::PullRequest, 8),
            pull(8),
        ),
        (
            delivery("pull_request_review", "dismissed", ItemKind::PullRequest, 8),
            pull(8),
        ),
        (
            delivery("pull_request_review", "edited", ItemKind::PullRequest, 8),
            vec![],
        ),
        (
            delivery(
                "pull_request_review_comment",
                "created",
                ItemKind::PullRequest,
                8,
            ),
            pull(8),
        ),
        (
            delivery(
                "pull_request_review_comment",
                "deleted",
                ItemKind::PullRequest,
                8,
            ),
            pull(8),
        ),
        (
            delivery("pull_request", "synchronize", ItemKind::PullRequest, 8),
            pull(8),
        ),
        (
            delivery("pull_request", "ready_for_review", ItemKind::PullRequest, 8),
            pull(8),
        ),
        (
            delivery("pull_request", "labeled", ItemKind::PullRequest, 8)
                .detail("label", HOLD_LABEL),
            pull(8),
        ),
        (
            delivery("pull_request", "unlabeled", ItemKind::PullRequest, 8).detail("label", "Hold"),
            pull(8),
        ),
        (
            delivery("pull_request", "opened", ItemKind::PullRequest, 8),
            vec![],
        ),
        (
            delivery("check_suite", "completed", ItemKind::None, 0).detail("pull_requests", "8,9"),
            [pull(8), pull(9)].concat(),
        ),
        (
            delivery("check_suite", "completed", ItemKind::None, 0),
            vec![],
        ),
        (
            delivery("check_suite", "requested", ItemKind::None, 0).detail("pull_requests", "8"),
            vec![],
        ),
        (delivery("push", "", ItemKind::None, 0), vec![]),
    ];
    for (delivery, expected) in cases {
        assert_eq!(
            delivery.reduce(),
            expected,
            "{} {}",
            delivery.event,
            delivery.action
        );
    }
}

#[test]
fn only_the_planes_repository_and_real_numbers_count() {
    let other = delivery("issue_comment", "created", ItemKind::Issue, 7).in_repository(OTHER);
    assert!(other.reduce().is_empty());
    let cased = delivery("issue_comment", "created", ItemKind::Issue, 7)
        .in_repository("MintSwarm/MintSwarm");
    assert_eq!(cased.reduce(), issue(7, TRIGGERED));
    assert!(
        delivery("issue_comment", "created", ItemKind::Issue, 0)
            .reduce()
            .is_empty()
    );
    let suite = delivery("check_suite", "completed", ItemKind::None, 0)
        .detail("pull_requests", "0,8")
        .in_repository(OTHER);
    assert!(suite.reduce().is_empty());
    let zero = delivery("check_suite", "completed", ItemKind::None, 0).detail("pull_requests", "0");
    assert!(zero.reduce().is_empty());
    let mut wrong_kind = delivery("issues", "closed", ItemKind::Issue, 7).event();
    wrong_kind.kind = "other.kind".to_owned();
    assert!(reduce(&wrong_kind, REPO, Some(LABEL)).is_empty());
}

#[test]
fn self_comments_and_reviews_queue_nothing() {
    for own in [
        delivery("issue_comment", "created", ItemKind::Issue, 7).body("Approve"),
        delivery("issue_comment", "edited", ItemKind::Issue, 7).body("Approve"),
        delivery("issue_comment", "created", ItemKind::PullRequest, 8),
        delivery("pull_request_review", "submitted", ItemKind::PullRequest, 8),
        delivery(
            "pull_request_review_comment",
            "created",
            ItemKind::PullRequest,
            8,
        ),
    ] {
        let own = own.by(ActorClass::Self_);
        assert!(own.reduce().is_empty(), "{} {}", own.event, own.action);
    }
}

#[test]
fn a_new_package_from_self_opens_its_issue() {
    for kind in PackageKind::ALL {
        let created = delivery("issue_comment", "created", ItemKind::Issue, 7)
            .by(ActorClass::Self_)
            .body(&package(kind));
        assert_eq!(
            created.reduce(),
            issue(7, ApprovalCause::Opened),
            "{kind:?}"
        );

        // A package without a well-formed digest line is still a package.
        let not_current = delivery("issue_comment", "created", ItemKind::Issue, 7)
            .by(ActorClass::Self_)
            .body(kind.marker());
        assert_eq!(not_current.reduce(), issue(7, ApprovalCause::Opened));

        let edited = delivery("issue_comment", "edited", ItemKind::Issue, 7)
            .by(ActorClass::Self_)
            .body(&package(kind));
        assert!(edited.reduce().is_empty(), "{kind:?}");

        let on_a_pull = delivery("issue_comment", "created", ItemKind::PullRequest, 8)
            .by(ActorClass::Self_)
            .body(&package(kind));
        assert!(on_a_pull.reduce().is_empty(), "{kind:?}");
    }
}

#[test]
fn a_relayed_steer_from_self_queues_nothing() {
    let steer = format!(
        "<!-- mintswarm: steer -->\n<!-- mintswarm: approval steer -->\nApprove\n{}",
        PackageKind::Plan.digest_line(SHA)
    );
    let own = delivery("issue_comment", "created", ItemKind::Issue, 7)
        .by(ActorClass::Self_)
        .body(&steer);
    assert!(own.reduce().is_empty());
}

#[test]
fn self_label_and_suite_deliveries_still_queue() {
    let label = delivery("issues", "labeled", ItemKind::Issue, 7)
        .detail("label", LABEL)
        .by(ActorClass::Self_);
    assert_eq!(label.reduce(), issue(7, TRIGGERED));
    let suite = delivery("check_suite", "completed", ItemKind::None, 0)
        .detail("pull_requests", "8")
        .by(ActorClass::Self_);
    assert_eq!(suite.reduce(), pull(8));
}

#[test]
fn only_the_hold_and_plan_approved_labels_count() {
    let other = delivery("issues", "labeled", ItemKind::Issue, 7).detail("label", "bug");
    assert!(other.reduce().is_empty());
    let unnamed = delivery("issues", "labeled", ItemKind::Issue, 7);
    assert!(unnamed.reduce().is_empty());
    // In field mode no label is a value.
    let field_mode = delivery("issues", "labeled", ItemKind::Issue, 7).detail("label", LABEL);
    assert!(reduce(&field_mode.event(), REPO, None).is_empty());
    let pull_label = delivery("pull_request", "labeled", ItemKind::PullRequest, 8)
        .detail("label", "needs-review");
    assert!(pull_label.reduce().is_empty());
}

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

fn fixture(name: &str) -> Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/webhooks")
        .join(format!("{name}.json"));
    let bytes = std::fs::read(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()));
    serde_json::from_slice(&bytes).unwrap()
}

/// `payload` as delivery `event`, normalised with `mintswarm-dev-keeper[bot]` as the plane and
/// `mintswarm-dev-bee[bot]` as its runner.
fn normalised(event: &str, payload: &Value) -> WatchEvent {
    let config = load_repo_config(CONFIG).unwrap();
    let senders = Senders::new(
        config.trust(),
        vec![
            Login("mintswarm-dev-keeper[bot]".to_owned()),
            Login("mintswarm-dev-bee[bot]".to_owned()),
        ],
    );
    let body = serde_json::to_vec(payload).unwrap();
    normalise(
        "00000000-0000-4000-8000-000000000001",
        event,
        body,
        &senders,
    )
    .unwrap()
    .event
}

#[test]
fn recorded_deliveries_queue_through_the_normaliser() {
    let comment = fixture("issue_comment.created");
    assert_eq!(
        reduce(&normalised("issue_comment", &comment), REPO, Some(LABEL)),
        issue(547, TRIGGERED)
    );

    let approved = fixture("pull_request_review.submitted.approved");
    assert_eq!(
        reduce(
            &normalised("pull_request_review", &approved),
            REPO,
            Some(LABEL)
        ),
        pull(551)
    );

    // The plane's own review queues nothing.
    let commented = fixture("pull_request_review.submitted.commented");
    assert!(
        reduce(
            &normalised("pull_request_review", &commented),
            REPO,
            Some(LABEL)
        )
        .is_empty()
    );

    // The suite is sent by the runner's App, and still queues its pull request.
    let suite = fixture("check_suite.completed");
    assert_eq!(
        reduce(&normalised("check_suite", &suite), REPO, Some(LABEL)),
        pull(524)
    );

    let mut labeled = fixture("issues.labeled");
    assert!(reduce(&normalised("issues", &labeled), REPO, Some(LABEL)).is_empty());
    labeled["label"]["name"] = json!(LABEL);
    assert_eq!(
        reduce(&normalised("issues", &labeled), REPO, Some(LABEL)),
        issue(598, TRIGGERED)
    );

    // A package the plane posts opens its issue.
    let mut package_comment = fixture("issue_comment.created");
    package_comment["sender"]["login"] = json!("mintswarm-dev-keeper[bot]");
    package_comment["sender"]["type"] = json!("Bot");
    package_comment["comment"]["body"] = json!(package(PackageKind::Design));
    assert_eq!(
        reduce(
            &normalised("issue_comment", &package_comment),
            REPO,
            Some(LABEL)
        ),
        issue(547, ApprovalCause::Opened)
    );
}

#[test]
fn a_closed_issue_merges_both_causes_into_one_entry() {
    let event = delivery("issues", "closed", ItemKind::Issue, 7).event();
    let mut entries = state_sync_reduce(&event, REPO);
    entries.extend(reduce(&event, REPO, Some(LABEL)));
    assert_eq!(entries.len(), 2);
    let mut merged = entries[0].1.clone();
    merged.merge(entries[1].1.clone());
    assert_eq!(merged.cause, Entry::triggered(Vec::new()).cause);
    assert_eq!(merged.approvals, Some(TRIGGERED));
    assert!(merged.fan_out);
    assert!(!merged.pull);

    // Opened outlasts a later trigger, and a PR cause survives a merge.
    let mut opened = Entry::approvals(ApprovalCause::Opened);
    opened.merge(Entry::approvals(TRIGGERED));
    assert_eq!(opened.approvals, Some(ApprovalCause::Opened));
    let mut triggered = Entry::approvals(TRIGGERED);
    triggered.merge(Entry::approvals(ApprovalCause::Opened));
    assert_eq!(triggered.approvals, Some(ApprovalCause::Opened));
    let mut pull = Entry::pull();
    pull.merge(Entry::approvals(TRIGGERED));
    assert!(pull.pull);
    assert_eq!(pull.cause, None);
}
