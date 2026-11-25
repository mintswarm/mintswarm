//! The read operations: `resolve`, ranges, blobs, trees, logs and merge bases. Revisions are
//! full ids only; no revision syntax reaches git.

use std::ffi::{OsStr, OsString};
use std::os::unix::ffi::OsStrExt;

use mintswarm_domain::ids::RevId;
use mintswarm_domain::vcs::{CommitInfo, Identity, RepoPath, RevRange, TreeEntry};

use super::env::{failure, success};
use super::{GitError, GitStore};

/// The fields of one `log` record, as `LOG_FORMAT` writes them.
const LOG_FIELDS: usize = 7;
const LOG_FORMAT: &str = "--format=%H%x00%P%x00%an%x00%ae%x00%cI%x00%s%x00%b";
/// Settings `log` gets over the store's own config: no signatures in the stream, UTF-8 text,
/// and the same commits for one path as for several.
const LOG_CONFIG: &[&str] = &[
    "log.showSignature=false",
    "i18n.logOutputEncoding=UTF-8",
    "log.follow=false",
];

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

/// The commits reachable from `revs.to` and not from `revs.from` that touch `paths`, newest
/// first; merges included.
pub(super) fn log(
    store: &GitStore,
    revs: &RevRange,
    paths: &[RepoPath],
) -> Result<Vec<CommitInfo>, GitError> {
    let mut args: Vec<OsString> = ["log", "-z", "--no-color", LOG_FORMAT]
        .map(OsString::from)
        .into();
    args.push(commit(store, &revs.to)?.into());
    if let Some(from) = &revs.from {
        args.push(format!("^{}", commit(store, from)?).into());
    }
    args.push("--".into());
    args.extend(path_args(paths, "log")?.into_iter().map(OsStr::to_owned));
    let out = success(
        "log",
        store.output(store.command_with(LOG_CONFIG, &args), "log")?,
    )?;
    let Some(out) = out.strip_suffix(b"\0") else {
        return if out.is_empty() {
            Ok(Vec::new())
        } else {
            Err(GitError::Parse {
                command: "log".to_owned(),
                detail: "output without its final NUL".to_owned(),
            })
        };
    };
    let fields: Vec<&[u8]> = out.split(|&byte| byte == 0).collect();
    if fields.len() % LOG_FIELDS != 0 {
        return Err(GitError::Parse {
            command: "log".to_owned(),
            detail: format!("{} fields, not records of {LOG_FIELDS}", fields.len()),
        });
    }
    let text = |bytes: &[u8]| String::from_utf8_lossy(bytes).into_owned();
    fields
        .chunks(LOG_FIELDS)
        .map(|record| {
            let parents = record[1]
                .split(|&byte| byte == b' ')
                .filter(|parent| !parent.is_empty())
                .map(|parent| id_text(store, parent, "log").map(RevId))
                .collect::<Result<_, _>>()?;
            Ok(CommitInfo {
                id: RevId(id_text(store, record[0], "log")?),
                parents,
                author: Identity {
                    name: text(record[2]),
                    email: text(record[3]),
                },
                committed_at: text(record[4]),
                subject: text(record[5]),
                body: text(record[6]),
            })
        })
        .collect()
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

/// The best common ancestor of two commits, or `None` when they share no history.
pub(super) fn merge_base(
    store: &GitStore,
    a: &RevId,
    b: &RevId,
) -> Result<Option<RevId>, GitError> {
    let a = commit(store, a)?;
    let b = commit(store, b)?;
    let output = store.output(store.command(["merge-base", a, b]), "merge-base")?;
    if output.status.code() == Some(1) && output.stdout.is_empty() {
        return Ok(None);
    }
    let out = success("merge-base", output)?;
    id_text(store, out.trim_ascii_end(), "merge-base").map(|id| Some(RevId(id)))
}
