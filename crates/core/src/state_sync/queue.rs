//! The per-repository queue and its one worker, design §6.
//!
//! [`StateSync`] is the hand-off: it holds no tracker, so [`StateSync::on_delivery`] cannot call
//! one. It reduces the event, merges the entries into a set of issue numbers kept in arrival
//! order, and wakes the worker. A number already queued absorbs the new cause and keeps its
//! place.
//!
//! [`Worker`] owns the tracker and evaluates one issue at a time, oldest first. Before it
//! evaluates an issue marked for fan-out, it queues the issues that one blocks. Each triggered
//! evaluation is queued again once after [`SETTLE`], without its cause, from one FIFO of
//! deadlines: the delay is constant, so the deadlines are already in order.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use mintswarm_domain::ids::IssueNumber;
use mintswarm_domain::tracker::{Comments, Direction, Fields, Issues, Links, Statuses};
use mintswarm_proto::watch::v1::WatchEvent;
use tokio::sync::Notify;
use tokio::time::Instant;

use super::evaluate::{Outcome, Settings, evaluate};
use super::rule::{BlockerChange, BlockerChangeKind, Cause};
use super::trigger;

/// How long after a triggered evaluation the issue is evaluated once more.
pub const SETTLE: Duration = Duration::from_secs(60);

/// Where each evaluation's JSON line goes; by default a `tracing` event of target `state_sync`.
pub type Sink = Arc<dyn Fn(&str) + Send + Sync>;

/// One queued issue's reasons to be evaluated.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// Why the issue is evaluated.
    pub cause: Cause,
    /// Set when the issue itself was closed or reopened: the worker first queues the issues it
    /// blocks, each with the issue's close or reopen as its blocker change, as the issue's
    /// state read at fan-out time gives it.
    pub fan_out: bool,
    /// Whether this is the settle repeat of a triggered evaluation, which is never repeated.
    pub repeat: bool,
    /// Whether the entry was already queued again after a value that kept moving.
    pub requeued: bool,
}

impl Entry {
    /// A triggered entry carrying `blocker_changes`, not a repeat.
    pub fn triggered(blocker_changes: Vec<BlockerChange>) -> Self {
        Self::of(Cause::Triggered { blocker_changes })
    }

    /// An entry with `cause`, unmarked.
    pub fn of(cause: Cause) -> Self {
        Self {
            cause,
            fan_out: false,
            repeat: false,
            requeued: false,
        }
    }

    /// Absorbs `other` into this entry.
    ///
    /// Blocker changes are a set union by blocker, repository and change, kept in arrival
    /// order. A triggered cause absorbs a
    /// sweep. A fan-out mark survives. The merged entry is a repeat when either side is one and
    /// neither is a triggered non-repeat, so a repeat absorbed by a sweep stays a repeat, and
    /// `requeued` is set when either is.
    pub fn merge(&mut self, other: Entry) {
        let fresh = |entry: &Entry| matches!(entry.cause, Cause::Triggered { .. }) && !entry.repeat;
        let repeat = (self.repeat || other.repeat) && !fresh(self) && !fresh(&other);
        self.cause = match (
            std::mem::replace(&mut self.cause, Cause::Sweep),
            other.cause,
        ) {
            (Cause::Sweep, Cause::Sweep) => Cause::Sweep,
            (Cause::Sweep, triggered @ Cause::Triggered { .. })
            | (triggered @ Cause::Triggered { .. }, Cause::Sweep) => triggered,
            (
                Cause::Triggered {
                    blocker_changes: mut ours,
                },
                Cause::Triggered {
                    blocker_changes: theirs,
                },
            ) => {
                for change in theirs {
                    let known = ours.iter().any(|known| {
                        known.blocker == change.blocker
                            && known.change == change.change
                            && known.repository.eq_ignore_ascii_case(&change.repository)
                    });
                    if !known {
                        ours.push(change);
                    }
                }
                Cause::Triggered {
                    blocker_changes: ours,
                }
            }
        };
        self.fan_out = self.fan_out || other.fan_out;
        self.repeat = repeat;
        self.requeued = self.requeued || other.requeued;
    }
}

