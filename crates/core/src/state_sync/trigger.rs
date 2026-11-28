//! From a normalised delivery to the issues it concerns, design §6.
//!
//! The reduction is pure: it reads only the event, never the tracker. A close or reopen of an
//! issue gives that issue, marked for fan-out, so the worker later reads the issues it blocks.
//! A dependency change gives the blocked issue, with the blocker, its repository and the change
//! as its cause, so a blocker in another repository is named as one too. Only blocked issues in
//! the plane's own repository are kept.

use mintswarm_domain::ids::IssueNumber;
use mintswarm_proto::watch::v1;

use super::queue::Entry;
use super::rule::{BlockerChange, BlockerChangeKind};
use crate::watch::WEBHOOK_KIND;

/// The queue entries `event` gives in `repository` (`owner/name`, compared ASCII
/// case-insensitively), each with its issue; empty for any event that concerns no issue there.
///
/// - An `issues` event with `closed` or `reopened` on an issue gives that issue, marked for
///   fan-out, with a triggered cause that carries no blocker change.
/// - An `issue_dependencies` event with `blocked_by_added`, `blocked_by_removed`,
///   `blocking_added` or `blocking_removed` gives the blocked issue, with the blocking issue
///   added or removed as its cause, named with its own repository, which is `repository` when
///   the payload gives none.
pub fn reduce(event: &v1::WatchEvent, repository: &str) -> Vec<(IssueNumber, Entry)> {
    if event.kind != WEBHOOK_KIND {
        return Vec::new();
    }
    let Some(v1::watch_event::Body::GithubWebhook(webhook)) = &event.body else {
        return Vec::new();
    };
    match webhook.event.as_str() {
        "issues" => issue_event(event, &webhook.action, repository),
        "issue_dependencies" => dependency_event(webhook, repository),
        _ => None,
    }
    .into_iter()
    .collect()
}

/// A close or reopen of an issue in `repository`.
fn issue_event(
    event: &v1::WatchEvent,
    action: &str,
    repository: &str,
) -> Option<(IssueNumber, Entry)> {
    if action != "closed" && action != "reopened" {
        return None;
    }
    let subject = event.subject.as_ref()?;
    if subject.item_kind() != v1::ItemKind::Issue
        || subject.number == 0
        || !subject.repository.eq_ignore_ascii_case(repository)
    {
        return None;
    }
    let mut entry = Entry::triggered(Vec::new());
    entry.fan_out = true;
    Some((IssueNumber(subject.number), entry))
}

/// A dependency added or removed on a blocked issue in `repository`.
fn dependency_event(webhook: &v1::GitHubWebhook, repository: &str) -> Option<(IssueNumber, Entry)> {
    let change = match webhook.action.as_str() {
        "blocked_by_added" | "blocking_added" => BlockerChangeKind::Added,
        "blocked_by_removed" | "blocking_removed" => BlockerChangeKind::Removed,
        _ => return None,
    };
    let dependency = webhook.dependency.as_ref()?;
    let blocked = dependency.blocked.as_ref()?;
    let blocking = dependency.blocking.as_ref()?;
    if blocked.number == 0
        || blocking.number == 0
        || !blocked.repository.eq_ignore_ascii_case(repository)
    {
        return None;
    }
    let blocker_repository = if blocking.repository.is_empty() {
        repository
    } else {
        &blocking.repository
    };
    let entry = Entry::triggered(vec![BlockerChange {
        blocker: IssueNumber(blocking.number),
        repository: blocker_repository.to_owned(),
        change,
    }]);
    Some((IssueNumber(blocked.number), entry))
}
