//! The version control abstraction, design §8: the trusted store, trees given to stages, and
//! publishing.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::ids::RevId;
use crate::plane::Credential;

/// A remote repository.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Remote {
    pub name: String,
    pub url: String,
}

/// What to fetch: a ref by name, or a commit by its full id.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RefOrId {
    Ref(String),
    Id(RevId),
}

/// A path in a repository's tree, as bytes.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RepoPath(pub Vec<u8>);

/// An entry of a tree.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TreeEntry {
    pub path: RepoPath,
    /// The git file mode, for example `0o100644`.
    pub mode: u32,
    pub blob: RevId,
}

/// The commits reachable from `to` and not from `from`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RevRange {
    pub from: Option<RevId>,
    pub to: RevId,
}

/// Who made a commit.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Identity {
    pub name: String,
    pub email: String,
}

/// A commit as `log` reads it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommitInfo {
    pub id: RevId,
    pub parents: Vec<RevId>,
    pub author: Identity,
    /// RFC 3339.
    pub committed_at: String,
    pub subject: String,
    pub body: String,
}

/// What a diff reports, besides the changed paths.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiffOpts {
    pub numstat: bool,
    pub added: bool,
    pub raw: bool,
    pub patch: bool,
}

/// How a path changed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeKind {
    Added,
    Modified,
    Deleted,
    Renamed,
    Copied,
    TypeChanged,
}

/// One changed path.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileChange {
    pub path: RepoPath,
    pub old_path: Option<RepoPath>,
    pub kind: ChangeKind,
    pub old_mode: Option<u32>,
    pub new_mode: Option<u32>,
    /// The blob or gitlink id before the change; `None` when the path did not exist.
    pub old_blob: Option<RevId>,
    /// The blob or gitlink id after the change; `None` when the path no longer exists.
    pub new_blob: Option<RevId>,
    /// Added and deleted lines; `None` for a binary file or without `numstat`.
    pub lines: Option<(u64, u64)>,
}

/// The changes between two trees.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangeSet {
    pub files: Vec<FileChange>,
    /// The unified diff, with `patch`.
    pub patch: Option<Vec<u8>>,
}

/// Whether a tree given to a stage may be written.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TreeMode {
    ReadOnly,
    Writable,
}

/// A materialised tree: a read-only snapshot or a job's private worktree, with no `.git` entry.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Workspace {
    pub path: PathBuf,
    /// The commit the tree was materialised from, or last committed.
    pub parent: RevId,
    pub mode: TreeMode,
}

/// The result of merging a commit into a worktree.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MergeOutcome {
    Clean(RevId),
    Conflicts(Vec<RepoPath>),
}

/// The result of rebasing a worktree's commits.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RebaseOutcome {
    Rebased(RevId),
    Conflicts(Vec<RepoPath>),
}

/// A branch name, without `refs/heads/`.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct BranchName(pub String);

/// The remote head a push expects to replace.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RemoteHead {
    Absent,
    At(RevId),
}

/// The result of a push.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PushOutcome {
    Pushed(RevId),
    /// The remote head was not the expected one.
    Rejected(RemoteHead),
}

/// A ref a fetch updated.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FetchedRef {
    pub name: String,
    pub id: RevId,
}

/// What a fetch brought into the store.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FetchReport {
    pub refs: Vec<FetchedRef>,
}

/// A repository: the trusted store, the trees made from it, and pushes from it.
pub trait Vcs: Send + Sync {
    type Error: std::error::Error + Send + Sync + 'static;

    /// Fetch refs or commits into the trusted store.
    fn fetch(
        &self,
        from: &Remote,
        wants: &[RefOrId],
        cred: Option<&Credential>,
    ) -> Result<FetchReport, Self::Error>;

    /// The full id of a revision; never revision syntax from a stage.
    fn resolve(&self, rev: &str) -> Result<RevId, Self::Error>;

    /// A file's bytes at a commit, or `None` when the path is not a file there.
    fn read_blob(&self, rev: &RevId, path: &RepoPath) -> Result<Option<Vec<u8>>, Self::Error>;

    /// Every entry of a commit's tree.
    fn list_tree(&self, rev: &RevId) -> Result<Vec<TreeEntry>, Self::Error>;

    /// The commits of a range that touch the paths, newest first.
    fn log(&self, revs: &RevRange, paths: &[RepoPath]) -> Result<Vec<CommitInfo>, Self::Error>;

    /// Every best common ancestor of two commits, as `git merge-base --all` finds them; empty
    /// when they share no history.
    fn merge_bases(&self, a: &RevId, b: &RevId) -> Result<Vec<RevId>, Self::Error>;

    /// Whether `a` is reachable from `b`; true when `a == b`.
    fn is_ancestor(&self, a: &RevId, b: &RevId) -> Result<bool, Self::Error>;

    /// The changes from one commit to another, limited to the paths and the trees under them;
    /// an empty `paths` means the whole tree.
    fn diff(
        &self,
        base: &RevId,
        head: &RevId,
        opts: DiffOpts,
        paths: &[RepoPath],
    ) -> Result<ChangeSet, Self::Error>;

    /// Make a tree of a commit at `dest`.
    fn materialise(
        &self,
        rev: &RevId,
        dest: &Path,
        mode: TreeMode,
    ) -> Result<Workspace, Self::Error>;

    /// A worktree's changes against its parent.
    fn changes(&self, ws: &Workspace) -> Result<ChangeSet, Self::Error>;

    /// Commit the paths of a worktree.
    fn commit(
        &self,
        ws: &Workspace,
        paths: &[RepoPath],
        msg: &str,
        who: &Identity,
    ) -> Result<RevId, Self::Error>;

    /// Merge a commit into a worktree.
    fn merge_into(&self, ws: &Workspace, theirs: &RevId) -> Result<MergeOutcome, Self::Error>;

    /// Rebase a worktree's commits; before the first push only.
    fn rebase_onto(&self, ws: &Workspace, onto: &RevId) -> Result<RebaseOutcome, Self::Error>;

    /// Push a commit to a branch, if the remote head is the expected one.
    fn push(
        &self,
        branch: &BranchName,
        rev: &RevId,
        expect: RemoteHead,
        to: &Remote,
        cred: &Credential,
    ) -> Result<PushOutcome, Self::Error>;
}
