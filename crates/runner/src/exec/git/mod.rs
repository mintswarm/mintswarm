//! The git VCS adapter, design §8: the trusted bare store, the neutral git environment, fetch
//! and the read operations, on the git command-line tool.
//!
//! Every git command runs through [`GitStore`]'s command builder, which applies the neutral
//! environment; nothing else in this module starts git.

mod diff;
mod env;
mod fetch;
mod read;
mod store;
#[cfg(test)]
pub(crate) mod tests;

use std::path::{Path, PathBuf};

use mintswarm_domain::ids::RevId;
use mintswarm_domain::plane::Credential;
use mintswarm_domain::vcs::{
    BranchName, ChangeSet, CommitInfo, DiffOpts, FetchReport, Identity, MergeOutcome, PushOutcome,
    RebaseOutcome, RefOrId, Remote, RemoteHead, RepoPath, RevRange, TreeEntry, TreeMode, Vcs,
    Workspace,
};

pub use diff::{DIFF_OPTIONS, PATCH_OPTIONS, PINNED_OPTIONS};
pub use store::{GIT_REQUIREMENT, store_path};

/// What went wrong in the git adapter.
#[derive(Debug, thiserror::Error)]
pub enum GitError {
    /// The `tool.git` probe result does not meet the store's requirement.
    #[error("git {found} does not meet the requirement `{requirement}`")]
    Version {
        /// The requirement, [`GIT_REQUIREMENT`].
        requirement: String,
        /// What the probe result gave.
        found: String,
    },
    /// The store's directory does not have mode 0700.
    #[error("the store {path} has mode {mode:o}, not 700")]
    Mode {
        /// The store's directory.
        path: PathBuf,
        /// The mode it has.
        mode: u32,
    },
    /// The store is not a bare repository.
    #[error("the store {0} is not a bare repository")]
    NotBare(PathBuf),
    /// The store is a shallow repository.
    #[error("the store {0} is shallow")]
    Shallow(PathBuf),
    /// The store is a partial clone, so a missing object could be fetched lazily.
    #[error("the store {path} is a partial clone: `{key}` is set")]
    Partial {
        /// The store's directory.
        path: PathBuf,
        /// The config key that makes it one.
        key: String,
    },
    /// The store borrows objects from another repository.
    #[error("the store {0} has a non-empty objects/info/alternates")]
    Alternates(PathBuf),
    /// The store has attributes of its own, which no option makes git ignore.
    #[error("the store {0} has a non-empty info/attributes")]
    Attributes(PathBuf),
    /// Revision syntax other than a full id, or `ID..ID` where a range is read.
    #[error("`{0}` is not a full id")]
    RevisionSyntax(String),
    /// A full id that names no commit in the store.
    #[error("{0} is not a commit in the store")]
    NotFound(String),
    /// A remote URL the adapter does not fetch from.
    #[error("the remote URL is refused: {0}")]
    Url(String),
    /// A fetched ref or id that does not end at a commit, such as a tree, a blob or a tag of one.
    #[error("`{0}` is not a commit")]
    NotCommit(String),
    /// A ref or remote name the adapter does not fetch.
    #[error("`{0}` is not a full branch or tag name")]
    RefName(String),
    /// A repository name that cannot name a store.
    #[error("`{0}` cannot name a store")]
    StoreName(String),
    /// A git command failed. The text is ours alone; what git wrote is kept in `stderr` for
    /// logs and diagnostics, never in the text.
    #[error("git {command} failed ({status})")]
    Git {
        /// The git subcommand.
        command: String,
        /// Its exit status, in our words: `exit code N` or `no exit code`.
        status: String,
        /// The start of what it wrote to standard error.
        stderr: String,
    },
    /// A git command's output was not what the adapter expects.
    #[error("git {command} gave unexpected output: {detail}")]
    Parse {
        /// The git subcommand.
        command: String,
        /// What was unexpected.
        detail: String,
    },
    /// A file or process operation failed. The text is `context` alone; the underlying error is
    /// only the `source()`.
    #[error("{context}")]
    Io {
        /// What the adapter was doing.
        context: String,
        /// The underlying error.
        #[source]
        source: std::io::Error,
    },
    /// An operation this adapter does not implement yet.
    #[error("{0} is not supported by the git adapter yet")]
    Unsupported(&'static str),
}

/// The trusted bare store of one repository.
#[derive(Debug)]
pub struct GitStore {
    /// The store's directory, absolute.
    path: PathBuf,
    /// The git program.
    program: PathBuf,
    /// The empty tree's id, for `GIT_ATTR_SOURCE`; `None` until the store is opened.
    attr_source: Option<String>,
    /// The length of a full id in the store's object format.
    id_len: usize,
    /// Warnings found when the store was opened.
    warnings: Vec<String>,
    /// Whether `file://` remotes are allowed; only tests set it.
    allow_file: bool,
}

impl GitStore {
    /// The store's directory.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The warnings found when the store was opened, such as an unknown git version.
    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }
}

impl Vcs for GitStore {
    type Error = GitError;

    fn fetch(
        &self,
        from: &Remote,
        wants: &[RefOrId],
        cred: Option<&Credential>,
    ) -> Result<FetchReport, GitError> {
        fetch::fetch(self, from, wants, cred)
    }

    fn resolve(&self, rev: &str) -> Result<RevId, GitError> {
        read::resolve(self, rev)
    }

    fn read_blob(&self, rev: &RevId, path: &RepoPath) -> Result<Option<Vec<u8>>, GitError> {
        read::read_blob(self, rev, path)
    }

    fn list_tree(&self, rev: &RevId) -> Result<Vec<TreeEntry>, GitError> {
        read::list_tree(self, rev)
    }

    fn log(&self, revs: &RevRange, paths: &[RepoPath]) -> Result<Vec<CommitInfo>, GitError> {
        read::log(self, revs, paths)
    }

    fn merge_base(&self, a: &RevId, b: &RevId) -> Result<Option<RevId>, GitError> {
        read::merge_base(self, a, b)
    }

    fn diff(&self, base: &RevId, head: &RevId, opts: DiffOpts) -> Result<ChangeSet, GitError> {
        diff::diff(self, base, head, &opts)
    }

    fn materialise(
        &self,
        _rev: &RevId,
        _dest: &Path,
        _mode: TreeMode,
    ) -> Result<Workspace, GitError> {
        Err(GitError::Unsupported("materialise"))
    }

    fn changes(&self, _ws: &Workspace) -> Result<ChangeSet, GitError> {
        Err(GitError::Unsupported("changes"))
    }

    fn commit(
        &self,
        _ws: &Workspace,
        _paths: &[RepoPath],
        _msg: &str,
        _who: &Identity,
    ) -> Result<RevId, GitError> {
        Err(GitError::Unsupported("commit"))
    }

    fn merge_into(&self, _ws: &Workspace, _theirs: &RevId) -> Result<MergeOutcome, GitError> {
        Err(GitError::Unsupported("merge_into"))
    }

    fn rebase_onto(&self, _ws: &Workspace, _onto: &RevId) -> Result<RebaseOutcome, GitError> {
        Err(GitError::Unsupported("rebase_onto"))
    }

    fn push(
        &self,
        _branch: &BranchName,
        _rev: &RevId,
        _expect: RemoteHead,
        _to: &Remote,
        _cred: &Credential,
    ) -> Result<PushOutcome, GitError> {
        Err(GitError::Unsupported("push"))
    }
}
