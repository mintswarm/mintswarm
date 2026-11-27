//! Whom the plane trusts, design §5.3 (B4), in two tables under `tracker`:
//!
//! - `tracker.ingest`: which issues are trusted to start a job. [`Trust::trusts`] is the one
//!   issue-trust predicate: whatever decides whether an issue is trusted calls it.
//! - `tracker.actions`: who is trusted with each decision action, an [`ActionTable`] with one
//!   [`ActionTrust`] row per [`Action`]. A row names a [`People`] rule and the App slugs trusted
//!   with the action; only [`Action::Merge`] may trust an App. [`Action::NAMES`] is the only
//!   place the action names are spelled.

use mintswarm_domain::tracker::{Issue, Login};

/// `tracker.ingest.match`: how the trusted labels and authors combine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrustMatch {
    /// A trusted label or a trusted author is enough.
    Any,
    /// Both a trusted label and a trusted author are needed.
    All,
}

impl TrustMatch {
    pub(crate) const NAMES: [(&'static str, TrustMatch); 2] =
        [("any", TrustMatch::Any), ("all", TrustMatch::All)];
}

/// `tracker.ingest`, typed. The default trusts nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Trust {
    /// `trusted.labels`.
    pub labels: Vec<String>,
    /// `trusted.authors`.
    pub authors: Vec<Login>,
    /// `match`, `any` when absent.
    pub r#match: TrustMatch,
}

impl Default for Trust {
    fn default() -> Self {
        Trust {
            labels: Vec::new(),
            authors: Vec::new(),
            r#match: TrustMatch::Any,
        }
    }
}

impl Trust {
    /// Whether `issue` is trusted: it carries a trusted label or has a trusted author under
    /// `any`, and both under `all`.
    ///
    /// Labels and logins are compared ASCII case-insensitively, as GitHub treats them. An empty
    /// list matches nothing.
    pub fn trusts(&self, issue: &Issue) -> bool {
        let label = issue
            .labels
            .iter()
            .any(|label| self.labels.iter().any(|t| t.eq_ignore_ascii_case(label)));
        let author = self
            .authors
            .iter()
            .any(|t| t.0.eq_ignore_ascii_case(&issue.author.0));
        match self.r#match {
            TrustMatch::Any => label || author,
            TrustMatch::All => label && author,
        }
    }
}

/// A decision action whose actor the plane checks against `tracker.actions`.
///
/// The discriminants index [`Action::NAMES`] and the rows of an [`ActionTable`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Action {
    /// Approving a plan.
    ApprovePlan,
    /// Approving a design.
    ApproveDesign,
    /// Approving a decomposition.
    ApproveDecomposition,
    /// Approving a pull request.
    ApprovePr,
    /// Merging a pull request.
    Merge,
}

/// The number of actions, and of rows in an [`ActionTable`].
const ACTIONS: usize = 5;

impl Action {
    /// Each action's name in `tracker.actions`, in discriminant order. The only place the names
    /// are spelled.
    pub const NAMES: [(&'static str, Action); ACTIONS] = [
        ("approve_plan", Action::ApprovePlan),
        ("approve_design", Action::ApproveDesign),
        ("approve_decomposition", Action::ApproveDecomposition),
        ("approve_pr", Action::ApprovePr),
        ("merge", Action::Merge),
    ];

    /// Every action, in [`Action::NAMES`] order.
    pub const ALL: [Action; ACTIONS] = {
        let mut all = [Action::ApprovePlan; ACTIONS];
        let mut n = 0;
        while n < ACTIONS {
            all[n] = Action::NAMES[n].1;
            // NAMES is in discriminant order, so `as_str` and the table can index by it.
            assert!(Action::NAMES[n].1 as usize == n);
            n += 1;
        }
        all
    };

    /// The action's name in `tracker.actions`.
    pub const fn as_str(self) -> &'static str {
        Action::NAMES[self as usize].0
    }
}

/// `tracker.actions.<action>.people`: which people are trusted with an action.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum People {
    /// The issue's owner, with write access and not a bot; issue approvals only.
    IssueOwner,
    /// The owners `CODEOWNERS` gives for the change; [`Action::ApprovePr`] only.
    Codeowners,
    /// No one; allowed in every row.
    #[default]
    None,
}

impl People {
    /// Each rule's name in `tracker.actions.<action>.people`.
    pub const NAMES: [(&'static str, People); 3] = [
        ("issue_owner", People::IssueOwner),
        ("codeowners", People::Codeowners),
        ("none", People::None),
    ];

    /// Whether the rule may stand in `action`'s row.
    pub fn allowed_in(self, action: Action) -> bool {
        match self {
            People::IssueOwner => matches!(
                action,
                Action::ApprovePlan | Action::ApproveDesign | Action::ApproveDecomposition
            ),
            People::Codeowners => action == Action::ApprovePr,
            People::None => true,
        }
    }
}

/// One `tracker.actions.<action>` row. The default trusts no one.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ActionTrust {
    /// `people`.
    pub people: People,
    /// `apps`: App slugs, without the `[bot]` suffix their logins carry; empty in every row
    /// but [`Action::Merge`].
    pub apps: Vec<String>,
}

/// `tracker.actions`, typed: one row for every [`Action`], so no row can be missing. The default
/// trusts no one with any action.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ActionTable {
    rows: [ActionTrust; ACTIONS],
}

impl ActionTable {
    /// `action`'s row.
    pub fn get(&self, action: Action) -> &ActionTrust {
        &self.rows[action as usize]
    }

    pub(crate) fn get_mut(&mut self, action: Action) -> &mut ActionTrust {
        &mut self.rows[action as usize]
    }
}
