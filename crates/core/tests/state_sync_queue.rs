//! The state-sync hand-off, queue and worker: the trigger reduction, a hand-off that never
//! calls the tracker, merged causes, arrival order, fan-out and the settle repeat.
// Not redundant: clippy's `allow-unwrap-in-tests` covers a `tests/` crate only under `cfg(test)`.
#![cfg(test)]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use mintswarm_config::{
    AWAITING_APPROVAL, Action, ActionTable, ActionTrust, BLOCKED, DONE, PLANNING, People, QUEUED,
    REQUIRED_OPTIONS, StateOption, StateSettings, StatusMode, Trust, TrustMatch,
};
use mintswarm_core::state_sync::queue::{Entry, SETTLE};
use mintswarm_core::state_sync::rule::{BlockerChange, BlockerChangeKind, Cause};
use mintswarm_core::state_sync::trigger::reduce;
use mintswarm_core::state_sync::{Settings, StateSync, Worker};
use mintswarm_core::testing::tracker::{FakeTracker, FakeTrackerError, FakeWrite};
use mintswarm_core::watch::WEBHOOK_KIND;
use mintswarm_domain::field::{FieldError, FieldRead, FieldRef, FieldSource};
use mintswarm_domain::ids::IssueNumber;
use mintswarm_domain::tracker::{
    Collaborators, Comment, Comments, Cursor, Dependency, Direction, Fields, Issue, IssueRead,
    IssueType, Issues, Link, LinkKind, Links, Login, ParentIssue, RepoRole, Statuses, SubItem,
    TrackerBase,
};
use mintswarm_proto::watch::v1::{
    GitHubWebhook, IssueDependency, ItemKind, ItemRef, Subject, WatchEvent, watch_event,
};
use serde_json::{Value, json};

const REPO: &str = "mintswarm/mintswarm";
const OTHER: &str = "someone/else";
const BOT: &str = "bee[bot]";
const MAINTAINER: &str = "maintainer";
const PEER: &str = "bee-keeper[bot]";

fn field_source() -> FieldSource {
    FieldSource::Field {
        name: "msw-state".to_owned(),
    }
}

fn settings() -> Settings {
    Settings {
        repository: REPO.to_owned(),
        bot: Login(BOT.to_owned()),
        peer_logins: vec![Login(PEER.to_owned())],
        trust: Trust {
            labels: vec!["mintswarm".to_owned()],
            authors: vec![Login(MAINTAINER.to_owned())],
            r#match: TrustMatch::Any,
        },
        state: StateSettings {
            source: field_source(),
            status: StatusMode::Off,
            options: REQUIRED_OPTIONS
                .iter()
                .map(|name| StateOption {
                    name: (*name).to_owned(),
                    status: None,
                })
                .collect(),
        },
        actions: owner_rows(),
    }
}

fn owner_rows() -> ActionTable {
    let owner = ActionTrust {
        people: People::IssueOwner,
        apps: Vec::new(),
    };
    ActionTable::default()
        .with(Action::ApprovePlan, owner.clone())
        .with(Action::ApproveDesign, owner.clone())
        .with(Action::ApproveDecomposition, owner)
}

fn dependency(repository: &str, number: u64, open: bool) -> Dependency {
    Dependency {
        repository: repository.to_owned(),
        number: IssueNumber(number),
        open,
    }
}

/// Seeds issue `number` with `value` and its listed blockers.
fn seed(fake: &FakeTracker, number: u64, open: bool, value: &str, blockers: &[(u64, bool)]) {
    let listed: Vec<Dependency> = blockers
        .iter()
        .map(|(blocker, open)| dependency(REPO, *blocker, *open))
        .collect();
    fake.add_issue(Issue {
        number: IssueNumber(number),
        title: format!("Issue {number}"),
        body: String::new(),
        state: if open { "open" } else { "closed" }.to_owned(),
        labels: Vec::new(),
        issue_type: Some(IssueType::Implementation),
        author: Login(MAINTAINER.to_owned()),
        author_is_bot: false,
        url: format!("https://github.com/{REPO}/issues/{number}"),
        sub_issues: 0,
        blocked_by: listed.len() as u64,
    });
    fake.set_dependencies(&IssueNumber(number), Direction::BlockedBy, listed);
    fake.put_field(&IssueNumber(number), field_source(), value);
}

#[derive(Clone, Copy)]
struct Side {
    repository: &'static str,
    number: u64,
}

fn side(repository: &'static str, number: u64) -> Side {
    Side { repository, number }
}

