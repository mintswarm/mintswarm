//! Commit records for `log` and `read_commit`: the raw commits from one `git cat-file --batch`,
//! and their first-parent changes from one `git diff-tree --stdin`, whatever the number of
//! commits.

use mintswarm_domain::ids::RevId;
use mintswarm_domain::time::format_utc_seconds;
use mintswarm_domain::vcs::{CommitInfo, FileChange, Identity};

use super::diff::{DIFF_OPTIONS, PINNED_OPTIONS, raw_changes, z_records};
use super::env::success;
use super::read::id_text;
use super::snapshot::Batch;
use super::{GitError, GitStore};

/// The commits `ids`, full ids of commits in the store, in their order.
pub(super) fn read(store: &GitStore, ids: &[RevId]) -> Result<Vec<CommitInfo>, GitError> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let mut batch = Batch::start(store)?;
    let raw = ids
        .iter()
        .map(|id| batch.object(&id.0, "commit"))
        .collect::<Result<Vec<_>, _>>()?;
    batch.finish()?;
    let mut commits = ids
        .iter()
        .zip(&raw)
        .map(|(id, raw)| parse_commit(store, id, raw))
        .collect::<Result<Vec<_>, _>>()?;
    let changes = first_parent_changes(store, &commits)?;
    for (commit, changes) in commits.iter_mut().zip(changes) {
        commit.changes = changes;
    }
    Ok(commits)
}

fn parse(detail: String) -> GitError {
    GitError::Parse {
        command: "cat-file".to_owned(),
        detail,
    }
}

/// A raw commit object as a record without its changes: the `parent` lines and the first
/// `author` line of the header, and the message after the header's blank line, byte for byte.
fn parse_commit(store: &GitStore, id: &RevId, raw: &[u8]) -> Result<CommitInfo, GitError> {
    let (header, message) = match raw.windows(2).position(|pair| pair == b"\n\n") {
        Some(at) => (&raw[..at], &raw[at + 2..]),
        None => (raw, &b""[..]),
    };
    let mut parents = Vec::new();
    let mut author = None;
    for line in header.split(|&byte| byte == b'\n') {
        if let Some(parent) = line.strip_prefix(b"parent ") {
            parents.push(RevId(id_text(store, parent, "cat-file")?));
        } else if let Some(line) = line.strip_prefix(b"author ") {
            author.get_or_insert(line);
        }
    }
    let (name, email, epoch) = author
        .and_then(parse_author)
        .ok_or_else(|| parse(format!("commit {} without a valid author line", id.0)))?;
    let text = |bytes: &[u8]| String::from_utf8_lossy(bytes).into_owned();
    Ok(CommitInfo {
        id: id.clone(),
        parents,
        author: Identity {
            name: text(name),
            email: text(email),
        },
        authored_at: format_utc_seconds(epoch),
        message: text(message),
        changes: Vec::new(),
    })
}

/// An author line after `author `: the name, ` <email>`, a decimal epoch and a `±hhmm` offset,
/// with no `<` or `>` in the email.
fn parse_author(line: &[u8]) -> Option<(&[u8], &[u8], u64)> {
    let (rest, offset) = split_last_space(line)?;
    let valid_offset = offset.len() == 5
        && matches!(offset[0], b'+' | b'-')
        && offset[1..].iter().all(u8::is_ascii_digit);
    let (rest, epoch) = split_last_space(rest)?;
    if !valid_offset || epoch.is_empty() || !epoch.iter().all(u8::is_ascii_digit) {
        return None;
    }
    let epoch = std::str::from_utf8(epoch).ok()?.parse::<u64>().ok()?;
    let rest = rest.strip_suffix(b">")?;
    let open = rest.iter().rposition(|&byte| byte == b'<')?;
    let (name, email) = (rest[..open].strip_suffix(b" ")?, &rest[open + 1..]);
    if email.contains(&b'>') {
        return None;
    }
    Some((name, email, epoch))
}

fn split_last_space(bytes: &[u8]) -> Option<(&[u8], &[u8])> {
    let at = bytes.iter().rposition(|&byte| byte == b' ')?;
    Some((&bytes[..at], &bytes[at + 1..]))
}

/// Each commit's changes against its first parent, or against the empty tree for a root commit.
fn first_parent_changes(
    store: &GitStore,
    commits: &[CommitInfo],
) -> Result<Vec<Vec<FileChange>>, GitError> {
    const COMMAND: &str = "diff-tree";
    let mut input = Vec::new();
    for commit in commits {
        input.extend_from_slice(commit.id.0.as_bytes());
        if let Some(parent) = commit.parents.first() {
            input.push(b' ');
            input.extend_from_slice(parent.0.as_bytes());
        }
        input.push(b'\n');
    }
    let args = [COMMAND, "--stdin", "--root", "-r", "--raw"]
        .into_iter()
        .chain(DIFF_OPTIONS)
        .chain(PINNED_OPTIONS);
    let out = success(
        COMMAND,
        store.output_with_input(store.command(args), COMMAND, &input)?,
    )?;

    // The stream is an id record for each commit with changes, in input order, then a header
    // and a path record for each change; a commit without changes has no id record.
    let bad = |detail: &str| GitError::Parse {
        command: COMMAND.to_owned(),
        detail: detail.to_owned(),
    };
    let records: Vec<&[u8]> = z_records(&out).collect();
    let mut grouped: Vec<Vec<&[u8]>> = vec![Vec::new(); commits.len()];
    let mut current = None;
    let mut next = 0;
    let mut at = 0;
    while at < records.len() {
        let record = records[at];
        if record.starts_with(b":") {
            let commit = current.ok_or_else(|| bad("a change before any commit id"))?;
            let path = records
                .get(at + 1)
                .ok_or_else(|| bad("a --raw header without a path"))?;
            let group: &mut Vec<&[u8]> = &mut grouped[commit];
            group.extend([record, *path]);
            at += 2;
        } else {
            let id = id_text(store, record, COMMAND)?;
            let found = commits[next..]
                .iter()
                .position(|commit| commit.id.0 == id)
                .ok_or_else(|| bad("a commit id out of the input's order"))?;
            current = Some(next + found);
            next += found + 1;
            at += 1;
        }
    }
    grouped
        .iter()
        .map(|records| raw_changes(store, COMMAND, records))
        .collect()
}
