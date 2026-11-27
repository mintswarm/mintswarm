//! The state-sync evaluator against the in-memory tracker: duplicate and paired deliveries, out
//! of order closes and reopens, its own writes, unreadable blockers, decomposed parents, active
//! issues, values moving between reads, status repair and the outcome of each evaluation.
// Not redundant: clippy's `allow-unwrap-in-tests` covers a `tests/` crate only under `cfg(test)`.
#![cfg(test)]

use std::sync::{Arc, Mutex};

use mintswarm_config::{
    BLOCKED, DONE, NONE, PLANNING, QUEUED, REQUIRED_OPTIONS, StateOption, StateSettings,
    StatusMode, Trust, TrustMatch,
};
use mintswarm_core::state_sync::evaluate::evaluate;
use mintswarm_core::state_sync::report;
use mintswarm_core::state_sync::rule::{BlockerChange, BlockerChangeKind, Cause};
use mintswarm_core::state_sync::{Outcome, Settings, StateSync};
use mintswarm_core::testing::tracker::{FakeTracker, FakeTrackerError, FakeWrite};
use mintswarm_core::watch::WEBHOOK_KIND;
use mintswarm_domain::field::{
    FieldError, FieldFinding, FieldRead, FieldRef, FieldSource, FieldValue,
};
use mintswarm_domain::ids::IssueNumber;
use mintswarm_domain::tracker::{
    Comment, Comments, Cursor, Dependency, Direction, Fields, Issue, IssueRead, IssueType, Issues,
    Link, Links, Login, Statuses, SubItem, TrackerBase,
};
use mintswarm_proto::watch::v1::{
    GitHubWebhook, IssueDependency, ItemKind, ItemRef, Subject, WatchEvent, watch_event,
};
use serde_json::Value;

const REPO: &str = "mintswarm/mintswarm";
const BOT: &str = "bee[bot]";
const MAINTAINER: &str = "maintainer";

fn field_source() -> FieldSource {
    FieldSource::Field {
        name: "msw-state".to_owned(),
    }
}

/// The status each value maps to; `none` maps to none.
fn status_of(name: &str) -> Option<String> {
    match name {
        n if n == QUEUED => Some("Todo".to_owned()),
        n if n == BLOCKED => Some("Blocked".to_owned()),
        n if n == DONE => Some("Done".to_owned()),
        n if n == NONE => None,
        _ => Some("In progress".to_owned()),
    }
}

fn settings(status: StatusMode) -> Settings {
    Settings {
        repository: REPO.to_owned(),
        bot: Login(BOT.to_owned()),
        trust: Trust {
            labels: Vec::new(),
            authors: vec![Login(MAINTAINER.to_owned())],
            r#match: TrustMatch::Any,
        },
        state: StateSettings {
            source: field_source(),
            status,
            options: REQUIRED_OPTIONS
                .iter()
                .map(|name| StateOption {
                    name: (*name).to_owned(),
                    status: status_of(name),
                })
                .collect(),
        },
    }
}

fn tracker() -> FakeTracker {
    FakeTracker::new(Login(BOT.to_owned()))
}

fn issue(number: u64, open: bool, blocked_by: u64, sub_issues: u64) -> Issue {
    Issue {
        number: IssueNumber(number),
        title: format!("Issue {number}"),
        body: String::new(),
        state: if open { "open" } else { "closed" }.to_owned(),
        labels: Vec::new(),
        issue_type: Some(IssueType::Implementation),
        author: Login(MAINTAINER.to_owned()),
        author_is_bot: false,
        url: format!("https://github.com/{REPO}/issues/{number}"),
        sub_issues,
        blocked_by,
    }
}

fn dependency(number: u64, open: bool) -> Dependency {
    Dependency {
        repository: REPO.to_owned(),
        number: IssueNumber(number),
        open,
    }
}