fn webhook_event(
    event: &str,
    action: &str,
    kind: ItemKind,
    subject: Side,
    dependency: Option<(Side, Side)>,
) -> WatchEvent {
    let item = |side: Side| ItemRef {
        repository: side.repository.to_owned(),
        number: side.number,
    };
    WatchEvent {
        kind: WEBHOOK_KIND.to_owned(),
        subject: Some(Subject {
            repository: subject.repository.to_owned(),
            item_kind: kind.into(),
            number: subject.number,
            ..Subject::default()
        }),
        body: Some(watch_event::Body::GithubWebhook(GitHubWebhook {
            event: event.to_owned(),
            action: action.to_owned(),
            dependency: dependency.map(|(blocked, blocking)| IssueDependency {
                blocked: Some(item(blocked)),
                blocking: Some(item(blocking)),
            }),
            ..GitHubWebhook::default()
        })),
        ..WatchEvent::default()
    }
}

fn issues_event(action: &str, number: u64) -> WatchEvent {
    webhook_event("issues", action, ItemKind::Issue, side(REPO, number), None)
}

/// The `blocked_by_<change>` delivery of a link of `blocked` to `blocking`.
fn link_event(change: &str, blocked: u64, blocking: u64) -> WatchEvent {
    webhook_event(
        "issue_dependencies",
        &format!("blocked_by_{change}"),
        ItemKind::Issue,
        side(REPO, blocked),
        Some((side(REPO, blocked), side(REPO, blocking))),
    )
}

fn triggered(changes: &[(u64, BlockerChangeKind)]) -> Cause {
    Cause::Triggered {
        blocker_changes: changes
            .iter()
            .map(|(blocker, change)| BlockerChange {
                blocker: IssueNumber(*blocker),
                repository: REPO.to_owned(),
                change: *change,
            })
            .collect(),
    }
}

fn entry(cause: Cause, fan_out: bool) -> Entry {
    let mut entry = Entry::of(cause);
    entry.fan_out = fan_out;
    entry
}

#[test]
fn issue_events_reduce_to_the_issue_marked_for_fan_out() {
    // The change the fan-out gives comes from the issue's state at fan-out time, not the action.
    let marked = true;
    let cases: Vec<(WatchEvent, Vec<(IssueNumber, Entry)>)> = vec![
        (
            issues_event("closed", 3),
            vec![(IssueNumber(3), entry(triggered(&[]), marked))],
        ),
        (
            issues_event("reopened", 3),
            vec![(IssueNumber(3), entry(triggered(&[]), marked))],
        ),
        (issues_event("labeled", 3), vec![]),
        (
            webhook_event(
                "issues",
                "closed",
                ItemKind::Issue,
                side("MintSwarm/MintSwarm", 3),
                None,
            ),
            vec![(IssueNumber(3), entry(triggered(&[]), marked))],
        ),
        (
            webhook_event("issues", "closed", ItemKind::Issue, side(OTHER, 3), None),
            vec![],
        ),
        (
            webhook_event(
                "issues",
                "closed",
                ItemKind::PullRequest,
                side(REPO, 3),
                None,
            ),
            vec![],
        ),
        (
            webhook_event(
                "issue_comment",
                "created",
                ItemKind::Issue,
                side(REPO, 3),
                None,
            ),
            vec![],
        ),
        (
            WatchEvent {
                kind: "other.kind".to_owned(),
                ..issues_event("closed", 3)
            },
            vec![],
        ),
    ];
    for (event, expected) in cases {
        assert_eq!(reduce(&event, REPO), expected, "{event:?}");
    }
}

