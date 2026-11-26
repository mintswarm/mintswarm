//! Approval of a pull request by code owners: the approval groups of its changed paths, and
//! whether its approvals complete it.
//!
//! There is one group per CODEOWNERS rule that is the last match of a changed path, and one
//! unowned group for the paths that no rule owns, which anyone with write access may approve.
//! The caller supplies every fact: the changed paths, the approvers, the team members, a
//! write-access check and the branch protection. The item's owner is not among them: owning a
//! work item gives no right to approve it.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::codeowners::{CodeOwners, Owner, Team};
use crate::tracker::Login;

/// What an approval group is keyed by.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GroupKey {
    /// The CODEOWNERS rule that is the last match of the group's paths.
    Rule {
        /// The rule's line, counted from 1.
        line: usize,
        /// The rule's pattern, as the file writes it.
        pattern: String,
    },
    /// The paths no rule owns: no rule matches them, or their last match has no owners.
    Unowned,
}

/// Who may approve a group.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Eligibility {
    /// One of these owners, or a member of one of these teams.
    Owners(Vec<Owner>),
    /// Anyone with write access to the repository.
    AnyoneWithWriteAccess,
}

/// The changed paths that one approval settles.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalGroup {
    /// The rule the group stands for, or the unowned group.
    pub key: GroupKey,
    /// Who may approve it.
    pub eligible: Eligibility,
    /// Its paths, in the order they were given.
    pub paths: Vec<String>,
}

/// The approval groups of a pull request's changed PATHS: one per matching rule in rule
/// order, then one unowned group when any path has no owner. A path given twice counts once.
pub fn groups<I>(owners: &CodeOwners, paths: I) -> Vec<ApprovalGroup>
where
    I: IntoIterator,
    I::Item: AsRef<str>,
{
    let mut by_line: BTreeMap<usize, ApprovalGroup> = BTreeMap::new();
    let mut unowned = Vec::new();
    let mut seen = BTreeSet::new();
    for path in paths {
        let path = path.as_ref();
        if !seen.insert(path.to_owned()) {
            continue;
        }
        match owners.matching_rule(path) {
            Some(rule) if !rule.owners.is_empty() => {
                by_line
                    .entry(rule.line)
                    .or_insert_with(|| ApprovalGroup {
                        key: GroupKey::Rule {
                            line: rule.line,
                            pattern: rule.pattern.clone(),
                        },
                        eligible: Eligibility::Owners(rule.owners.clone()),
                        paths: Vec::new(),
                    })
                    .paths
                    .push(path.to_owned());
            }
            _ => unowned.push(path.to_owned()),
        }
    }
    let mut out: Vec<ApprovalGroup> = by_line.into_values().collect();
    if !unowned.is_empty() {
        out.push(ApprovalGroup {
            key: GroupKey::Unowned,
            eligible: Eligibility::AnyoneWithWriteAccess,
            paths: unowned,
        });
    }
    out
}

/// The facts about people that the caller looks up.
#[derive(Clone, Copy)]
pub struct Facts<'a> {
    /// The members of each team the groups name. A team missing here has no members.
    pub teams: &'a BTreeMap<Team, Vec<Login>>,
    /// Whether a login has write access to the repository.
    pub has_write: &'a dyn Fn(&Login) -> bool,
}

/// The branch protection that applies to the pull request's base.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Protection {
    /// Whether every group needs an approval from its code owners.
    pub require_code_owner_review: bool,
    /// How many distinct approvers are needed.
    pub required_approving_reviews: u32,
}

/// How far the approvals of a pull request go.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalStatus {
    /// The indices of the groups an approver satisfies, in group order.
    pub satisfied: Vec<usize>,
    /// The indices of the groups still waiting for an approver, in group order.
    pub open: Vec<usize>,
    /// The distinct approvers that count: those with write access, compared without regard to
    /// case.
    pub distinct_approvers: u32,
    /// Whether the approvals complete the pull request.
    pub complete: bool,
}

/// Evaluates the APPROVERS of a pull request against its GROUPS.
///
/// Only approvers with write access count, as GitHub ignores the reviews of others, and
/// logins are compared without regard to ASCII case, each counted once. A group is satisfied
/// by a counted approver who is one of its user owners or a member of one of its teams; the
/// unowned group by any counted approver. A group owned only by e-mail owners, or by teams
/// missing from the facts, cannot be satisfied and stays open.
///
/// The approvals are complete when the counted approvers reach the required number and, when
/// code-owner review is required, every group is satisfied. Without required code-owner
/// review only the count decides, and the open groups still name whom to ask.
pub fn evaluate(
    groups: &[ApprovalGroup],
    approvers: &[Login],
    facts: &Facts<'_>,
    protection: &Protection,
) -> ApprovalStatus {
    let mut counted: Vec<&Login> = Vec::new();
    for approver in approvers {
        if counted.iter().any(|c| same(c, approver)) || !(facts.has_write)(approver) {
            continue;
        }
        counted.push(approver);
    }
    let (satisfied, open): (Vec<usize>, Vec<usize>) = (0..groups.len()).partition(|&i| {
        counted
            .iter()
            .any(|a| may_approve(&groups[i].eligible, a, facts))
    });
    let distinct_approvers = u32::try_from(counted.len()).unwrap_or(u32::MAX);
    let complete = distinct_approvers >= protection.required_approving_reviews
        && (!protection.require_code_owner_review || open.is_empty());
    ApprovalStatus {
        satisfied,
        open,
        distinct_approvers,
        complete,
    }
}

fn same(a: &Login, b: &Login) -> bool {
    a.0.eq_ignore_ascii_case(&b.0)
}

fn may_approve(eligible: &Eligibility, approver: &Login, facts: &Facts<'_>) -> bool {
    match eligible {
        Eligibility::AnyoneWithWriteAccess => true,
        Eligibility::Owners(owners) => owners.iter().any(|owner| match owner {
            Owner::User(login) => same(login, approver),
            Owner::Team(team) => facts
                .teams
                .get(team)
                .is_some_and(|members| members.iter().any(|m| same(m, approver))),
            Owner::Email(_) => false,
        }),
    }
}