/// Seeds issue `number` with `value` and its listed blockers, its blocker total the listed count
/// plus `unlisted`.
fn seed(
    fake: &FakeTracker,
    number: u64,
    open: bool,
    value: &str,
    blockers: &[(u64, bool)],
    unlisted: u64,
) {
    let listed: Vec<Dependency> = blockers
        .iter()
        .map(|(blocker, open)| dependency(*blocker, *open))
        .collect();
    let total = listed.len() as u64 + unlisted;
    fake.add_issue(issue(number, open, total, 0));
    fake.set_dependencies(&IssueNumber(number), Direction::BlockedBy, listed);
    fake.put_field(&IssueNumber(number), field_source(), value);
}

fn event(event: &str, action: &str, subject: u64, dependency: Option<(u64, u64)>) -> WatchEvent {
    WatchEvent {
        kind: WEBHOOK_KIND.to_owned(),
        subject: Some(Subject {
            repository: REPO.to_owned(),
            item_kind: ItemKind::Issue.into(),
            number: subject,
            ..Subject::default()
        }),
        body: Some(watch_event::Body::GithubWebhook(GitHubWebhook {
            event: event.to_owned(),
            action: action.to_owned(),
            dependency: dependency.map(|(blocked, blocking)| IssueDependency {
                blocked: Some(ItemRef {
                    repository: REPO.to_owned(),
                    number: blocked,
                }),
                blocking: Some(ItemRef {
                    repository: REPO.to_owned(),
                    number: blocking,
                }),
            }),
            ..GitHubWebhook::default()
        })),
        ..WatchEvent::default()
    }
}

fn closed(number: u64) -> WatchEvent {
    event("issues", "closed", number, None)
}

fn reopened(number: u64) -> WatchEvent {
    event("issues", "reopened", number, None)
}

/// The delivery for `side` (`blocked_by` for the blocked issue, `blocking` for the blocker) of a
/// link of `blocked` to `blocking`.
fn linked(side: &str, blocked: u64, blocking: u64) -> WatchEvent {
    let subject = if side == "blocked_by" {
        blocked
    } else {
        blocking
    };
    event(
        "issue_dependencies",
        &format!("{side}_added"),
        subject,
        Some((blocked, blocking)),
    )
}

/// A hand-off and worker over `fake`, whose lines are kept.
struct Plane {
    fake: Arc<FakeTracker>,
    sync: StateSync,
    worker: mintswarm_core::state_sync::Worker<FakeTracker>,
    lines: Arc<Mutex<Vec<Value>>>,
}

fn plane(fake: FakeTracker, status: StatusMode) -> Plane {
    let fake = Arc::new(fake);
    let lines = Arc::new(Mutex::new(Vec::new()));
    let sink_lines = Arc::clone(&lines);
    let (sync, worker) = StateSync::new(Arc::clone(&fake), settings(status));
    let worker = worker.with_sink(Arc::new(move |line: &str| {
        sink_lines
            .lock()
            .unwrap()
            .push(serde_json::from_str(line).unwrap());
    }));
    Plane {
        fake,
        sync,
        worker,
        lines,
    }
}

fn set_fields(writes: &[FakeWrite], number: u64) -> Vec<Option<String>> {
    writes
        .iter()
        .filter_map(|write| match write {
            FakeWrite::SetField { issue, option, .. } if issue.0 == number => Some(option.clone()),
            _ => None,
        })
        .collect()
}

fn set_statuses(writes: &[FakeWrite], number: u64) -> Vec<String> {
    writes
        .iter()
        .filter_map(|write| match write {
            FakeWrite::SetStatus { issue, status } if issue.0 == number => Some(status.clone()),
            _ => None,
        })
        .collect()
}

fn posts(writes: &[FakeWrite], number: u64) -> Vec<String> {
    writes
        .iter()
        .filter_map(|write| match write {
            FakeWrite::PostComment { issue, body } if issue.0 == number => Some(body.clone()),
            _ => None,
        })
        .collect()
}

fn touched(writes: &[FakeWrite], number: u64) -> bool {
    !set_fields(writes, number).is_empty()
        || !set_statuses(writes, number).is_empty()
        || !posts(writes, number).is_empty()
}

fn change(blocker: u64, change: BlockerChangeKind) -> Cause {
    Cause::Triggered {
        blocker_changes: vec![BlockerChange {
            blocker: IssueNumber(blocker),
            change,
        }],
    }
}