#[test]
fn dependency_events_reduce_to_the_blocked_issue() {
    let added = BlockerChangeKind::Added;
    let removed = BlockerChangeKind::Removed;
    let link = |action: &str, blocked: Side, blocking: Side| {
        webhook_event(
            "issue_dependencies",
            action,
            ItemKind::Issue,
            blocked,
            Some((blocked, blocking)),
        )
    };
    let cases = vec![
        (
            link("blocked_by_added", side(REPO, 6), side(REPO, 3)),
            Some(added),
        ),
        (
            link("blocking_added", side(REPO, 6), side(REPO, 3)),
            Some(added),
        ),
        (
            link("blocked_by_removed", side(REPO, 6), side(REPO, 3)),
            Some(removed),
        ),
        (
            link("blocking_removed", side(REPO, 6), side(REPO, 3)),
            Some(removed),
        ),
        // A blocked issue in another repository is not this plane's.
        (link("blocking_added", side(OTHER, 6), side(REPO, 3)), None),
        (link("something_else", side(REPO, 6), side(REPO, 3)), None),
    ];
    for (event, change) in cases {
        let expected: Vec<(IssueNumber, Entry)> = change
            .map(|change| (IssueNumber(6), entry(triggered(&[(3, change)]), false)))
            .into_iter()
            .collect();
        assert_eq!(reduce(&event, REPO), expected, "{event:?}");
    }

    // A blocker in another repository is a change named with its own repository, so it is
    // never taken for `#3` here.
    for (action, change) in [("blocked_by_added", added), ("blocked_by_removed", removed)] {
        let event = link(action, side(REPO, 6), side(OTHER, 3));
        let cause = Cause::Triggered {
            blocker_changes: vec![BlockerChange {
                blocker: IssueNumber(3),
                repository: OTHER.to_owned(),
                change,
            }],
        };
        let expected = vec![(IssueNumber(6), entry(cause, false))];
        assert_eq!(reduce(&event, REPO), expected, "{event:?}");
    }

    // A blocking side without a repository is in this one.
    let event = link("blocked_by_added", side(REPO, 6), side("", 3));
    let expected = vec![(IssueNumber(6), entry(triggered(&[(3, added)]), false))];
    assert_eq!(reduce(&event, REPO), expected, "{event:?}");
}

#[test]
fn a_merge_removes_duplicate_changes() {
    let mut ours = entry(triggered(&[(3, BlockerChangeKind::Added)]), false);
    ours.repeat = true;
    let mut theirs = entry(
        triggered(&[
            (3, BlockerChangeKind::Added),
            (4, BlockerChangeKind::Removed),
        ]),
        true,
    );
    theirs.requeued = true;
    ours.merge(theirs);
    assert_eq!(
        ours.cause,
        Some(triggered(&[
            (3, BlockerChangeKind::Added),
            (4, BlockerChangeKind::Removed)
        ]))
    );
    assert!(ours.fan_out);
    assert!(!ours.repeat);
    assert!(ours.requeued);

    let mut sweep = Entry::of(Cause::Sweep);
    sweep.merge(Entry::of(Cause::Sweep));
    assert_eq!(sweep.cause, Some(Cause::Sweep));
    sweep.merge(entry(triggered(&[]), false));
    assert_eq!(sweep.cause, Some(triggered(&[])));
    assert!(!sweep.repeat);
}

#[test]
fn a_repeat_merged_with_a_sweep_stays_a_repeat() {
    let repeat = || {
        let mut entry = entry(triggered(&[]), false);
        entry.repeat = true;
        entry
    };
    for (mut ours, theirs) in [
        (Entry::of(Cause::Sweep), repeat()),
        (repeat(), Entry::of(Cause::Sweep)),
    ] {
        ours.merge(theirs);
        assert_eq!(ours.cause, Some(triggered(&[])));
        assert!(ours.repeat);
    }

    // A fresh trigger still absorbs the repeat.
    let mut ours = repeat();
    ours.merge(entry(triggered(&[(3, BlockerChangeKind::Added)]), false));
    assert!(!ours.repeat);
}

/// A tracker every call of which is counted and then never answers.
#[derive(Default)]
struct Stalling {
    calls: AtomicUsize,
}

