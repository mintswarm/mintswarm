//! One approvals evaluation of one issue from fresh reads: read, judge and log, writing nothing.
//!
//! 1. Read the issue with all its comments.
//! 2. Read the `msw-state` value; an unset field is `none`. A read error or a value state-sync
//!    would not evaluate stops the evaluation as failed, with no judgement.
//! 3. Only when [`judge::needs_approver`] says a person's `approve` or `reject` is still to
//!    answer, read the issue's owner up its parent chain, then the owner's role.
//! 4. [`judge::judge`] every command comment.
//!
//! Each evaluation gives one JSON line. The worker runs it after state-sync for the same
//! number, so the value it reads is the one state-sync left.

use mintswarm_domain::ids::IssueNumber;
use mintswarm_domain::tracker::{Collaborators, Fields, Issues};
use serde_json::{Value, json};

use super::approver::{ApproverError, issue_owner};
use super::judge::{self, Approver, Facts, Judged, Verdict, latest_package, needs_approver};
use super::trigger::ApprovalCause;
use crate::state_sync::Settings;
use crate::state_sync::evaluate::value_of;

/// Evaluates `issue`'s approvals for `cause` and gives its JSON line: `evaluator`
/// (`approvals`), `issue`, `cause`, `state`, `context`, `package` and `sha256` of the latest
/// package, `judged` (each `comment`, `author`, `verdict`, and `check` and `reason` on a
/// refusal), and `outcome` `judged` or `failed`, with a `detail` on a failure.
pub async fn evaluate<T>(
    tracker: &T,
    settings: &Settings,
    issue: IssueNumber,
    cause: ApprovalCause,
) -> Value
where
    T: Issues + Fields + Collaborators,
{
    let mut line = json!({
        "evaluator": "approvals",
        "issue": issue.0,
        "cause": cause.as_str(),
        "state": Value::Null,
        "context": Value::Null,
        "package": Value::Null,
        "sha256": Value::Null,
        "judged": [],
        "outcome": "judged",
        "detail": Value::Null,
    });
    if let Err(detail) = run(tracker, settings, issue, &mut line).await {
        line["outcome"] = "failed".into();
        line["detail"] = detail.into();
    }
    line
}

/// The evaluation's steps, filling `line`; an error is the failure's detail.
async fn run<T>(
    tracker: &T,
    settings: &Settings,
    issue: IssueNumber,
    line: &mut Value,
) -> Result<(), String>
where
    T: Issues + Fields + Collaborators,
{
    let read = tracker
        .read_issue(issue.clone(), None)
        .await
        .map_err(|error| format!("issue read: {error}"))?;
    let state = value_of(
        tracker.read_field(issue, &settings.state.field_ref()).await,
        &settings.state,
    )
    .map_err(|stop| stop.detail())?;
    line["state"] = state.as_str().into();

    let self_logins = settings.self_logins();
    if let Some(package) = latest_package(&read.comments, &self_logins) {
        line["context"] = package.kind.noun().into();
        line["package"] = package.comment.url.as_str().into();
        line["sha256"] = package.sha256.into();
    }

    let owner = if needs_approver(&read.comments, &self_logins) {
        match issue_owner(tracker, &read.issue).await {
            Ok(owner) => {
                let role = tracker
                    .read_permission(owner.clone())
                    .await
                    .map_err(|error| format!("role read: {error}"))?;
                Some(Some((owner, role)))
            }
            Err(ApproverError::Owner(_)) => Some(None),
            Err(ApproverError::Tracker(error)) => return Err(format!("owner read: {error}")),
        }
    } else {
        None
    };
    let approver = match &owner {
        None => Approver::NotRead,
        Some(None) => Approver::NoOwner,
        Some(Some((login, role))) => Approver::Owner { login, role: *role },
    };

    let judged = judge::judge(&Facts {
        issue: &read.issue,
        comments: &read.comments,
        trust: &settings.trust,
        approver,
        state: &state,
        actions: &settings.actions,
        self_logins: &self_logins,
    })
    .map_err(|error| error.to_string())?;
    line["judged"] = judged.iter().map(judged_line).collect();
    Ok(())
}

/// One judged comment in the line.
fn judged_line(judged: &Judged) -> Value {
    let mut out = json!({
        "comment": judged.comment,
        "author": judged.author.0,
        "verdict": match &judged.verdict {
            Verdict::Approve { .. } => "approve",
            Verdict::Reject { .. } => "reject",
            Verdict::Refused(_) => "refused",
            Verdict::Help { .. } => "help",
            Verdict::Hint { .. } => "hint",
        },
    });
    if let Verdict::Refused(refusal) = &judged.verdict {
        out["check"] = refusal.check().into();
        out["reason"] = refusal.reason().into();
    }
    out
}
