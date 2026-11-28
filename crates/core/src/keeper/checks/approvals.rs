//! When a pull request approval stands, how far the standing approvals cover the approval
//! groups, and the hint for an Approve command comment.
//!
//! An approver's standing is decided by their own latest submitted review in state
//! `APPROVED`, `CHANGES_REQUESTED` or `DISMISSED`, by `submitted_at`, the later one in host
//! order on a tie; `COMMENTED` and `PENDING` reviews take no part. Logins are compared without
//! regard to ASCII case. A standing `APPROVED` review by a person counts when:
//!
//! - its commit is the head: the review is trusted as the host anchors it, with no shape check
//!   here, since a clean update of the branch keeps the approval and moves its anchor to the
//!   head;
//! - or its commit is one the caller lists as keeping the approved shape: the interdiff of the
//!   pull request's diff against its merge base at that commit and at the head is empty.
//!
//! What withdraws an approval: the approver's own later Request changes, a dismissal, or an
//! anchor at an older commit with no same-shape fact. A force-pushed rebase may still arrive
//! `DISMISSED` while the host dismisses stale approvals on push, and then the dismissal
//! withdraws it.
//!
//! What does not: comments, inline comments, edits, steer comments and other people's
//! reviews, whether they approve, comment or request changes. Bots never approve.
//!
//! A person's comment that reads as the Approve command approves nothing; it yields a hint
//! pointing at the review instead.

use mintswarm_config::People;
use mintswarm_domain::tracker::{Review, is_bot};
use mintswarm_rules::approver::{self, Approver, Cover};
use mintswarm_rules::command::{self, Command, Reading};

use super::{ApproveHint, Facts};

/// The review states that decide an approver's standing.
const DECIDING: [&str; 3] = ["APPROVED", "CHANGES_REQUESTED", "DISMISSED"];

/// The approvals that count and their cover.
pub(super) struct Collected {
    /// Whether the `approve_pr` row trusts the CODEOWNERS with approvals.
    pub trusted: bool,
    /// The counted reviews, in host order.
    pub approvals: Vec<Review>,
    /// Their cover of the approval groups.
    pub cover: Cover,
    /// Whether a standing approval was dropped for want of a same-shape fact.
    pub shape_dropped: bool,
}

/// The standing approvals of `facts`, and their cover.
pub(super) fn collect(facts: &Facts<'_>) -> Collected {
    let trusted = facts.approve_pr.people == People::Codeowners;
    let mut counted = Vec::new();
    let mut shape_dropped = false;
    if trusted {
        for index in deciding(facts.reviews) {
            let review = &facts.reviews[index];
            if review.state != "APPROVED" || review.author_is_bot || is_bot(&review.author.0, None)
            {
                continue;
            }
            let anchored = review.commit.as_ref().is_some_and(|commit| {
                commit == &facts.pull.head || facts.same_shape.contains(commit)
            });
            if anchored {
                counted.push(index);
            } else {
                shape_dropped = true;
            }
        }
    }
    counted.sort_unstable();
    let approvals: Vec<Review> = counted
        .into_iter()
        .map(|index| facts.reviews[index].clone())
        .collect();
    let approvers: Vec<Approver> = approvals
        .iter()
        .map(|review| Approver {
            login: review.author.clone(),
            is_bot: review.author_is_bot,
        })
        .collect();
    let cover = approver::cover(facts.groups, &approvers, facts.roles);
    Collected {
        trusted,
        approvals,
        cover,
        shape_dropped,
    }
}

/// The index of each reviewer's deciding review: their latest submitted one in a deciding
/// state, by `submitted_at`, the later in host order on a tie.
fn deciding(reviews: &[Review]) -> Vec<usize> {
    let mut latest: Vec<usize> = Vec::new();
    for (index, review) in reviews.iter().enumerate() {
        let Some(at) = review.submitted_at.as_deref() else {
            continue;
        };
        if !DECIDING.contains(&review.state.as_str()) {
            continue;
        }
        let same = latest.iter_mut().find(|kept| {
            reviews[**kept]
                .author
                .0
                .eq_ignore_ascii_case(&review.author.0)
        });
        match same {
            Some(kept) => {
                if reviews[*kept]
                    .submitted_at
                    .as_deref()
                    .is_none_or(|k| at >= k)
                {
                    *kept = index;
                }
            }
            None => latest.push(index),
        }
    }
    latest
}

/// The hints for the people's Approve command comments of `facts`, in host order.
pub(super) fn hints(facts: &Facts<'_>) -> Vec<ApproveHint> {
    facts
        .comments
        .iter()
        .filter(|comment| !is_bot(&comment.author.0, None))
        .filter(|comment| {
            matches!(
                command::read(&comment.body),
                Some(Reading::Command {
                    command: Command::Approve,
                    ..
                })
            )
        })
        .map(|comment| ApproveHint {
            comment: comment.id,
            review_url: format!("{}/files", facts.pull.url),
        })
        .collect()
}