impl Stalling {
    async fn stall<R>(&self) -> R {
        self.calls.fetch_add(1, Ordering::SeqCst);
        std::future::pending().await
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl TrackerBase for Stalling {
    type Error = FakeTrackerError;
}

impl Issues for Stalling {
    async fn fetch_ready(&self) -> Result<Vec<Issue>, Self::Error> {
        self.stall().await
    }

    async fn read_issue(
        &self,
        _: IssueNumber,
        _: Option<Cursor>,
    ) -> Result<IssueRead, Self::Error> {
        self.stall().await
    }

    async fn create_sub_item(&self, _: IssueNumber, _: SubItem) -> Result<Issue, Self::Error> {
        self.stall().await
    }

    async fn set_issue_type(&self, _: IssueNumber, _: IssueType) -> Result<(), Self::Error> {
        self.stall().await
    }
}

impl Links for Stalling {
    async fn read_links(&self, _: IssueNumber) -> Result<Vec<Link>, Self::Error> {
        self.stall().await
    }

    async fn link_dependency(&self, _: IssueNumber, _: IssueNumber) -> Result<(), Self::Error> {
        self.stall().await
    }

    async fn read_dependencies(
        &self,
        _: IssueNumber,
        _: Direction,
    ) -> Result<Vec<Dependency>, Self::Error> {
        self.stall().await
    }
}

impl Statuses for Stalling {
    async fn read_status(&self, _: IssueNumber) -> Result<Option<String>, Self::Error> {
        self.stall().await
    }

    async fn set_status(&self, _: IssueNumber, _: String) -> Result<(), Self::Error> {
        self.stall().await
    }
}

impl Comments for Stalling {
    async fn post_comment(&self, _: IssueNumber, _: String) -> Result<Comment, Self::Error> {
        self.stall().await
    }
}

impl Collaborators for Stalling {
    async fn read_permission(&self, _: Login) -> Result<RepoRole, Self::Error> {
        self.stall().await
    }

    async fn read_parent(&self, _: IssueNumber) -> Result<Option<ParentIssue>, Self::Error> {
        self.stall().await
    }
}

impl Fields for Stalling {
    async fn read_field(&self, _: IssueNumber, _: &FieldRef) -> Result<FieldRead, FieldError> {
        self.stall().await
    }

    async fn set_field(
        &self,
        _: IssueNumber,
        _: &FieldRef,
        _: Option<&str>,
    ) -> Result<(), FieldError> {
        self.stall().await
    }
}

#[test]
fn on_delivery_never_calls_the_tracker() {
    let tracker = Arc::new(Stalling::default());
    let (sync, _worker) = StateSync::new(Arc::clone(&tracker), settings());
    sync.on_delivery(&issues_event("closed", 3));
    sync.on_delivery(&link_event("added", 6, 3));
    sync.on_delivery(&webhook_event(
        "issue_comment",
        "created",
        ItemKind::Issue,
        side(REPO, 6),
        None,
    ));
    assert_eq!(tracker.calls(), 0);
    assert_eq!(sync.queued(), 2);
}

#[tokio::test]
async fn a_stalled_worker_does_not_hold_the_hand_off() {
    let tracker = Arc::new(Stalling::default());
    let (sync, worker) = StateSync::new(Arc::clone(&tracker), settings());
    let running = tokio::spawn(worker.run());
    sync.on_delivery(&issues_event("closed", 3));
    for _ in 0..100 {
        if tracker.calls() > 0 {
            break;
        }
        tokio::task::yield_now().await;
    }
    assert_eq!(tracker.calls(), 1, "the worker is stuck in its first call");
    assert_eq!(sync.queued(), 0);

    sync.on_delivery(&link_event("added", 6, 3));
    sync.on_delivery(&link_event("added", 6, 4));
    sync.on_delivery(&issues_event("closed", 3));
    assert_eq!(sync.queued(), 2);
    assert_eq!(tracker.calls(), 1);
    running.abort();
}

/// A hand-off and worker over a fake, whose lines are kept.
struct Plane<T> {
    fake: Arc<T>,
    sync: StateSync,
    worker: Worker<T>,
    lines: Arc<Mutex<Vec<Value>>>,
}

fn plane<T>(fake: T) -> Plane<T>
where
    T: Issues + Links + Fields + Statuses + Comments + Collaborators,
{
    let fake = Arc::new(fake);
    let lines = Arc::new(Mutex::new(Vec::new()));
    let sink_lines = Arc::clone(&lines);
    let (sync, worker) = StateSync::new(Arc::clone(&fake), settings());
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

impl<T> Plane<T> {
    /// State-sync's lines, in order.
    fn lines(&self) -> Vec<Value> {
        self.lines_where(|line| line["evaluator"] == "state_sync")
    }

    /// Approvals' lines, in order.
    fn approvals_lines(&self) -> Vec<Value> {
        self.lines_where(|line| line["evaluator"] == "approvals")
    }

    fn lines_where(&self, keep: impl Fn(&Value) -> bool) -> Vec<Value> {
        self.lines
            .lock()
            .unwrap()
            .iter()
            .filter(|line| keep(line))
            .cloned()
            .collect()
    }

    /// Every line, both evaluators', in order.
    fn all_lines(&self) -> Vec<Value> {
        self.lines_where(|_| true)
    }

    fn lines_of(&self, number: u64) -> Vec<Value> {
        self.lines()
            .into_iter()
            .filter(|line| line["issue"] == number)
            .collect()
    }
}

fn tracker() -> FakeTracker {
    FakeTracker::new(Login(BOT.to_owned()))
}

fn posts(writes: &[FakeWrite], number: u64) -> usize {
    writes
        .iter()
        .filter(|write| matches!(write, FakeWrite::PostComment { issue, .. } if issue.0 == number))
        .count()
}

#[tokio::test]
async fn two_entries_for_one_issue_make_one_evaluation() {
    let fake = tracker();
    seed(&fake, 7, true, PLANNING, &[(3, true), (4, true)]);
    let mut plane = plane(fake);
    plane.sync.on_delivery(&link_event("added", 7, 3));
    plane.sync.on_delivery(&link_event("added", 7, 4));
    assert_eq!(plane.sync.queued(), 1);
    plane.worker.drain().await;

    let lines = plane.lines();
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0]["issue"], 7);
    assert_eq!(
        lines[0]["causes"],
        json!([
            { "blocker": "#3", "change": "added" },
            { "blocker": "#4", "change": "added" },
        ])
    );
    assert_eq!(posts(&plane.fake.writes(), 7), 1);
}

#[tokio::test]
async fn a_sweep_entry_is_absorbed_by_a_triggered_one() {
    let fake = tracker();
    seed(&fake, 7, true, PLANNING, &[(3, true)]);
    let mut plane = plane(fake);
    plane.sync.enqueue(IssueNumber(7), Cause::Sweep);
    plane.sync.on_delivery(&link_event("added", 7, 3));
    plane.worker.drain().await;

    let lines = plane.lines();
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0]["cause"], "triggered");
    assert_eq!(lines[0]["reported"], true);
}