fn no_change() -> Cause {
    Cause::Triggered {
        blocker_changes: Vec::new(),
    }
}

/// `#3` closed and `done`, blocking `#5` (`blocked`, no other blocker) and `#6` (active).
fn closed_blocker() -> FakeTracker {
    let fake = tracker();
    seed(&fake, 3, false, DONE, &[], 0);
    seed(&fake, 5, true, BLOCKED, &[(3, false)], 0);
    seed(&fake, 6, true, PLANNING, &[(3, false)], 0);
    fake.set_dependencies(
        &IssueNumber(3),
        Direction::Blocking,
        vec![dependency(5, true), dependency(6, true)],
    );
    fake
}

#[tokio::test]
async fn a_duplicate_delivery_writes_and_reports_once() {
    for split in [false, true] {
        let mut plane = plane(closed_blocker(), StatusMode::Off);
        plane.sync.on_delivery(&closed(3));
        if split {
            plane.worker.drain().await;
        }
        plane.sync.on_delivery(&closed(3));
        plane.worker.drain().await;

        let writes = plane.fake.writes();
        assert_eq!(
            set_fields(&writes, 5),
            vec![Some(QUEUED.to_owned())],
            "split {split}"
        );
        assert_eq!(posts(&writes, 6).len(), 1, "split {split}");
        assert!(set_fields(&writes, 6).is_empty(), "split {split}");
        assert!(!touched(&writes, 3), "split {split}");
    }
}

#[tokio::test]
async fn the_two_deliveries_of_one_link_report_once() {
    for split in [false, true] {
        let fake = tracker();
        seed(&fake, 3, true, PLANNING, &[], 0);
        seed(&fake, 6, true, PLANNING, &[(3, true)], 0);
        let mut plane = plane(fake, StatusMode::Off);
        plane.sync.on_delivery(&linked("blocked_by", 6, 3));
        if split {
            plane.worker.drain().await;
        }
        plane.sync.on_delivery(&linked("blocking", 6, 3));
        plane.worker.drain().await;

        let writes = plane.fake.writes();
        assert_eq!(posts(&writes, 6).len(), 1, "split {split}");
        assert!(set_fields(&writes, 6).is_empty(), "split {split}");
        assert!(!touched(&writes, 3), "split {split}");
    }
}

#[tokio::test]
async fn close_then_reopen_out_of_order_follows_the_facts() {
    for split in [false, true] {
        let fake = tracker();
        // The issue was closed and reopened; the reopen arrives first.
        seed(&fake, 8, true, DONE, &[], 0);
        let mut plane = plane(fake, StatusMode::Off);
        plane.sync.on_delivery(&reopened(8));
        if split {
            plane.worker.drain().await;
        }
        plane.sync.on_delivery(&closed(8));
        plane.worker.drain().await;

        let writes = plane.fake.writes();
        assert_eq!(
            set_fields(&writes, 8),
            vec![Some(QUEUED.to_owned())],
            "split {split}"
        );
        assert_eq!(
            plane
                .fake
                .field(&IssueNumber(8), &field_source())
                .as_deref(),
            Some(QUEUED)
        );
    }
}

#[tokio::test]
async fn re_evaluating_after_its_own_write_writes_nothing() {
    let fake = closed_blocker();
    let settings = settings(StatusMode::IssueStatus);
    let first = evaluate(&fake, &settings, IssueNumber(5), &no_change()).await;
    assert_eq!(first.outcome, Outcome::Written);
    let before = fake.writes();
    assert_eq!(set_fields(&before, 5), vec![Some(QUEUED.to_owned())]);
    assert_eq!(set_statuses(&before, 5), vec!["Todo".to_owned()]);

    let second = evaluate(&fake, &settings, IssueNumber(5), &no_change()).await;
    assert_eq!(second.outcome, Outcome::Unchanged);
    assert_eq!(fake.writes(), before);
}

