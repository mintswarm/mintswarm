//! One evaluation of one issue from fresh reads, design §7 steps 1–6, with its blocker note.
//!
//! 1. Read the issue; an untrusted one is skipped.
//! 2. For an open issue, read its blockers; unlisted ones count as open (B17).
//! 3. Read the `msw-state` value; an unset field is `none`, and an unconfigured value is logged
//!    and not written.
//! 4. Apply the [`rule`](super::rule).
//! 5. Before writing a new value, read it again: when it moved, start again once, and when it
//!    moved again, defer. The value is written only when it differs.
//! 6. Under `status: issue_status`, write the status of the value written, or repair a wrong
//!    status on `queued`, `blocked` and `done`. Under `status: off` no status is written.
//!
//! When the rule asks for a report, the blocker note is posted through
//! [`report::post_if_changed`]. Every evaluation gives one [`Outcome`] and one JSON line.

use mintswarm_config::{BLOCKED, DONE, NONE, QUEUED, StateSettings, StatusMode, Trust};
use mintswarm_domain::field::{FieldError, FieldFinding, FieldRead, FieldRef};
use mintswarm_domain::ids::IssueNumber;
use mintswarm_domain::tracker::{
    Comments, Dependency, Direction, Fields, Issues, Links, Login, Statuses,
};
use serde_json::{Value, json};

use super::report::{self, Report};
use super::rule::{BlockerChangeKind, Cause, Facts, decide, open_blockers};

/// What the evaluator needs of the repository's config.
#[derive(Clone, Debug)]
pub struct Settings {
    /// The repository, `owner/name`.
    pub repository: String,
    /// The plane's login, whose notes are its own.
    pub bot: Login,
    /// `tracker.ingest`: which issues the plane evaluates.
    pub trust: Trust,
    /// `tracker.state`.
    pub state: StateSettings,
}

/// How an evaluation ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// It completed with a value or status write, or a post.
    Written,
    /// It completed with nothing to write or post.
    Unchanged,
    /// The issue is not evaluated: it is untrusted, or holds an unconfigured value. Nothing was
    /// written.
    Skipped,
    /// The value moved between its reads twice, so nothing was written.
    Deferred,
    /// A read or write failed; nothing was written after the failure.
    Failed,
}

impl Outcome {
    fn as_str(self) -> &'static str {
        match self {
            Self::Written => "written",
            Self::Unchanged => "unchanged",
            Self::Skipped => "skipped",
            Self::Deferred => "deferred",
            Self::Failed => "failed",
        }
    }
}

/// One evaluation's outcome and its JSON line.
#[derive(Clone, Debug, PartialEq)]
pub struct Evaluation {
    pub outcome: Outcome,
    /// `issue`, `causes`, `fan_out`, `value_before`, `value_after`, `wrote`, `status_before`,
    /// `status_after`, `reported` and `outcome`, with a `detail` on a skip, deferral or failure.
    pub line: Value,
}

/// What the evaluation has done so far, for its line.
#[derive(Debug, Default)]
struct Line {
    value_before: Option<String>,
    value_after: Option<String>,
    wrote: Vec<&'static str>,
    status_before: Option<String>,
    status_after: Option<String>,
    reported: bool,
}

impl Line {
    fn finish(
        self,
        issue: &IssueNumber,
        cause: &Cause,
        outcome: Outcome,
        detail: Option<String>,
    ) -> Evaluation {
        let causes: Vec<Value> = match cause {
            Cause::Sweep => Vec::new(),
            Cause::Triggered { blocker_changes } => blocker_changes
                .iter()
                .map(|change| {
                    json!({ "blocker": change.blocker.0, "change": change_name(change.change) })
                })
                .collect(),
        };
        let line = json!({
            "issue": issue.0,
            "cause": match cause { Cause::Sweep => "sweep", Cause::Triggered { .. } => "triggered" },
            "causes": causes,
            "fan_out": Value::Null,
            "value_before": self.value_before,
            "value_after": self.value_after,
            "wrote": self.wrote,
            "status_before": self.status_before,
            "status_after": self.status_after,
            "reported": self.reported,
            "outcome": outcome.as_str(),
            "detail": detail,
        });
        Evaluation { outcome, line }
    }
}

fn change_name(change: BlockerChangeKind) -> &'static str {
    match change {
        BlockerChangeKind::Closed => "closed",
        BlockerChangeKind::Reopened => "reopened",
        BlockerChangeKind::Added => "added",
        BlockerChangeKind::Removed => "removed",
    }
}

/// Why an evaluation stopped early.
enum Stop {
    Skipped(String),
    Failed(String),
}

/// The value a field read gives: unset is `none`, an unconfigured value a skip, and a labels
/// conflict or any other error a failure.
fn value_of(read: Result<FieldRead, FieldError>, state: &StateSettings) -> Result<String, Stop> {
    let name = match read {
        Ok(FieldRead::Unset) => NONE.to_owned(),
        Ok(FieldRead::Value(value)) => value.name,
        Ok(FieldRead::Conflict(values)) => {
            let names: Vec<String> = values.into_iter().map(|value| value.name).collect();
            return Err(Stop::Failed(format!(
                "labels conflict: {}",
                names.join(", ")
            )));
        }
        Err(FieldError::Field(FieldFinding::OptionUnmapped)) => {
            return Err(Stop::Skipped("the value is not configured".to_owned()));
        }
        Err(error) => return Err(Stop::Failed(format!("field read: {error}"))),
    };
    if state.option(&name).is_none() {
        return Err(Stop::Skipped(format!(
            "the value {name:?} is not configured"
        )));
    }
    Ok(name)
}

