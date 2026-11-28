//! The `msw-state` rule, design §4: from an issue's facts and the cause of the evaluation to the
//! next value and whether to report.
//!
//! The rule is pure. It names only the four values it treats specially, `NONE`, `QUEUED`,
//! `BLOCKED` and `DONE` from `mintswarm_config`; every other configured value is active (B14),
//! including any value added later.

use mintswarm_config::{BLOCKED, DONE, NONE, QUEUED};
use mintswarm_domain::ids::IssueNumber;
use mintswarm_domain::tracker::Dependency;

/// What the rule reads of one issue, design §4.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Facts<'a> {
    /// Whether the issue is closed (B2).
    pub closed: bool,
    /// The issue's open blockers, unlisted ones counted as open (B17); see [`open_blockers`].
    pub open_blockers: u64,
    /// Whether the issue has sub-issues, which keep a `blocked` parent blocked (B10).
    pub has_sub_issues: bool,
    /// The issue's current `msw-state` value, a configured name.
    pub current: &'a str,
}

/// Why the rule runs, design §4 (B15).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Cause {
    /// The periodic sweep.
    Sweep,
    /// A tracker event; the blocker changes it carries are the ones of this issue, and may be
    /// none, as for a close or reopen of the issue itself.
    Triggered {
        /// The changes to this issue's blockers.
        blocker_changes: Vec<BlockerChange>,
    },
}

/// One change to a blocker of the issue, design §4 (B15).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlockerChange {
    /// The blocking issue's number, in `repository`.
    pub blocker: IssueNumber,
    /// The blocking issue's repository, `owner/name`, which may be another than the blocked
    /// issue's.
    pub repository: String,
    /// What happened to it.
    pub change: BlockerChangeKind,
}

/// What happened to a blocker, design §4 (B15).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlockerChangeKind {
    /// The blocker was closed.
    Closed,
    /// The blocker was reopened.
    Reopened,
    /// The blocker was added.
    Added,
    /// The blocker was removed.
    Removed,
}

/// The rule's outcome, design §4.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Decision {
    /// The value to set, `None` to leave the current one.
    pub next: Option<&'static str>,
    /// Whether to report the blocker change on an active issue (B15).
    pub report: bool,
}

/// The open blockers of an issue: the listed ones still open, plus every blocker of `total` the
/// listing did not return, which counts as open (B17).
pub fn open_blockers(listed: &[Dependency], total: u64) -> u64 {
    let listed_count = u64::try_from(listed.len()).unwrap_or(u64::MAX);
    let open = listed.iter().filter(|dependency| dependency.open).count();
    let open = u64::try_from(open).unwrap_or(u64::MAX);
    open.saturating_add(total.saturating_sub(listed_count))
}

/// How the rule treats a value (B14).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Class {
    None,
    Queued,
    Blocked,
    Done,
    Active,
}

fn class(name: &str) -> Class {
    if name == NONE {
        Class::None
    } else if name == QUEUED {
        Class::Queued
    } else if name == BLOCKED {
        Class::Blocked
    } else if name == DONE {
        Class::Done
    } else {
        Class::Active
    }
}

/// The next `msw-state` value and whether to report, by the seven rows of design §4, first match
/// wins (B2, B10, B14, B15, B17).
///
/// 1. Closed and not `done`: `done`.
/// 2. Closed: unchanged.
/// 3. `done`: `blocked` with open blockers, else `queued`.
/// 4. `queued` with open blockers: `blocked`.
/// 5. `blocked` with no open blockers and no sub-issues: `queued`.
/// 6. Active, with a cause that carries a blocker change: unchanged, reported.
/// 7. Anything else: unchanged, not reported.
pub fn decide(facts: Facts<'_>, cause: &Cause) -> Decision {
    let blocked = facts.open_blockers > 0;
    let changed = match cause {
        Cause::Sweep => false,
        Cause::Triggered { blocker_changes } => !blocker_changes.is_empty(),
    };
    let (next, report) = match (
        facts.closed,
        class(facts.current),
        blocked,
        facts.has_sub_issues,
        changed,
    ) {
        (true, Class::Done, _, _, _) => (None, false),
        (true, _, _, _, _) => (Some(DONE), false),
        (false, Class::Done, true, _, _) => (Some(BLOCKED), false),
        (false, Class::Done, false, _, _) => (Some(QUEUED), false),
        (false, Class::Queued, true, _, _) => (Some(BLOCKED), false),
        (false, Class::Blocked, false, false, _) => (Some(QUEUED), false),
        (false, Class::Active, _, _, true) => (None, true),
        (false, Class::None | Class::Queued | Class::Blocked | Class::Active, _, _, _) => {
            (None, false)
        }
    };
    Decision { next, report }
}