#[tokio::test]
async fn an_unreadable_blocker_counts_as_open() {
    let fake = tracker();
    // Two blockers each, of which the listing returns only one, closed.
    seed(&fake, 5, true, QUEUED, &[(3, false)], 1);
    seed(&fake, 6, true, PLANNING, &[(3, false)], 1);
    let settings = settings(StatusMode::Off);

    let queued = evaluate(&fake, &settings, IssueNumber(5), &no_change()).await;
    assert_eq!(queued.outcome, Outcome::Written);
    assert_eq!(
        set_fields(&fake.writes(), 5),
        vec![Some(BLOCKED.to_owned())]
    );

    let active = evaluate(
        &fake,
        &settings,
        IssueNumber(6),
        &change(3, BlockerChangeKind::Closed),
    )
    .await;
    assert_eq!(active.outcome, Outcome::Written);
    let posted = posts(&fake.writes(), 6);
    assert_eq!(posted.len(), 1);
    assert!(
        posted[0].contains(&report::key(&[], 1, REPO)),
        "{}",
        posted[0]
    );
    assert!(posted[0].contains("unreadable=1"));
}

#[tokio::test]
async fn a_blocked_parent_with_sub_issues_stays_blocked() {
    let fake = tracker();
    fake.add_issue(issue(9, true, 0, 2));
    fake.put_field(&IssueNumber(9), field_source(), BLOCKED);
    let settings = settings(StatusMode::Off);
    for cause in [
        Cause::Sweep,
        no_change(),
        change(4, BlockerChangeKind::Removed),
    ] {
        let evaluation = evaluate(&fake, &settings, IssueNumber(9), &cause).await;
        assert_eq!(evaluation.outcome, Outcome::Unchanged);
    }
    assert!(fake.writes().is_empty());
}

#[tokio::test]
async fn a_reopen_gives_blocked_or_queued() {
    for (blockers, expected) in [(vec![(4, true)], BLOCKED), (vec![(4, false)], QUEUED)] {
        let fake = tracker();
        seed(&fake, 8, true, DONE, &blockers, 0);
        let mut plane = plane(fake, StatusMode::Off);
        plane.sync.on_delivery(&reopened(8));
        plane.worker.drain().await;
        assert_eq!(
            set_fields(&plane.fake.writes(), 8),
            vec![Some(expected.to_owned())]
        );
    }
}

#[tokio::test]
async fn an_active_issue_is_reported_not_written() {
    let fake = tracker();
    seed(&fake, 6, true, PLANNING, &[(3, true), (4, false)], 0);
    let evaluation = evaluate(
        &fake,
        &settings(StatusMode::IssueStatus),
        IssueNumber(6),
        &change(4, BlockerChangeKind::Closed),
    )
    .await;
    assert_eq!(evaluation.outcome, Outcome::Written);
    assert_eq!(evaluation.line["reported"], true);
    let writes = fake.writes();
    assert!(set_fields(&writes, 6).is_empty());
    assert!(set_statuses(&writes, 6).is_empty());
    let posted = posts(&writes, 6);
    assert_eq!(posted.len(), 1);
    assert!(posted[0].contains("<!-- mintswarm: blockers open=#3 unreadable=0 -->"));
    assert!(posted[0].contains(PLANNING));
}

#[tokio::test]
async fn a_value_changed_between_read_and_write_is_kept() {
    // The session moves the value after the first read: the restart sees it and writes nothing.
    let fake = tracker();
    seed(&fake, 5, true, BLOCKED, &[], 0);
    fake.script_field_changes(
        &IssueNumber(5),
        field_source(),
        vec![Some(PLANNING.to_owned())],
    );
    let settings = settings(StatusMode::Off);
    let evaluation = evaluate(&fake, &settings, IssueNumber(5), &no_change()).await;
    assert_eq!(evaluation.outcome, Outcome::Unchanged);
    assert!(fake.writes().is_empty());
    assert_eq!(
        fake.field(&IssueNumber(5), &field_source()).as_deref(),
        Some(PLANNING)
    );

    // The value moves on every read, through the requeue too: deferred twice, never written.
    let fake = tracker();
    seed(&fake, 5, true, BLOCKED, &[], 0);
    // Each evaluation reads the value four times, and each read moves it.
    let moves = [DONE, BLOCKED]
        .repeat(4)
        .into_iter()
        .map(|name| Some(name.to_owned()))
        .collect();
    fake.script_field_changes(&IssueNumber(5), field_source(), moves);
    let mut plane = plane(fake, StatusMode::Off);
    plane.sync.enqueue(IssueNumber(5), no_change());
    plane.worker.drain().await;
    let lines = plane.lines.lock().unwrap().clone();
    let outcomes: Vec<&Value> = lines.iter().map(|line| &line["outcome"]).collect();
    assert_eq!(outcomes, vec!["deferred", "deferred"]);
    assert!(plane.fake.writes().is_empty());
    assert_eq!(plane.sync.queued(), 0);
}