/// Evaluates `issue` for `cause` from fresh reads, writing only what differs.
pub async fn evaluate<T>(
    tracker: &T,
    settings: &Settings,
    issue: IssueNumber,
    cause: &Cause,
) -> Evaluation
where
    T: Issues + Links + Fields + Statuses + Comments,
{
    let mut line = Line::default();
    let (outcome, detail) = match run(tracker, settings, &issue, cause, &mut line).await {
        Ok(Some(detail)) => (Outcome::Deferred, Some(detail)),
        Ok(None) if line.wrote.is_empty() && !line.reported => (Outcome::Unchanged, None),
        Ok(None) => (Outcome::Written, None),
        Err(Stop::Skipped(detail)) => (Outcome::Skipped, Some(detail)),
        Err(Stop::Failed(detail)) => (Outcome::Failed, Some(detail)),
    };
    line.finish(&issue, cause, outcome, detail)
}

/// The evaluation's steps; `Ok(Some(_))` is a deferral, with its reason.
async fn run<T>(
    tracker: &T,
    settings: &Settings,
    issue: &IssueNumber,
    cause: &Cause,
    line: &mut Line,
) -> Result<Option<String>, Stop>
where
    T: Issues + Links + Fields + Statuses + Comments,
{
    let field: FieldRef = settings.state.field_ref();
    let mut restarted = false;
    loop {
        // Step 1.
        let read = tracker
            .read_issue(issue.clone(), None)
            .await
            .map_err(|error| Stop::Failed(format!("issue read: {error}")))?;
        if !settings.trust.trusts(&read.issue) {
            return Err(Stop::Skipped("the issue is not trusted".to_owned()));
        }
        let closed = !read.issue.state.eq_ignore_ascii_case("open");

        // Step 2.
        let listed: Vec<Dependency> = if closed {
            Vec::new()
        } else {
            tracker
                .read_dependencies(issue.clone(), Direction::BlockedBy)
                .await
                .map_err(|error| Stop::Failed(format!("blocker read: {error}")))?
        };
        let open = if closed {
            0
        } else {
            open_blockers(&listed, read.issue.blocked_by)
        };
        let listed_count = u64::try_from(listed.len()).unwrap_or(u64::MAX);
        let unreadable = read.issue.blocked_by.saturating_sub(listed_count);

        // Step 3.
        let current = value_of(
            tracker.read_field(issue.clone(), &field).await,
            &settings.state,
        )?;
        line.value_before = Some(current.clone());
        line.value_after = Some(current.clone());

        // Step 4.
        let facts = Facts {
            closed,
            open_blockers: open,
            has_sub_issues: read.issue.sub_issues > 0,
            current: &current,
        };
        let decision = decide(facts, cause);

        // Step 5.
        let mut written = None;
        if let Some(next) = decision.next {
            let again = value_of(
                tracker.read_field(issue.clone(), &field).await,
                &settings.state,
            )?;
            if again != current {
                if restarted {
                    line.value_after = Some(again.clone());
                    return Ok(Some(format!(
                        "the value moved from {current:?} to {again:?} again"
                    )));
                }
                restarted = true;
                continue;
            }
            tracker
                .set_field(issue.clone(), &field, Some(next))
                .await
                .map_err(|error| Stop::Failed(format!("field write: {error}")))?;
            line.wrote.push("value");
            line.value_after = Some(next.to_owned());
            written = Some(next);
        }

        // Step 6.
        if settings.state.status == StatusMode::IssueStatus {
            let target = written.or_else(|| {
                [QUEUED, BLOCKED, DONE]
                    .into_iter()
                    .find(|name| *name == current)
            });
            let status = target
                .and_then(|name| settings.state.option(name))
                .and_then(|option| option.status.clone());
            if let Some(status) = status {
                let now = tracker
                    .read_status(issue.clone())
                    .await
                    .map_err(|error| Stop::Failed(format!("status read: {error}")))?;
                line.status_before.clone_from(&now);
                line.status_after.clone_from(&now);
                if now.as_deref() != Some(status.as_str()) {
                    tracker
                        .set_status(issue.clone(), status.clone())
                        .await
                        .map_err(|error| Stop::Failed(format!("status write: {error}")))?;
                    line.wrote.push("status");
                    line.status_after = Some(status);
                }
            }
        }

        // The blocker note.
        if decision.report {
            let changes = match cause {
                Cause::Triggered { blocker_changes } => blocker_changes.as_slice(),
                Cause::Sweep => &[],
            };
            let open_listed: Vec<Dependency> =
                listed.into_iter().filter(|blocker| blocker.open).collect();
            let note = Report {
                repository: &settings.repository,
                changes,
                open: &open_listed,
                unreadable,
                value: &current,
            };
            line.reported = report::post_if_changed(
                tracker,
                issue.clone(),
                &settings.bot,
                &read.comments,
                &note,
            )
            .await
            .map_err(|error| Stop::Failed(format!("note post: {error}")))?;
        }
        return Ok(None);
    }
}
