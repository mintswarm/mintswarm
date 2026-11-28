//! Repository reads without a clone, design §9.4: a branch's head, the repository's default
//! branch and a file's bytes at a commit, through the contents, repository and branch endpoints,
//! and on top of them a file's text and the CODEOWNERS file in force at a commit.
//!
//! Every request goes through [`GitHubReads`]' GET path, so it shares its ETag cache, its retry
//! layer and policy, and its rate-limit pause. A file is read inline from the contents API; a
//! file above its 1 MB inline limit is refused as [`RepoReadError::TooLarge`].
//!
//! A file is read only at a full commit id; any other revision is refused as
//! [`RepoReadError::InvalidRev`] before a request, since the contents API would read a branch or
//! tag name, or the default branch for an empty one, as a moving ref.

use std::future::Future;

use serde_json::Value;

use mintswarm_domain::hosting::{CODEOWNERS_PATHS, CodeOwnersFile, RepoReadError, RepoReads};
use mintswarm_domain::ids::RevId;
use mintswarm_domain::retry::{Exhausted, Failure};
use mintswarm_domain::vcs::{BranchName, RepoPath};

use crate::key_store::decode_base64;
use crate::reads::{GitHubReads, ReadError};
use crate::rest::path_segment;
use crate::secret::Token;
use crate::token::Permissions;

/// Reads of one repository's branch heads and files at a commit, over a borrowed
/// [`GitHubReads`] and a token its caller minted with [`GitHubRepoReads::permissions`].
///
/// GitHub answers 404 for a file that is missing, but also for a commit it does not know and a
/// repository the token cannot see, so [`RepoReads::read_file`] reads all of these as `None`.
/// Pin the revision with [`RepoReads::branch_head`] through the same reader first: it
/// reports lost access as [`RepoReadError::Inaccessible`], and the commit it returns is known.
#[derive(Clone, Copy, Debug)]
pub struct GitHubRepoReads<'a> {
    reads: &'a GitHubReads,
    token: &'a Token,
}

impl GitHubReads {
    /// Repository reads through this client with `token`.
    pub fn repo_reads<'a>(&'a self, token: &'a Token) -> GitHubRepoReads<'a> {
        GitHubRepoReads { reads: self, token }
    }
}

impl GitHubRepoReads<'_> {
    /// The permissions a token for these reads asks for, from its
    /// [`TokenClient`](crate::TokenClient).
    pub fn permissions() -> Permissions {
        Permissions::read(["contents", "metadata"])
    }

    /// The text of the file at `path` in commit `rev`, read as
    /// [`read_file`](RepoReads::read_file) reads its bytes: `None` when there is no file, and
    /// [`RepoReadError::NotText`] when its bytes are not UTF-8.
    pub async fn read_text(
        &self,
        rev: &RevId,
        path: &RepoPath,
    ) -> Result<Option<String>, RepoReadError> {
        match self.file(rev, path).await? {
            Some(bytes) => String::from_utf8(bytes)
                .map(Some)
                .map_err(|_| RepoReadError::NotText),
            None => Ok(None),
        }
    }

    /// The CODEOWNERS file in force at commit `rev`: the first of [`CODEOWNERS_PATHS`] that
    /// holds a file, read in that order and no further, or `None` when none does. `rev` must be
    /// a full commit id, or the read fails with [`RepoReadError::InvalidRev`] before a request.
    pub async fn codeowners(&self, rev: &RevId) -> Result<Option<CodeOwnersFile>, RepoReadError> {
        if !is_commit_id(&rev.0) {
            return Err(RepoReadError::InvalidRev);
        }
        for candidate in CODEOWNERS_PATHS {
            let path = RepoPath(candidate.as_bytes().to_vec());
            if let Some(text) = self.read_text(rev, &path).await? {
                return Ok(Some(CodeOwnersFile { path, text }));
            }
        }
        Ok(None)
    }

    async fn head(&self, branch: &BranchName) -> Result<RevId, RepoReadError> {
        if !BranchName::is_valid(&branch.0) {
            return Err(RepoReadError::InvalidBranch);
        }
        let branch_path = format!(
            "{}/branches/{}",
            self.reads.repo_path(),
            encode_segments(&branch.0)
        );
        let body = self
            .reads
            .get(self.token, &branch_path)
            .await
            .map_err(head_error)?;
        let sha = body
            .get("commit")
            .and_then(|commit| commit.get("sha"))
            .and_then(Value::as_str)
            .filter(|sha| is_commit_id(sha))
            .ok_or(RepoReadError::Unexpected)?;
        Ok(RevId(sha.to_owned()))
    }

    async fn default_name(&self) -> Result<BranchName, RepoReadError> {
        let repo = self
            .reads
            .get(self.token, &self.reads.repo_path())
            .await
            .map_err(head_error)?;
        repo.get("default_branch")
            .and_then(Value::as_str)
            .filter(|name| BranchName::is_valid(name))
            .map(|name| BranchName(name.to_owned()))
            .ok_or(RepoReadError::Unexpected)
    }

    async fn file(&self, rev: &RevId, path: &RepoPath) -> Result<Option<Vec<u8>>, RepoReadError> {
        if !is_commit_id(&rev.0) {
            return Err(RepoReadError::InvalidRev);
        }
        let segments = checked_path(path)?;
        let contents_path = format!(
            "{}/contents/{}?ref={}",
            self.reads.repo_path(),
            segments,
            path_segment(&rev.0)
        );
        let body = match self.reads.get(self.token, &contents_path).await {
            Ok(body) => body,
            Err(ReadError::NotFound { .. }) => return Ok(None),
            Err(error) => return Err(read_error(error)),
        };
        file_bytes(&body)
    }
}