#[tokio::test]
async fn a_failed_issue_read_ends_failed_and_writes_nothing() {
    let fake = closed_blocker();
    fake.unreadable(&IssueNumber(5));
    let evaluation = evaluate(
        &fake,
        &settings(StatusMode::IssueStatus),
        IssueNumber(5),
        &no_change(),
    )
    .await;
    assert_eq!(evaluation.outcome, Outcome::Failed);
    assert!(fake.writes().is_empty());
}

#[tokio::test]
async fn status_off_writes_no_status() {
    let fake = tracker();
    // A write case and a wrong-status case.
    seed(&fake, 5, true, DONE, &[], 0);
    seed(&fake, 7, true, QUEUED, &[], 0);
    fake.put_status(&IssueNumber(7), "Done");
    let settings = settings(StatusMode::Off);
    for number in [5, 7] {
        evaluate(&fake, &settings, IssueNumber(number), &no_change()).await;
    }
    let writes = fake.writes();
    assert_eq!(set_fields(&writes, 5), vec![Some(QUEUED.to_owned())]);
    assert!(set_statuses(&writes, 5).is_empty());
    assert!(set_statuses(&writes, 7).is_empty());
}

/// Whether the issue is open, its value, its listed blockers and the status the value maps to.
type Case<'a> = (bool, &'a str, &'a [(u64, bool)], &'a str);

#[tokio::test]
async fn a_wrong_status_is_repaired() {
    let settings = settings(StatusMode::IssueStatus);
    let cases: [Case<'_>; 3] = [
        (true, QUEUED, &[], "Todo"),
        (true, BLOCKED, &[(4, true)], "Blocked"),
        (false, DONE, &[], "Done"),
    ];
    for (open, value, blockers, status) in cases {
        let fake = tracker();
        seed(&fake, 5, open, value, blockers, 0);
        fake.put_status(&IssueNumber(5), "In progress");
        let evaluation = evaluate(&fake, &settings, IssueNumber(5), &Cause::Sweep).await;
        assert_eq!(evaluation.outcome, Outcome::Written, "{value}");
        let writes = fake.writes();
        assert_eq!(set_statuses(&writes, 5), vec![status.to_owned()], "{value}");
        assert!(set_fields(&writes, 5).is_empty(), "{value}");

        // A right status is left alone.
        let again = evaluate(&fake, &settings, IssueNumber(5), &Cause::Sweep).await;
        assert_eq!(again.outcome, Outcome::Unchanged, "{value}");
        assert_eq!(fake.writes(), writes, "{value}");
    }
}

/// A tracker whose field reads as a labels conflict of two configured values, passing every
/// other call to an inner fake, which does not model labels.
struct ConflictingField(FakeTracker);

impl TrackerBase for ConflictingField {
    type Error = FakeTrackerError;
}

impl Issues for ConflictingField {
    async fn fetch_ready(&self) -> Result<Vec<Issue>, Self::Error> {
        self.0.fetch_ready().await
    }

    async fn read_issue(
        &self,
        issue: IssueNumber,
        since: Option<Cursor>,
    ) -> Result<IssueRead, Self::Error> {
        self.0.read_issue(issue, since).await
    }

    async fn create_sub_item(
        &self,
        parent: IssueNumber,
        item: SubItem,
    ) -> Result<Issue, Self::Error> {
        self.0.create_sub_item(parent, item).await
    }

    async fn set_issue_type(&self, issue: IssueNumber, kind: IssueType) -> Result<(), Self::Error> {
        self.0.set_issue_type(issue, kind).await
    }
}

