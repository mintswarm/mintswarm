//! `tracker.ingest`: which issues are trusted to start a job, design §5.3 (B4).
//!
//! [`Trust::trusts`] is the one trust predicate: whatever decides whether an issue is
//! trusted calls it.

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