/// The queued issues, in arrival order.
#[derive(Debug, Default)]
struct Queue {
    entries: HashMap<IssueNumber, Entry>,
    order: VecDeque<IssueNumber>,
}

impl Queue {
    fn push(&mut self, issue: IssueNumber, entry: Entry) {
        match self.entries.get_mut(&issue) {
            Some(queued) => queued.merge(entry),
            None => {
                self.order.push_back(issue.clone());
                self.entries.insert(issue, entry);
            }
        }
    }

    fn pop(&mut self) -> Option<(IssueNumber, Entry)> {
        while let Some(issue) = self.order.pop_front() {
            if let Some(entry) = self.entries.remove(&issue) {
                return Some((issue, entry));
            }
        }
        None
    }
}

/// What the hand-off and the worker share.
#[derive(Debug, Default)]
struct Shared {
    queue: Mutex<Queue>,
    notify: Notify,
}

impl Shared {
    fn queue(&self) -> MutexGuard<'_, Queue> {
        // The queue is consistent after every statement, so a panic elsewhere leaves it usable.
        self.queue.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn push(&self, entries: impl IntoIterator<Item = (IssueNumber, Entry)>) {
        let mut pushed = false;
        {
            let mut queue = self.queue();
            for (issue, entry) in entries {
                queue.push(issue, entry);
                pushed = true;
            }
        }
        if pushed {
            // Keeps a permit when the worker is not waiting, so no wake-up is lost.
            self.notify.notify_one();
        }
    }
}

/// The hand-off into one repository's queue.
#[derive(Clone, Debug)]
pub struct StateSync {
    repository: String,
    shared: Arc<Shared>,
}

impl StateSync {
    /// The hand-off for `settings.repository` and the worker that drains it with `tracker`.
    pub fn new<T>(tracker: Arc<T>, settings: Settings) -> (Self, Worker<T>) {
        let shared = Arc::new(Shared::default());
        let handle = Self {
            repository: settings.repository.clone(),
            shared: Arc::clone(&shared),
        };
        let worker = Worker {
            tracker,
            settings,
            shared,
            settle: VecDeque::new(),
            sink: Arc::new(|line: &str| tracing::info!(target: "state_sync", "{line}")),
        };
        (handle, worker)
    }

    /// Reduces `event` and queues its entries, without waiting and without any tracker call.
    pub fn on_delivery(&self, event: &WatchEvent) {
        self.shared.push(trigger::reduce(event, &self.repository));
    }

    /// Queues `issue` with `cause`, merged into its entry when it is already queued.
    pub fn enqueue(&self, issue: IssueNumber, cause: Cause) {
        self.shared.push([(issue, Entry::of(cause))]);
    }

    /// How many issues are queued.
    pub fn queued(&self) -> usize {
        self.shared.queue().entries.len()
    }
}

/// The one worker of a repository's queue.
pub struct Worker<T> {
    tracker: Arc<T>,
    settings: Settings,
    shared: Arc<Shared>,
    /// The settle repeats, by deadline, each with the fan-out mark it keeps.
    settle: VecDeque<(Instant, IssueNumber, bool)>,
    sink: Sink,
}