#[tokio::test]
async fn entries_are_evaluated_in_arrival_order() {
    let fake = tracker();
    seed(&fake, 9, true, PLANNING, &[]);
    seed(&fake, 3, true, PLANNING, &[]);
    let mut plane = plane(fake);
    for number in [9, 3, 9] {
        plane.sync.enqueue(IssueNumber(number), Cause::Sweep);
    }
    plane.worker.drain().await;
    let order: Vec<Value> = plane
        .lines()
        .into_iter()
        .map(|line| line["issue"].clone())
        .collect();
    assert_eq!(order, vec![json!(9), json!(3)]);
}

/// A fake whose first read of the issues an issue blocks fails; every other call, later
/// `Blocking` reads included, goes to the inner fake.
struct FailFirstBlocking {
    inner: FakeTracker,
    failed: AtomicUsize,
}

impl TrackerBase for FailFirstBlocking {
    type Error = FakeTrackerError;
}

impl Issues for FailFirstBlocking {
    async fn fetch_ready(&self) -> Result<Vec<Issue>, Self::Error> {
        self.inner.fetch_ready().await
    }

    async fn read_issue(
        &self,
        issue: IssueNumber,
        since: Option<Cursor>,
    ) -> Result<IssueRead, Self::Error> {
        self.inner.read_issue(issue, since).await
    }

    async fn create_sub_item(
        &self,
        parent: IssueNumber,
        item: SubItem,
    ) -> Result<Issue, Self::Error> {
        self.inner.create_sub_item(parent, item).await
    }

    async fn set_issue_type(&self, issue: IssueNumber, kind: IssueType) -> Result<(), Self::Error> {
        self.inner.set_issue_type(issue, kind).await
    }
}

impl Links for FailFirstBlocking {
    async fn read_links(&self, issue: IssueNumber) -> Result<Vec<Link>, Self::Error> {
        self.inner.read_links(issue).await
    }

    async fn link_dependency(
        &self,
        issue: IssueNumber,
        blocker: IssueNumber,
    ) -> Result<(), Self::Error> {
        self.inner.link_dependency(issue, blocker).await
    }

    async fn read_dependencies(
        &self,
        issue: IssueNumber,
        direction: Direction,
    ) -> Result<Vec<Dependency>, Self::Error> {
        if direction == Direction::Blocking && self.failed.fetch_add(1, Ordering::SeqCst) == 0 {
            return Err(FakeTrackerError::Unreadable { issue });
        }
        self.inner.read_dependencies(issue, direction).await
    }
}

impl Statuses for FailFirstBlocking {
    async fn read_status(&self, issue: IssueNumber) -> Result<Option<String>, Self::Error> {
        self.inner.read_status(issue).await
    }

    async fn set_status(&self, issue: IssueNumber, status: String) -> Result<(), Self::Error> {
        self.inner.set_status(issue, status).await
    }
}

impl Comments for FailFirstBlocking {
    async fn post_comment(&self, issue: IssueNumber, body: String) -> Result<Comment, Self::Error> {
        self.inner.post_comment(issue, body).await
    }
}