impl Links for ConflictingField {
    async fn read_links(&self, issue: IssueNumber) -> Result<Vec<Link>, Self::Error> {
        self.0.read_links(issue).await
    }

    async fn link_dependency(
        &self,
        issue: IssueNumber,
        blocker: IssueNumber,
    ) -> Result<(), Self::Error> {
        self.0.link_dependency(issue, blocker).await
    }

    async fn read_dependencies(
        &self,
        issue: IssueNumber,
        direction: Direction,
    ) -> Result<Vec<Dependency>, Self::Error> {
        self.0.read_dependencies(issue, direction).await
    }
}

impl Statuses for ConflictingField {
    async fn read_status(&self, issue: IssueNumber) -> Result<Option<String>, Self::Error> {
        self.0.read_status(issue).await
    }

    async fn set_status(&self, issue: IssueNumber, status: String) -> Result<(), Self::Error> {
        self.0.set_status(issue, status).await
    }
}

impl Comments for ConflictingField {
    async fn post_comment(&self, issue: IssueNumber, body: String) -> Result<Comment, Self::Error> {
        self.0.post_comment(issue, body).await
    }
}

impl Fields for ConflictingField {
    async fn read_field(
        &self,
        _issue: IssueNumber,
        _field: &FieldRef,
    ) -> Result<FieldRead, FieldError> {
        let value = |name: &str| FieldValue {
            field_id: None,
            option_id: None,
            name: name.to_owned(),
        };
        Ok(FieldRead::Conflict(vec![value(QUEUED), value(DONE)]))
    }

    async fn set_field(
        &self,
        issue: IssueNumber,
        field: &FieldRef,
        option: Option<&str>,
    ) -> Result<(), FieldError> {
        self.0.set_field(issue, field, option).await
    }
}

#[tokio::test]
async fn the_outcome_mapping() {
    let settings = settings(StatusMode::IssueStatus);
    let five = IssueNumber(5);

    // An untrusted issue.
    let fake = tracker();
    seed(&fake, 5, true, DONE, &[], 0);
    let mut stranger = issue(5, true, 0, 0);
    stranger.author = Login("stranger".to_owned());
    fake.add_issue(stranger);
    let evaluation = evaluate(&fake, &settings, five.clone(), &no_change()).await;
    assert_eq!(evaluation.outcome, Outcome::Skipped);
    assert!(fake.writes().is_empty());

    // An unconfigured value.
    let fake = tracker();
    seed(&fake, 5, false, "a value nobody configured", &[], 0);
    let evaluation = evaluate(&fake, &settings, five.clone(), &no_change()).await;
    assert_eq!(evaluation.outcome, Outcome::Skipped);
    assert!(fake.writes().is_empty());

    // A labels conflict.
    let conflict = ConflictingField(tracker());
    seed(&conflict.0, 5, false, DONE, &[], 0);
    let evaluation = evaluate(&conflict, &settings, five.clone(), &no_change()).await;
    assert_eq!(evaluation.outcome, Outcome::Failed);
    assert!(conflict.0.writes().is_empty());

    // A field transport error and a missing option.
    for error in [
        FieldError::Transport("connection reset".to_owned()),
        FieldError::Field(FieldFinding::OptionMissing),
    ] {
        let fake = tracker();
        seed(&fake, 5, true, DONE, &[], 0);
        fake.fail_fields(&five, error.clone());
        let evaluation = evaluate(&fake, &settings, five.clone(), &no_change()).await;
        assert_eq!(evaluation.outcome, Outcome::Failed, "{error}");
        assert!(fake.writes().is_empty(), "{error}");
    }

    // A value write followed by a status write the tracker refuses.
    let fake = tracker().with_statuses(["Blocked", "Done"]);
    seed(&fake, 5, true, DONE, &[], 0);
    let evaluation = evaluate(&fake, &settings, five.clone(), &no_change()).await;
    assert_eq!(evaluation.outcome, Outcome::Failed);
    let writes = fake.writes();
    assert_eq!(set_fields(&writes, 5), vec![Some(QUEUED.to_owned())]);
    assert!(set_statuses(&writes, 5).is_empty());
    assert_eq!(evaluation.line["wrote"], serde_json::json!(["value"]));
}