impl<T> Worker<T>
where
    T: Issues + Links + Fields + Statuses + Comments,
{
    /// The worker writing each evaluation's line to `sink` instead of standard error.
    #[must_use]
    pub fn with_sink(mut self, sink: Sink) -> Self {
        self.sink = sink;
        self
    }

    /// Evaluates until nothing is queued and no settle repeat is due at the current time,
    /// then returns.
    pub async fn drain(&mut self) {
        loop {
            self.release_due(Instant::now());
            let next = self.shared.queue().pop();
            let Some((issue, entry)) = next else {
                return;
            };
            self.process(issue, entry).await;
        }
    }

    /// Drains, then waits for a hand-off or the next settle deadline, for ever.
    pub async fn run(mut self) {
        let shared = Arc::clone(&self.shared);
        loop {
            self.drain().await;
            match self.settle.front() {
                Some((deadline, _, _)) => {
                    let deadline = *deadline;
                    tokio::select! {
                        () = shared.notify.notified() => {}
                        () = tokio::time::sleep_until(deadline) => {}
                    }
                }
                None => shared.notify.notified().await,
            }
        }
    }

    /// Queues every settle repeat due at `now` as a repeat without a cause.
    fn release_due(&mut self, now: Instant) {
        let mut due = Vec::new();
        while self
            .settle
            .front()
            .is_some_and(|(deadline, _, _)| *deadline <= now)
        {
            if let Some((_, issue, fan_out)) = self.settle.pop_front() {
                let mut entry = Entry::triggered(Vec::new());
                entry.fan_out = fan_out;
                entry.repeat = true;
                due.push((issue, entry));
            }
        }
        if !due.is_empty() {
            let mut queue = self.shared.queue();
            for (issue, entry) in due {
                queue.push(issue, entry);
            }
        }
    }

    /// Fans out, evaluates and logs one entry, then queues its requeue or settle repeat.
    async fn process(&mut self, issue: IssueNumber, entry: Entry) {
        let fan_out = if entry.fan_out {
            Some(self.fan_out(&issue).await)
        } else {
            None
        };
        let mut evaluation =
            evaluate(&*self.tracker, &self.settings, issue.clone(), &entry.cause).await;
        evaluation.line["fan_out"] = match &fan_out {
            None => serde_json::Value::Null,
            Some(Ok(())) => "done".into(),
            Some(Err(_)) => "failed".into(),
        };
        if let Some(Err(error)) = &fan_out {
            evaluation.line["fan_out_error"] = error.as_str().into();
        }
        evaluation.line["repeat"] = entry.repeat.into();
        (self.sink)(&evaluation.line.to_string());

        // A failed fan-out keeps its mark, so the repeat reads the blocked issues again.
        let keep_mark = matches!(fan_out, Some(Err(_)));
        if evaluation.outcome == Outcome::Deferred {
            if !entry.requeued {
                let mut again = Entry::of(entry.cause);
                again.fan_out = keep_mark;
                again.repeat = entry.repeat;
                again.requeued = true;
                self.shared.push([(issue, again)]);
            }
            return;
        }
        if matches!(entry.cause, Cause::Triggered { .. }) && !entry.repeat {
            self.settle
                .push_back((Instant::now() + SETTLE, issue, keep_mark));
        }
    }

    /// Queues every issue `issue` blocks in the same repository, with the close or reopen of
    /// `issue` as its cause: `Reopened` when the issue reads open now, `Closed` otherwise, so
    /// deliveries out of order give the change the facts show.
    async fn fan_out(&self, issue: &IssueNumber) -> Result<(), String> {
        let read = self
            .tracker
            .read_issue(issue.clone(), None)
            .await
            .map_err(|error| error.to_string())?;
        let change = if read.issue.state.eq_ignore_ascii_case("open") {
            BlockerChangeKind::Reopened
        } else {
            BlockerChangeKind::Closed
        };
        let blocked = self
            .tracker
            .read_dependencies(issue.clone(), Direction::Blocking)
            .await
            .map_err(|error| error.to_string())?;
        let entries: Vec<(IssueNumber, Entry)> = blocked
            .into_iter()
            .filter(|dependency| {
                dependency
                    .repository
                    .eq_ignore_ascii_case(&self.settings.repository)
            })
            .map(|dependency| {
                let cause = vec![BlockerChange {
                    blocker: issue.clone(),
                    repository: self.settings.repository.clone(),
                    change,
                }];
                (dependency.number, Entry::triggered(cause))
            })
            .collect();
        self.shared.push(entries);
        Ok(())
    }
}