impl Collaborators for FailFirstBlocking {
    async fn read_permission(&self, login: Login) -> Result<RepoRole, Self::Error> {
        self.inner.read_permission(login).await
    }

    async fn read_parent(&self, issue: IssueNumber) -> Result<Option<ParentIssue>, Self::Error> {
        self.inner.read_parent(issue).await
    }
}

impl Fields for FailFirstBlocking {
    async fn read_field(
        &self,
        issue: IssueNumber,
        field: &FieldRef,
    ) -> Result<FieldRead, FieldError> {
        self.inner.read_field(issue, field).await
    }

    async fn set_field(
        &self,
        issue: IssueNumber,
        field: &FieldRef,
        option: Option<&str>,
    ) -> Result<(), FieldError> {
        self.inner.set_field(issue, field, option).await
    }
}

#[tokio::test(start_paused = true)]
async fn a_failed_fan_out_is_retried_by_the_settle_repeat() {
    let inner = tracker();
    seed(&inner, 3, false, DONE, &[]);
    seed(&inner, 5, true, BLOCKED, &[(3, false)]);
    inner.set_dependencies(
        &IssueNumber(3),
        Direction::Blocking,
        vec![dependency(REPO, 5, true), dependency(OTHER, 8, true)],
    );
    let mut plane = plane(FailFirstBlocking {
        inner,
        failed: AtomicUsize::new(0),
    });
    plane.sync.on_delivery(&issues_event("closed", 3));
    plane.worker.drain().await;
    let lines = plane.lines();
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0]["issue"], 3);
    assert_eq!(lines[0]["fan_out"], "failed");

    tokio::time::advance(SETTLE).await;
    plane.worker.drain().await;
    let lines = plane.lines();
    assert_eq!(lines.len(), 3, "{lines:?}");
    assert_eq!(lines[1]["issue"], 3);
    assert_eq!(lines[1]["repeat"], true);
    assert_eq!(lines[1]["fan_out"], "done");
    assert_eq!(lines[2]["issue"], 5);
    assert_eq!(lines[2]["value_after"], QUEUED);
    assert!(plane.lines_of(8).is_empty());

    // The repeat after a fan-out that succeeded carries no mark.
    tokio::time::advance(SETTLE).await;
    plane.worker.drain().await;
    let repeats: Vec<Value> = plane.lines_of(3);
    assert_eq!(repeats.len(), 2);
    let five = plane.lines_of(5);
    assert_eq!(five.len(), 2);
    assert_eq!(five[1]["repeat"], true);
    assert_eq!(five[1]["fan_out"], Value::Null);
}

#[tokio::test(start_paused = true)]
async fn a_triggered_evaluation_settles_once_without_a_report() {
    let fake = tracker();
    seed(&fake, 6, true, PLANNING, &[(3, true), (4, true)]);
    let mut plane = plane(fake);
    plane.sync.on_delivery(&link_event("added", 6, 4));
    plane.worker.drain().await;
    assert_eq!(plane.lines().len(), 1);
    assert_eq!(posts(&plane.fake.writes(), 6), 1);

    // A listed blocker closes without a delivery, so the open set changes.
    plane.fake.set_dependencies(
        &IssueNumber(6),
        Direction::BlockedBy,
        vec![dependency(REPO, 3, false), dependency(REPO, 4, true)],
    );
    tokio::time::advance(SETTLE - std::time::Duration::from_secs(1)).await;
    plane.worker.drain().await;
    assert_eq!(plane.lines().len(), 1, "not yet due");

    tokio::time::advance(std::time::Duration::from_secs(1)).await;
    plane.worker.drain().await;
    let lines = plane.lines();
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[1]["repeat"], true);
    assert_eq!(lines[1]["causes"], json!([]));
    assert_eq!(lines[1]["reported"], false);
    assert_eq!(posts(&plane.fake.writes(), 6), 1);

    tokio::time::advance(SETTLE).await;
    tokio::time::advance(SETTLE).await;
    plane.worker.drain().await;
    assert_eq!(plane.lines().len(), 2);
}

