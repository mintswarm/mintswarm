//! `diff` between two commits' trees, with the landing floor's options.

use std::collections::{BTreeMap, BTreeSet};

use mintswarm_domain::ids::RevId;
use mintswarm_domain::vcs::{ChangeKind, ChangeSet, DiffOpts, FileChange, RepoPath};

use super::read::{id_text, resolve};
use super::{GitError, GitStore};

/// The landing floor's diff options (`landing/floor.py` `DIFF_OPTIONS`), exactly.
pub const DIFF_OPTIONS: [&str; 8] = [
    "--no-renames",
    "-z",
    "--ignore-submodules=none",
    "--diff-algorithm=myers",
    "--no-ext-diff",
    "--no-textconv",
    "--no-relative",
    "--no-color",
];

/// What the store's config could otherwise change in any form of the output (`msw-vcs`).
pub const PINNED_OPTIONS: [&str; 3] = ["--full-index", "--no-abbrev", "-O/dev/null"];

/// The same for the patch only (`msw-vcs`).
pub const PATCH_OPTIONS: [&str; 7] = [
    "--patch",
    "--src-prefix=a/",
    "--dst-prefix=b/",
    "--unified=3",
    "--inter-hunk-context=0",
    "--indent-heuristic",
    "--submodule=short",
];

/// The changes from `base`'s tree to `head`'s.
pub(super) fn diff(
    store: &GitStore,
    base: &RevId,
    head: &RevId,
    opts: &DiffOpts,
) -> Result<ChangeSet, GitError> {
    let base = resolve(store, store.checked_id(base)?)?.0;
    let head = resolve(store, store.checked_id(head)?)?.0;
    let run = |form: &[&str], options: &[&str]| {
        let args = std::iter::once("diff")
            .chain(form.iter().copied())
            .chain(options.iter().copied())
            .chain(PINNED_OPTIONS)
            .chain([base.as_str(), head.as_str()]);
        store.git(args)
    };

    let raw = run(&["--raw", "--no-abbrev"], &DIFF_OPTIONS)?;
    let mut files = raw_changes(store, "diff", &z_records(&raw).collect::<Vec<_>>())?;
    if !opts.raw {
        for file in &mut files {
            file.old_mode = None;
            file.new_mode = None;
            file.old_blob = None;
            file.new_blob = None;
        }
    }
    if opts.numstat {
        let mut lines = numstat(&run(&["--numstat"], &DIFF_OPTIONS)?)?;
        let paths: BTreeSet<&RepoPath> = files.iter().map(|file| &file.path).collect();
        if !lines.keys().eq(paths.iter().copied()) {
            return Err(parse("--numstat and --raw disagree on the changed paths"));
        }
        for file in &mut files {
            file.lines = lines.remove(&file.path).flatten();
        }
    }
    if opts.added {
        let out = run(&["--name-only", "--diff-filter=A"], &DIFF_OPTIONS)?;
        let named: BTreeSet<RepoPath> = z_records(&out)
            .map(|path| RepoPath(path.to_vec()))
            .collect();
        let added: BTreeSet<RepoPath> = files
            .iter()
            .filter(|file| file.kind == ChangeKind::Added)
            .map(|file| file.path.clone())
            .collect();
        if named != added {
            return Err(parse(
                "--diff-filter=A and --raw disagree on the added paths",
            ));
        }
    }
    let patch = if opts.patch {
        let options: Vec<&str> = DIFF_OPTIONS
            .into_iter()
            .filter(|option| *option != "-z")
            .collect();
        Some(run(&PATCH_OPTIONS, &options)?)
    } else {
        None
    };
    Ok(ChangeSet { files, patch })
}

fn parse(detail: &str) -> GitError {
    parse_in("diff", detail)
}

fn parse_in(command: &str, detail: &str) -> GitError {
    GitError::Parse {
        command: command.to_owned(),
        detail: detail.to_owned(),
    }
}

/// The NUL-terminated records of `-z` output.
pub(super) fn z_records(out: &[u8]) -> impl Iterator<Item = &[u8]> {
    let out = out.strip_suffix(b"\0").unwrap_or(out);
    out.split(|&byte| byte == 0).filter(|_| !out.is_empty())
}

/// The changes the `--raw -z` records of `command` list: a header record then a path record for
/// each, with the modes and blob ids always set where a side exists.
pub(super) fn raw_changes(
    store: &GitStore,
    command: &str,
    records: &[&[u8]],
) -> Result<Vec<FileChange>, GitError> {
    if records.len() % 2 != 0 {
        return Err(parse_in(command, "a --raw header without a path"));
    }
    records
        .chunks(2)
        .map(|pair| {
            let (header, path) = (pair[0], pair[1]);
            let bad = || {
                parse_in(
                    command,
                    &format!("--raw header {:?}", String::from_utf8_lossy(header)),
                )
            };
            let fields: Vec<&[u8]> = header
                .strip_prefix(b":")
                .ok_or_else(bad)?
                .split(|&byte| byte == b' ')
                .collect();
            let [old_mode, new_mode, old_blob, new_blob, status] = fields.as_slice() else {
                return Err(bad());
            };
            let mode = |field: &[u8]| {
                std::str::from_utf8(field)
                    .ok()
                    .filter(|text| text.len() == 6)
                    .and_then(|text| u32::from_str_radix(text, 8).ok())
                    .ok_or_else(bad)
            };
            let kind = match *status {
                b"A" => ChangeKind::Added,
                b"M" => ChangeKind::Modified,
                b"D" => ChangeKind::Deleted,
                b"T" => ChangeKind::TypeChanged,
                _ => return Err(bad()),
            };
            if path.is_empty() {
                return Err(bad());
            }
            let present = |mode: u32| (mode != 0).then_some(mode);
            let blob = |field: &[u8]| {
                if field.len() == store.id_len && field.iter().all(|&byte| byte == b'0') {
                    Ok(None)
                } else {
                    id_text(store, field, command).map(|id| Some(RevId(id)))
                }
            };
            Ok(FileChange {
                path: RepoPath(path.to_vec()),
                old_path: None,
                kind,
                old_mode: present(mode(old_mode)?),
                new_mode: present(mode(new_mode)?),
                old_blob: blob(old_blob)?,
                new_blob: blob(new_blob)?,
                lines: None,
            })
        })
        .collect()
}

/// `--numstat -z` records by path: the added and deleted lines, `None` for a binary file.
fn numstat(out: &[u8]) -> Result<BTreeMap<RepoPath, Option<(u64, u64)>>, GitError> {
    let mut lines = BTreeMap::new();
    for record in z_records(out) {
        let bad = || {
            parse(&format!(
                "--numstat record {:?}",
                String::from_utf8_lossy(record)
            ))
        };
        let mut parts = record.splitn(3, |&byte| byte == b'\t');
        let (Some(added), Some(deleted), Some(path)) = (parts.next(), parts.next(), parts.next())
        else {
            return Err(bad());
        };
        if path.is_empty() {
            return Err(bad());
        }
        let count = |field: &[u8]| {
            std::str::from_utf8(field)
                .ok()
                .filter(|text| !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit()))
                .and_then(|text| text.parse::<u64>().ok())
        };
        let counts = match (added, deleted) {
            (b"-", b"-") => None,
            _ => Some((
                count(added).ok_or_else(bad)?,
                count(deleted).ok_or_else(bad)?,
            )),
        };
        if lines.insert(RepoPath(path.to_vec()), counts).is_some() {
            return Err(bad());
        }
    }
    Ok(lines)
}
