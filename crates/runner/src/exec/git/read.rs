//! The read operations: `resolve`, ranges, blobs, trees, logs, merge bases and ancestry. Revisions are
//! full ids only; no revision syntax reaches git.

use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;

use mintswarm_domain::ids::RevId;
use mintswarm_domain::vcs::{CommitInfo, RepoPath, RevRange, TreeEntry};

use super::env::{failure, success};
use super::{GitError, GitStore, commits};

impl GitStore {
    /// Whether `text` is a full id in the store's object format, other than the zero id.
    pub(super) fn is_full_id(&self, text: &str) -> bool {
        text.len() == self.id_len
            && text
                .bytes()
                .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
            && text.bytes().any(|byte| byte != b'0')
    }

    /// `rev` when it is a full id; never revision syntax.
    pub(super) fn checked_id<'a>(&self, rev: &'a RevId) -> Result<&'a str, GitError> {
        if self.is_full_id(&rev.0) {
            Ok(&rev.0)
        } else {
            Err(GitError::RevisionSyntax(rev.0.clone()))
        }
    }

    /// A range of `ID` or `ID..ID`, each a full id naming a commit in the store; nothing else.
    pub fn parse_range(&self, text: &str) -> Result<RevRange, GitError> {
        match text.split_once("..") {
            None => Ok(RevRange {
                from: None,
                to: resolve(self, text)?,
            }),
            Some((from, to)) => {
                if !self.is_full_id(from) || !self.is_full_id(to) {
                    return Err(GitError::RevisionSyntax(text.to_owned()));
                }
                Ok(RevRange {
                    from: Some(resolve(self, from)?),
                    to: resolve(self, to)?,
                })
            }
        }
    }
}

/// The full id `rev` gives, when it is a full id naming a commit in the store.
pub(super) fn resolve(store: &GitStore, rev: &str) -> Result<RevId, GitError> {
    if !store.is_full_id(rev) {
        return Err(GitError::RevisionSyntax(rev.to_owned()));
    }
    let spec = format!("{rev}^{{commit}}");
    let output = store.output(
        store.command(["rev-parse", "--verify", "--quiet", spec.as_str()]),
        "rev-parse",
    )?;
    match output.status.code() {
        Some(0) if output.stdout.trim_ascii_end() == rev.as_bytes() => Ok(RevId(rev.to_owned())),
        Some(0) => Err(GitError::Parse {
            command: "rev-parse".to_owned(),
            detail: format!("{rev} resolved to another id"),
        }),
        Some(1) => Err(GitError::NotFound(rev.to_owned())),
        _ => Err(failure("rev-parse", &output)),
    }
}

/// A commit's id, after checking it is a full id naming a commit in the store.
pub(super) fn commit<'a>(store: &GitStore, rev: &'a RevId) -> Result<&'a str, GitError> {
    let id = store.checked_id(rev)?;
    resolve(store, id)?;
    Ok(id)
}

/// A path as one command-line argument, unless it cannot be one.
fn path_arg(path: &RepoPath) -> Option<&OsStr> {
    (!path.0.is_empty() && !path.0.contains(&0)).then(|| OsStr::from_bytes(&path.0))
}

/// The paths `command` limits itself to, as command-line arguments; an empty path or one with a
/// NUL byte is refused.
pub(super) fn path_args<'a>(
    paths: &'a [RepoPath],
    command: &str,
) -> Result<Vec<&'a OsStr>, GitError> {
    paths
        .iter()
        .map(|path| {
            path_arg(path).ok_or_else(|| GitError::Parse {
                command: command.to_owned(),
                detail: "an empty path or one with a NUL byte".to_owned(),
            })
        })
        .collect()
}

/// One `ls-tree -z` record: mode, type, id and path.
struct LsTreeEntry<'a> {
    mode: u32,
    kind: &'a [u8],
    id: &'a [u8],
    path: &'a [u8],
}

fn ls_tree_entries(out: &[u8]) -> Result<Vec<LsTreeEntry<'_>>, GitError> {
    let bad = |record: &[u8]| GitError::Parse {
        command: "ls-tree".to_owned(),
        detail: format!("record {:?}", String::from_utf8_lossy(record)),
    };
    let records = out.strip_suffix(b"\0").unwrap_or(out);
    if records.is_empty() {
        return Ok(Vec::new());
    }
    records
        .split(|&byte| byte == 0)
        .map(|record| {
            let tab = record
                .iter()
                .position(|&byte| byte == b'\t')
                .ok_or_else(|| bad(record))?;
            let (head, path) = (&record[..tab], &record[tab + 1..]);
            let mut fields = head.split(|&byte| byte == b' ');
            let (Some(mode), Some(kind), Some(id), None) =
                (fields.next(), fields.next(), fields.next(), fields.next())
            else {
                return Err(bad(record));
            };
            let mode = std::str::from_utf8(mode)
                .ok()
                .and_then(|mode| u32::from_str_radix(mode, 8).ok())
                .ok_or_else(|| bad(record))?;
            if path.is_empty() {
                return Err(bad(record));
            }
            Ok(LsTreeEntry {
                mode,
                kind,
                id,
                path,
            })
        })
        .collect()
}