#[tokio::test(start_paused = true)]
async fn a_deferred_entry_settles_once() {
    let fake = tracker();
    seed(&fake, 5, true, BLOCKED, &[]);
    // The first evaluation reads the value four times and sees it move twice; it then rests at
    // `queued`.
    let moves = [DONE, BLOCKED, DONE, QUEUED]
        .into_iter()
        .map(|name| Some(name.to_owned()))
        .collect();
    fake.script_field_changes(&IssueNumber(5), field_source(), moves);
    let mut plane = plane(fake);
    plane.sync.on_delivery(&link_event("removed", 5, 3));
    plane.worker.drain().await;
    let outcomes: Vec<Value> = plane
        .lines()
        .into_iter()
        .map(|line| line["outcome"].clone())
        .collect();
    assert_eq!(outcomes, vec![json!("deferred"), json!("unchanged")]);
    assert!(plane.fake.writes().is_empty());

    tokio::time::advance(SETTLE).await;
    plane.worker.drain().await;
    tokio::time::advance(SETTLE).await;
    plane.worker.drain().await;
    let lines = plane.lines();
    assert_eq!(lines.len(), 3);
    assert_eq!(lines[2]["repeat"], true);
}

#[tokio::test(start_paused = true)]
async fn run_hands_off_and_logs() {
    let fake = Arc::new(tracker());
    seed(&fake, 7, true, PLANNING, &[(3, true)]);
    let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel::<String>();
    let (sync, worker) = StateSync::new(Arc::clone(&fake), settings());
    let worker = worker.with_sink(Arc::new(move |line: &str| {
        let _ = sender.send(line.to_owned());
    }));
    let running = tokio::spawn(worker.run());
    sync.on_delivery(&link_event("added", 7, 3));
    let line: Value = serde_json::from_str(&receiver.recv().await.unwrap()).unwrap();
    assert_eq!(line["issue"], 7);
    assert_eq!(line["reported"], true);
    running.abort();
}

const SHA: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

/// A comment on `number`, by `login`, at `minute`.
fn comment_on(id: u64, number: u64, login: &str, minute: u32, body: &str) -> Comment {
    let time = format!("2026-11-24T10:{minute:02}:00Z");
    Comment {
        id,
        author: Login(login.to_owned()),
        author_is_bot: login.ends_with("[bot]"),
        body: body.to_owned(),
        created_at: time.clone(),
        updated_at: time,
        url: format!("https://github.com/{REPO}/issues/{number}#issuecomment-{id}"),
    }
}

/// The plane's plan package on `number`.
fn plan_package(id: u64, number: u64) -> Comment {
    let body = format!(
        "{}\n{}\nPlease approve.",
        mintswarm_rules::command::PackageKind::Plan.marker(),
        mintswarm_rules::command::PackageKind::Plan.digest_line(SHA)
    );
    comment_on(id, number, BOT, 0, &body)
}

/// A person's comment delivery on issue `number`.
fn comment_event(number: u64) -> WatchEvent {
    webhook_event(
        "issue_comment",
        "created",
        ItemKind::Issue,
        side(REPO, number),
        None,
    )
}

#[tokio::test]
async fn state_sync_runs_before_approvals_for_one_number() {
    let fake = tracker();
    seed(&fake, 4, false, AWAITING_APPROVAL, &[]);
    fake.set_role(Login(MAINTAINER.to_owned()), RepoRole::Write);
    fake.add_comment(&IssueNumber(4), plan_package(10, 4));
    fake.add_comment(&IssueNumber(4), comment_on(11, 4, MAINTAINER, 5, "approve"));
    let mut plane = plane(fake);
    plane.sync.on_delivery(&issues_event("closed", 4));
    plane.sync.on_delivery(&comment_event(4));
    assert_eq!(plane.sync.queued(), 1);
    plane.worker.drain().await;

    let lines: Vec<Value> = plane
        .all_lines()
        .into_iter()
        .filter(|line| line["issue"] == 4)
        .collect();
    let evaluators: Vec<&Value> = lines.iter().map(|line| &line["evaluator"]).collect();
    assert_eq!(evaluators, ["state_sync", "approvals"]);
    assert_eq!(lines[0]["value_after"], DONE);
    assert_eq!(lines[1]["state"], DONE);
    assert_eq!(lines[1]["cause"], "triggered");
    assert_eq!(lines[1]["outcome"], "judged");
    assert_eq!(lines[1]["context"], "plan");
    assert_eq!(lines[1]["sha256"], SHA);
    assert_eq!(lines[1]["judged"][0]["comment"], 11);
    assert_eq!(lines[1]["judged"][0]["verdict"], "refused");
    assert_eq!(lines[1]["judged"][0]["check"], 5);
}