impl RepoReads for GitHubRepoReads<'_> {
    fn branch_head(
        &self,
        branch: &BranchName,
    ) -> impl Future<Output = Result<RevId, RepoReadError>> + Send {
        self.head(branch)
    }

    fn default_branch(&self) -> impl Future<Output = Result<BranchName, RepoReadError>> + Send {
        self.default_name()
    }

    fn read_file(
        &self,
        rev: &RevId,
        path: &RepoPath,
    ) -> impl Future<Output = Result<Option<Vec<u8>>, RepoReadError>> + Send {
        self.file(rev, path)
    }
}

/// `path` as percent-encoded segments joined by `/`, or [`RepoReadError::InvalidPath`] when it
/// is not UTF-8, is empty, or has an empty, `.` or `..` segment (a leading or trailing `/` makes
/// an empty one).
fn checked_path(path: &RepoPath) -> Result<String, RepoReadError> {
    let text = std::str::from_utf8(&path.0).map_err(|_| RepoReadError::InvalidPath)?;
    if text
        .split('/')
        .any(|segment| matches!(segment, "" | "." | ".."))
    {
        return Err(RepoReadError::InvalidPath);
    }
    Ok(encode_segments(text))
}

/// `text` with each `/`-separated segment percent-encoded and the `/` kept.
fn encode_segments(text: &str) -> String {
    text.split('/')
        .map(path_segment)
        .collect::<Vec<_>>()
        .join("/")
}

/// Whether `sha` is a full commit id: 40 or 64 lowercase hexadecimal characters.
pub fn is_commit_id(sha: &str) -> bool {
    matches!(sha.len(), 40 | 64)
        && sha
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// The bytes of a contents answer: `None` for a directory (a JSON array) or an entry that is
/// not a file (a symlink or a submodule).
fn file_bytes(body: &Value) -> Result<Option<Vec<u8>>, RepoReadError> {
    if body.is_array() {
        return Ok(None);
    }
    let kind = body
        .get("type")
        .and_then(Value::as_str)
        .ok_or(RepoReadError::Unexpected)?;
    if kind != "file" {
        return Ok(None);
    }
    match body.get("encoding").and_then(Value::as_str) {
        Some("base64") => {}
        Some("none") => return Err(RepoReadError::TooLarge),
        _ => return Err(RepoReadError::Unexpected),
    }
    let size = body
        .get("size")
        .and_then(Value::as_u64)
        .ok_or(RepoReadError::Unexpected)?;
    let content: Vec<u8> = body
        .get("content")
        .and_then(Value::as_str)
        .ok_or(RepoReadError::Unexpected)?
        .bytes()
        .filter(|byte| !matches!(byte, b'\n' | b'\r'))
        .collect();
    let bytes = decode_base64(&content).ok_or(RepoReadError::Unexpected)?;
    if u64::try_from(bytes.len()).ok() != Some(size) {
        return Err(RepoReadError::Unexpected);
    }
    Ok(Some(bytes))
}

/// A head or repository read's error: a missing repository or branch is
/// [`RepoReadError::Inaccessible`].
fn head_error(error: ReadError) -> RepoReadError {
    match error {
        ReadError::NotFound { .. } => RepoReadError::Inaccessible,
        other => read_error(other),
    }
}

/// A read error as its repository-read class; GitHub's text never passes through. A read whose
/// retries ran out on a rate limit is [`RepoReadError::RateLimited`] with the wait GitHub named,
/// and one that ran out on another transient failure is [`RepoReadError::Transport`]. A 401 or a
/// 403 that is not a rate limit is a credential or permission fault, so it is
/// [`RepoReadError::Inaccessible`].
fn read_error(error: ReadError) -> RepoReadError {
    match error {
        ReadError::Exhausted(Exhausted {
            last: Failure::RateLimited { wait },
            ..
        }) => RepoReadError::RateLimited { retry_after: wait },
        // Our own text, with the attempt count and the last failure's class.
        ReadError::Exhausted(exhausted) => RepoReadError::Transport(exhausted.to_string()),
        ReadError::Transport(detail) => RepoReadError::Transport(detail),
        ReadError::NotFound { .. }
        | ReadError::Status {
            status: 401 | 403, ..
        } => RepoReadError::Inaccessible,
        ReadError::Status { .. }
        | ReadError::Unexpected { .. }
        | ReadError::TooManyPages { .. }
        | ReadError::GraphQl { .. }
        | ReadError::CommaInLabel { .. } => RepoReadError::Unexpected,
    }
}

#[cfg(test)]
mod tests {
    use super::{checked_path, is_commit_id};
    use mintswarm_domain::hosting::RepoReadError;
    use mintswarm_domain::vcs::RepoPath;

    #[test]
    fn a_path_is_encoded_per_segment() {
        let path = RepoPath(b".mintswarm/stage defs/a+b.md".to_vec());
        assert_eq!(
            checked_path(&path).unwrap(),
            ".mintswarm/stage%20defs/a%2Bb.md"
        );
    }

    #[test]
    fn commit_ids_are_full_lowercase_hex() {
        assert!(is_commit_id(&"a".repeat(40)));
        assert!(is_commit_id(&"0".repeat(64)));
        assert!(!is_commit_id(&"A".repeat(40)));
        assert!(!is_commit_id(&"a".repeat(39)));
        assert!(!is_commit_id(&"g".repeat(40)));
    }

    #[test]
    fn unnameable_paths_are_refused() {
        for path in [&b""[..], b"/a", b"a/", b"a//b", b"./a", b"a/../b", b"\xff"] {
            assert_eq!(
                checked_path(&RepoPath(path.to_vec())),
                Err(RepoReadError::InvalidPath),
                "{path:?}"
            );
        }
    }
}
