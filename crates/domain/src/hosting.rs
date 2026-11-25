//! Reads of a hosted repository without a clone, design §9.4: the plane pins the base commit and
//! reads config and stage definitions through the hosting integration.

use std::fmt;
use std::future::Future;
use std::time::Duration;

use crate::ids::RevId;
use crate::vcs::RepoPath;

/// Why a repository read failed. Every host maps its own errors into these classes, with this
/// type's text, so a caller can tell a rate limit from any other failure without knowing the
/// host.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RepoReadError {
    /// The host refused the request for a rate limit; it may say how long to wait.
    RateLimited {
        /// The delay the host asked for, if any.
        retry_after: Option<Duration>,
    },
    /// The repository or its default branch is missing or not shown to the credential, or the
    /// credential is refused or lacks the permission to read it.
    Inaccessible,
    /// The path cannot be named on the host: not UTF-8, empty, with an empty, `.` or `..`
    /// segment, or with a leading or trailing `/`.
    InvalidPath,
    /// The revision is not a full commit id, so a read at it could follow a moving ref.
    InvalidRev,
    /// The file is too large for the host to return inline.
    TooLarge,
    /// The host answered with a status or a shape the reader does not expect.
    Unexpected,
    /// The request could not be sent or its answer not read.
    Transport(String),
}

impl fmt::Display for RepoReadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RateLimited {
                retry_after: Some(delay),
            } => write!(f, "rate limited; retry after {}s", delay.as_secs()),
            Self::RateLimited { retry_after: None } => f.write_str("rate limited"),
            Self::Inaccessible => f.write_str("the repository or its branch is not accessible"),
            Self::InvalidPath => f.write_str("the path cannot be read from the host"),
            Self::InvalidRev => f.write_str("the revision is not a full commit id"),
            Self::TooLarge => f.write_str("the file is too large to read inline"),
            Self::Unexpected => f.write_str("the host's answer is not of the expected shape"),
            Self::Transport(detail) => write!(f, "transport error: {detail}"),
        }
    }
}

impl std::error::Error for RepoReadError {}

impl RepoReadError {
    /// The delay a rate limit asked for; `None` for every other class, or when the host gave
    /// none.
    pub fn retry_after(&self) -> Option<Duration> {
        match self {
            Self::RateLimited { retry_after } => *retry_after,
            _ => None,
        }
    }

    /// Whether the host refused the read for a rate limit.
    pub fn is_rate_limited(&self) -> bool {
        matches!(self, Self::RateLimited { .. })
    }
}

/// Reads of one hosted repository at a commit, without a clone.
pub trait RepoReads: Send + Sync {
    /// The commit at the head of the repository's default branch.
    fn default_branch_head(&self) -> impl Future<Output = Result<RevId, RepoReadError>> + Send;

    /// The bytes of the file at `path` in commit `rev`, which must be a full commit id or the
    /// read fails with [`RepoReadError::InvalidRev`]. `None` means there is no file at the path,
    /// the same meaning as [`Vcs::read_blob`](crate::vcs::Vcs::read_blob).
    ///
    /// A host may also answer `None` when it does not show the commit or the repository at
    /// all: one it does not know, or one the credential can no longer see. A caller should pin
    /// `rev` with [`default_branch_head`](Self::default_branch_head) through the same reader
    /// first, which reports lost access as [`RepoReadError::Inaccessible`], before it treats
    /// `None` as an absent file.
    fn read_file(
        &self,
        rev: &RevId,
        path: &RepoPath,
    ) -> impl Future<Output = Result<Option<Vec<u8>>, RepoReadError>> + Send;
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::RepoReadError;

    fn others() -> [RepoReadError; 6] {
        [
            RepoReadError::Inaccessible,
            RepoReadError::InvalidPath,
            RepoReadError::InvalidRev,
            RepoReadError::TooLarge,
            RepoReadError::Unexpected,
            RepoReadError::Transport("GET /repos/a/b: the request failed".into()),
        ]
    }

    #[test]
    fn a_rate_limit_carries_its_delay() {
        let limited = RepoReadError::RateLimited {
            retry_after: Some(Duration::from_secs(7)),
        };
        assert!(limited.is_rate_limited());
        assert_eq!(limited.retry_after(), Some(Duration::from_secs(7)));
    }

    #[test]
    fn a_rate_limit_without_a_delay_has_none() {
        let limited = RepoReadError::RateLimited { retry_after: None };
        assert!(limited.is_rate_limited());
        assert_eq!(limited.retry_after(), None);
    }

    #[test]
    fn other_classes_are_not_rate_limits() {
        for error in others() {
            assert!(!error.is_rate_limited(), "{error:?}");
            assert_eq!(error.retry_after(), None, "{error:?}");
        }
    }

    #[test]
    fn every_class_has_its_own_text() {
        let mut texts: Vec<String> = others().iter().map(ToString::to_string).collect();
        texts.push(RepoReadError::RateLimited { retry_after: None }.to_string());
        texts.push(
            RepoReadError::RateLimited {
                retry_after: Some(Duration::from_secs(2)),
            }
            .to_string(),
        );
        let count = texts.len();
        texts.sort();
        texts.dedup();
        assert_eq!(texts.len(), count, "{texts:?}");
    }
}
