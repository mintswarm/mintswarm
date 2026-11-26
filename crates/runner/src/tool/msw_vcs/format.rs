//! The output forms of `msw-vcs`, written from the domain's records alone: the `log` record and
//! the `show` commit, the tree listing, and the `--numstat`, `--name-only` and `--raw` forms of
//! `diff`. The patch is the VCS's own bytes.

use mintswarm_domain::vcs::{ChangeKind, CommitInfo, FileChange, TreeEntry};

use super::args::quote;

/// The mode of a gitlink.
pub const GITLINK: u32 = 0o160000;

/// A change's status letter.
fn status(kind: ChangeKind) -> &'static [u8] {
    match kind {
        ChangeKind::Added => b"A",
        ChangeKind::Deleted => b"D",
        ChangeKind::Modified => b"M",
        ChangeKind::TypeChanged => b"T",
        ChangeKind::Renamed => b"R",
        ChangeKind::Copied => b"C",
    }
}

/// One commit's `log` record: its header lines, then a `path` line per change, sorted by path
/// bytes.
pub fn record(info: &CommitInfo) -> Vec<u8> {
    let mut out = Vec::new();
    let mut line = |parts: &[&[u8]]| {
        for part in parts {
            out.extend_from_slice(part);
        }
        out.push(b'\n');
    };
    line(&[b"commit ", info.id.0.as_bytes()]);
    for parent in &info.parents {
        line(&[b"parent ", parent.0.as_bytes()]);
    }
    line(&[
        b"author ",
        info.author.name.as_bytes(),
        b" <",
        info.author.email.as_bytes(),
        b">",
    ]);
    line(&[b"date ", info.authored_at.as_bytes()]);
    let subject = info.message.split('\n').next().unwrap_or_default();
    line(&[b"subject ", subject.as_bytes()]);
    let mut changes: Vec<&FileChange> = info.changes.iter().collect();
    changes.sort_by(|a, b| a.path.cmp(&b.path));
    for change in changes {
        line(&[b"path ", status(change.kind), b" ", &quote(&change.path.0)]);
    }
    out
}

/// The `log` output: the records separated by one blank line.
pub fn log(commits: &[CommitInfo]) -> Vec<u8> {
    commits.iter().map(record).collect::<Vec<_>>().join(&b'\n')
}

/// `show REV`: the commit's record, then, when the message is not empty, a blank line and the
/// message with a final newline removed, each non-empty line indented by four spaces.
pub fn commit(info: &CommitInfo) -> Vec<u8> {
    let mut out = record(info);
    let message = info.message.strip_suffix('\n').unwrap_or(&info.message);
    if !message.is_empty() {
        out.push(b'\n');
        for line in message.split('\n') {
            if !line.is_empty() {
                out.extend_from_slice(b"    ");
                out.extend_from_slice(line.as_bytes());
            }
            out.push(b'\n');
        }
    }
    out
}

/// One tree listing line: `<mode> <id>\t<path>`.
fn tree_line(out: &mut Vec<u8>, entry: &TreeEntry) {
    out.extend_from_slice(format!("{:06o} {}\t", entry.mode, entry.blob.0).as_bytes());
    out.extend(quote(&entry.path.0));
    out.push(b'\n');
}

/// The tree listing of `entries`, sorted by path bytes.
pub fn tree<'a>(entries: impl IntoIterator<Item = &'a TreeEntry>) -> Vec<u8> {
    let mut entries: Vec<&TreeEntry> = entries.into_iter().collect();
    entries.sort_by(|a, b| a.path.cmp(&b.path));
    let mut out = Vec::new();
    for entry in entries {
        tree_line(&mut out, entry);
    }
    out
}

/// `diff --numstat`: `<added>\t<deleted>\t<path>`, or `-\t-\t<path>` for a binary file.
pub fn numstat(files: &[FileChange]) -> Vec<u8> {
    let mut out = Vec::new();
    for file in files {
        match file.lines {
            Some((added, deleted)) => out.extend(format!("{added}\t{deleted}\t").as_bytes()),
            None => out.extend_from_slice(b"-\t-\t"),
        }
        out.extend(quote(&file.path.0));
        out.push(b'\n');
    }
    out
}

/// `diff --name-only`: one path per line.
pub fn name_only(files: &[FileChange]) -> Vec<u8> {
    let mut out = Vec::new();
    for file in files {
        out.extend(quote(&file.path.0));
        out.push(b'\n');
    }
    out
}

/// `diff --raw`: `:<old mode> <new mode> <old id> <new id> <status>\t<path>`, with `000000`
/// and an all-zero id of the other side's length for a missing side.
pub fn raw(files: &[FileChange]) -> Vec<u8> {
    let mut out = Vec::new();
    for file in files {
        let mode = |mode: Option<u32>| format!("{:06o}", mode.unwrap_or(0));
        let length = file
            .old_blob
            .as_ref()
            .or(file.new_blob.as_ref())
            .map_or(40, |id| id.0.len());
        let id = |id: Option<&_>| {
            id.map_or_else(
                || "0".repeat(length),
                |id: &mintswarm_domain::ids::RevId| id.0.clone(),
            )
        };
        out.extend(
            format!(
                ":{} {} {} {} ",
                mode(file.old_mode),
                mode(file.new_mode),
                id(file.old_blob.as_ref()),
                id(file.new_blob.as_ref()),
            )
            .as_bytes(),
        );
        out.extend_from_slice(status(file.kind));
        out.push(b'\t');
        out.extend(quote(&file.path.0));
        out.push(b'\n');
    }
    out
}

/// A gitlink listed alone, for `show REV:PATH` naming one.
pub fn gitlink(entry: &TreeEntry) -> Vec<u8> {
    let mut out = Vec::new();
    tree_line(&mut out, entry);
    out
}