#[tokio::test]
async fn an_awaiting_issue_with_an_approve_is_judged_without_writes() {
    let fake = tracker();
    seed(&fake, 4, true, AWAITING_APPROVAL, &[]);
    fake.set_role(Login(MAINTAINER.to_owned()), RepoRole::Write);
    fake.add_comment(&IssueNumber(4), plan_package(10, 4));
    fake.add_comment(&IssueNumber(4), comment_on(11, 4, MAINTAINER, 5, "approve"));
    let mut plane = plane(fake);
    plane.sync.on_delivery(&comment_event(4));
    plane.worker.drain().await;

    assert!(plane.lines().is_empty());
    let lines = plane.approvals_lines();
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0]["state"], AWAITING_APPROVAL);
    assert_eq!(lines[0]["package"], plan_package(10, 4).url);
    assert_eq!(lines[0]["judged"][0]["verdict"], "approve");
    assert!(plane.fake.writes().is_empty());
}

#[tokio::test(start_paused = true)]
async fn a_deferred_entry_with_an_approvals_cause_is_judged_once() {
    let fake = tracker();
    seed(&fake, 5, true, BLOCKED, &[]);
    // State-sync's four reads see the value move twice; the approvals step's read then finds
    // it at rest at `queued`.
    let moves = [DONE, BLOCKED, DONE, QUEUED, QUEUED]
        .into_iter()
        .map(|name| Some(name.to_owned()))
        .collect();
    fake.script_field_changes(&IssueNumber(5), field_source(), moves);
    let mut plane = plane(fake);
    plane.sync.on_delivery(&link_event("removed", 5, 3));
    plane.sync.on_delivery(&comment_event(5));
    assert_eq!(plane.sync.queued(), 1);
    plane.worker.drain().await;

    let outcomes: Vec<Value> = plane
        .lines()
        .into_iter()
        .map(|line| line["outcome"].clone())
        .collect();
    assert_eq!(outcomes, vec![json!("deferred"), json!("unchanged")]);
    let approvals = plane.approvals_lines();
    assert_eq!(approvals.len(), 1);
    assert_eq!(approvals[0]["issue"], 5);
    assert_eq!(approvals[0]["state"], QUEUED);
    assert!(plane.fake.writes().is_empty());

    // The settle repeat is state-sync's alone.
    tokio::time::advance(SETTLE).await;
    plane.worker.drain().await;
    assert_eq!(plane.lines().len(), 3);
    assert_eq!(plane.approvals_lines().len(), 1);
}

#[tokio::test]
async fn the_owner_and_role_are_read_only_for_an_approve() {
    let fake = tracker();
    seed(&fake, 2, true, PLANNING, &[]);
    seed(&fake, 9, true, AWAITING_APPROVAL, &[]);
    // `#9` was filed by a bot, so its owner is its person-filed parent's author.
    fake.add_issue(Issue {
        number: IssueNumber(9),
        title: "Issue 9".to_owned(),
        body: String::new(),
        state: "open".to_owned(),
        labels: vec!["mintswarm".to_owned()],
        issue_type: Some(IssueType::Implementation),
        author: Login("helper[bot]".to_owned()),
        author_is_bot: true,
        url: format!("https://github.com/{REPO}/issues/9"),
        sub_issues: 0,
        blocked_by: 0,
    });
    fake.set_links(
        &IssueNumber(9),
        vec![Link {
            kind: LinkKind::Parent,
            number: 2,
        }],
    );
    fake.set_role(Login(MAINTAINER.to_owned()), RepoRole::Write);
    fake.add_comment(&IssueNumber(9), plan_package(10, 9));
    fake.add_comment(&IssueNumber(9), comment_on(11, 9, MAINTAINER, 5, "help"));
    let mut plane = plane(fake);
    plane.sync.on_delivery(&comment_event(9));
    plane.worker.drain().await;
    assert_eq!(plane.approvals_lines()[0]["judged"][0]["verdict"], "help");
    assert_eq!(plane.fake.permission_reads(), 0);
    assert_eq!(plane.fake.parent_reads(), 0);

    plane
        .fake
        .add_comment(&IssueNumber(9), comment_on(12, 9, MAINTAINER, 6, "approve"));
    plane.sync.on_delivery(&comment_event(9));
    plane.worker.drain().await;
    let lines = plane.approvals_lines();
    assert_eq!(lines[1]["judged"][1]["verdict"], "approve");
    assert_eq!(plane.fake.permission_reads(), 1);
    assert_eq!(plane.fake.parent_reads(), 1);
}

#[test]
fn self_logins_are_the_bot_then_its_peers() {
    assert_eq!(
        settings().self_logins(),
        [Login(BOT.to_owned()), Login(PEER.to_owned())]
    );
}