/// A file's bytes at a commit; `None` for a tree, a gitlink or a missing path.
pub(super) fn read_blob(
    store: &GitStore,
    rev: &RevId,
    path: &RepoPath,
) -> Result<Option<Vec<u8>>, GitError> {
    let id = commit(store, rev)?;
    let Some(arg) = path_arg(path) else {
        return Ok(None);
    };
    let out = store.git([
        OsStr::new("ls-tree"),
        OsStr::new("-z"),
        OsStr::new("--full-tree"),
        OsStr::new(id),
        OsStr::new("--"),
        arg,
    ])?;
    let blob = ls_tree_entries(&out)?
        .into_iter()
        .find(|entry| entry.path == path.0.as_slice() && entry.kind == b"blob");
    match blob {
        None => Ok(None),
        Some(entry) => {
            let id = OsStr::from_bytes(entry.id);
            store
                .git([OsStr::new("cat-file"), OsStr::new("blob"), id])
                .map(Some)
        }
    }
}

/// Every blob and gitlink of a commit's tree, recursively.
pub(super) fn list_tree(store: &GitStore, rev: &RevId) -> Result<Vec<TreeEntry>, GitError> {
    let id = commit(store, rev)?;
    let out = store.git(["ls-tree", "-r", "-z", "--full-tree", id])?;
    ls_tree_entries(&out)?
        .into_iter()
        .map(|entry| {
            Ok(TreeEntry {
                path: RepoPath(entry.path.to_vec()),
                mode: entry.mode,
                blob: RevId(id_text(store, entry.id, "ls-tree")?),
            })
        })
        .collect()
}

/// An id git wrote, checked to be a full id.
pub(super) fn id_text(store: &GitStore, bytes: &[u8], command: &str) -> Result<String, GitError> {
    let text = String::from_utf8_lossy(bytes).into_owned();
    if store.is_full_id(&text) {
        Ok(text)
    } else {
        Err(GitError::Parse {
            command: command.to_owned(),
            detail: format!("`{text}` is not a full id"),
        })
    }
}

/// The commits reachable from `revs.to` and not from `revs.from`, children before their
/// parents. With `paths`, only the commits with first-parent changes under them, each keeping
/// those changes alone; merges are judged by their first-parent changes like any commit.
pub(super) fn log(
    store: &GitStore,
    revs: &RevRange,
    paths: &[RepoPath],
) -> Result<Vec<CommitInfo>, GitError> {
    path_args(paths, "log")?;
    let mut args = vec![
        "rev-list".to_owned(),
        "--topo-order".to_owned(),
        commit(store, &revs.to)?.to_owned(),
    ];
    if let Some(from) = &revs.from {
        args.push(format!("^{}", commit(store, from)?));
    }
    args.push("--".to_owned());
    let ids = store
        .git(&args)?
        .split(|&byte| byte == b'\n')
        .filter(|line| !line.is_empty())
        .map(|line| id_text(store, line, "rev-list").map(RevId))
        .collect::<Result<Vec<_>, _>>()?;
    let mut commits = commits::read(store, &ids)?;
    if !paths.is_empty() {
        commits.retain_mut(|commit| {
            commit.changes.retain(|change| under(&change.path, paths));
            !commit.changes.is_empty()
        });
    }
    Ok(commits)
}

/// Whether `path` is one of `paths` or lies under one of them as a directory.
fn under(path: &RepoPath, paths: &[RepoPath]) -> bool {
    paths.iter().any(|limit| {
        path.0 == limit.0
            || path
                .0
                .strip_prefix(limit.0.as_slice())
                .is_some_and(|rest| rest.starts_with(b"/"))
    })
}

/// One commit, with its first-parent changes.
pub(super) fn read_commit(store: &GitStore, rev: &RevId) -> Result<CommitInfo, GitError> {
    let id = RevId(commit(store, rev)?.to_owned());
    let mut commits = commits::read(store, std::slice::from_ref(&id))?;
    match (commits.pop(), commits.is_empty()) {
        (Some(info), true) => Ok(info),
        _ => Err(GitError::Parse {
            command: "cat-file".to_owned(),
            detail: format!("not one record for {}", id.0),
        }),
    }
}

/// The raw subjects of every commit reachable from `rev`, as
/// `git -c log.showSignature=false log -z --format=%s <commit> --` writes them: neither re-encoded
/// nor decoded, so their digest is the one `plan-approval/floor.py` binds.
pub(crate) fn subjects(store: &GitStore, rev: &RevId) -> Result<Vec<u8>, GitError> {
    let id = commit(store, rev)?;
    success(
        "log",
        store.output(
            store.command_with(
                &["log.showSignature=false"],
                ["log", "-z", "--format=%s", id, "--"],
            ),
            "log",
        )?,
    )
}

/// Every best common ancestor of two commits; empty when they share no history.
pub(super) fn merge_bases(store: &GitStore, a: &RevId, b: &RevId) -> Result<Vec<RevId>, GitError> {
    let a = commit(store, a)?;
    let b = commit(store, b)?;
    let output = store.output(store.command(["merge-base", "--all", a, b]), "merge-base")?;
    if output.status.code() == Some(1) && output.stdout.is_empty() {
        return Ok(Vec::new());
    }
    let out = success("merge-base", output)?;
    out.split(|&byte| byte == b'\n')
        .filter(|line| !line.is_empty())
        .map(|line| id_text(store, line, "merge-base").map(RevId))
        .collect()
}

/// Whether `a` is reachable from `b`, `a == b` included.
pub(super) fn is_ancestor(store: &GitStore, a: &RevId, b: &RevId) -> Result<bool, GitError> {
    let a = commit(store, a)?;
    let b = commit(store, b)?;
    let output = store.output(
        store.command(["merge-base", "--is-ancestor", a, b]),
        "merge-base",
    )?;
    match output.status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ => Err(failure("merge-base", &output)),
    }
}
