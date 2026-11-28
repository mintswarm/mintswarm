//! From a normalised delivery to the issues and pull requests approvals and the steward
//! evaluate, design #548 §5.
//!
//! The reduction is pure: it reads only the event, never the tracker. Its entries go on the
//! state-sync queue, the one queue per repository, beside the state-sync causes
//! [`state_sync::trigger`](crate::state_sync::trigger) gives. An issue gets an approvals cause,
//! a pull request the PR cause; both share the repository's number space.
//!
//! The plane's own comments and reviews, a `SELF` sender's, queue nothing, with one exception:
//! a newly posted approval package queues its issue with [`ApprovalCause::Opened`], so the wait
//! opens on that delivery. Other `SELF` deliveries (labels, pushes, check suites) queue as any
//! sender's do.

use mintswarm_domain::ids::IssueNumber;
use mintswarm_proto::watch::v1;
use mintswarm_rules::command;

use crate::state_sync::queue::Entry;
use crate::steward::checks::HOLD_LABEL;
use crate::watch::WEBHOOK_KIND;

/// Why an issue's approvals are evaluated; the later cause is the stronger one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ApprovalCause {
    /// A delivery that may hold or change a decision: a comment, a close or reopen, or a
    /// `plan approved` label.
    Triggered,
    /// The plane posted a new approval package on the issue.
    Opened,
}

impl ApprovalCause {
    /// The cause as its line names it: `triggered` or `opened`.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Triggered => "triggered",
            Self::Opened => "opened",
        }
    }
}

/// The queue entries `event` gives in `repository` (`owner/name`, compared ASCII
/// case-insensitively) for approvals and the steward; empty for any event that concerns
/// neither there.
///
/// `plan_approved_label` is the label of the `plan approved` value in labels mode, and `None`
/// in field mode, where no label is a value.
///
/// - `issue_comment` `created` or `edited` gives its issue with [`ApprovalCause::Triggered`],
///   or its pull request with the PR cause.
/// - `issues` `closed` or `reopened` gives the issue; `labeled` or `unlabeled` gives it only
///   for `plan_approved_label`, compared ASCII case-insensitively.
/// - `pull_request_review` `submitted` or `dismissed`, and every `pull_request_review_comment`,
///   give the pull request.
/// - `pull_request` `synchronize` and `ready_for_review`, and `labeled` or `unlabeled` with
///   [`HOLD_LABEL`], give the pull request.
/// - `check_suite` `completed` gives each pull request its `pull_requests` detail names.
///
/// A `SELF` comment, review or review comment gives nothing, except an `issue_comment`
/// `created` on an issue whose body is an approval package, current or not, which gives the
/// issue with [`ApprovalCause::Opened`]. Number 0 is never queued.
pub fn reduce(
    event: &v1::WatchEvent,
    repository: &str,
    plan_approved_label: Option<&str>,
) -> Vec<(IssueNumber, Entry)> {
    if event.kind != WEBHOOK_KIND {
        return Vec::new();
    }
    let Some(v1::watch_event::Body::GithubWebhook(webhook)) = &event.body else {
        return Vec::new();
    };
    let Some(subject) = event
        .subject
        .as_ref()
        .filter(|subject| subject.repository.eq_ignore_ascii_case(repository))
    else {
        return Vec::new();
    };
    let own = event
        .actor
        .as_ref()
        .is_some_and(|actor| actor.class() == v1::ActorClass::Self_);
    let action = webhook.action.as_str();
    let kind = subject.item_kind();
    let number = (subject.number != 0).then_some(IssueNumber(subject.number));
    let issue = |entry: Entry| {
        number
            .clone()
            .filter(|_| kind == v1::ItemKind::Issue)
            .map(|number| (number, entry))
    };
    let pull = || {
        number
            .clone()
            .filter(|_| kind == v1::ItemKind::PullRequest)
            .map(|number| (number, Entry::pull()))
    };
    let detail = |key: &str| webhook.detail.get(key).map(|text| text.text.as_str());
    let found = match (webhook.event.as_str(), action) {
        ("issue_comment", "created" | "edited") => match kind {
            v1::ItemKind::Issue if own => {
                let package = webhook
                    .body
                    .as_ref()
                    .and_then(|body| command::package(&body.text));
                if action == "created" && package.is_some() {
                    issue(Entry::approvals(ApprovalCause::Opened))
                } else {
                    None
                }
            }
            v1::ItemKind::Issue => issue(Entry::approvals(ApprovalCause::Triggered)),
            _ if own => None,
            _ => pull(),
        },
        ("issues", "closed" | "reopened") => issue(Entry::approvals(ApprovalCause::Triggered)),
        ("issues", "labeled" | "unlabeled") => {
            let named = detail("label").zip(plan_approved_label);
            if named.is_some_and(|(label, wanted)| label.eq_ignore_ascii_case(wanted)) {
                issue(Entry::approvals(ApprovalCause::Triggered))
            } else {
                None
            }
        }
        ("pull_request_review", "submitted" | "dismissed") | ("pull_request_review_comment", _)
            if !own =>
        {
            pull()
        }
        ("pull_request", "synchronize" | "ready_for_review") => pull(),
        ("pull_request", "labeled" | "unlabeled") => {
            if detail("label").is_some_and(|label| label.eq_ignore_ascii_case(HOLD_LABEL)) {
                pull()
            } else {
                None
            }
        }
        ("check_suite", "completed") => {
            return detail("pull_requests")
                .into_iter()
                .flat_map(|numbers| numbers.split(','))
                .filter_map(|number| number.parse::<u64>().ok())
                .filter(|number| *number != 0)
                .map(|number| (IssueNumber(number), Entry::pull()))
                .collect();
        }
        _ => None,
    };
    found.into_iter().collect()
}
