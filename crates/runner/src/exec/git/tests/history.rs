//! The history operations: path-limited diffs, merge bases, ancestry and commit records.

use std::collections::BTreeSet;

use mintswarm_domain::vcs::{ChangeKind, DiffOpts, RepoPath, Vcs};

use super::super::GitError;
use super::{Fixture, path};

/// Every form of the diff.
fn every_form() -> DiffOpts {
    DiffOpts {
        numstat: true,
        added: true,
        raw: true,
        patch: true,
    }
}

/// The paths a unified diff names in its `diff --git a/<path> b/<path>` lines.
fn patch_paths(patch: &[u8]) -> BTreeSet<RepoPath> {
    patch
        .split(|&byte| byte == b'\n')
        .filter_map(|line| line.strip_prefix(b"diff --git a/"))
        .map(|rest| {
            let half = rest.len().saturating_sub(3) / 2;
            path(&rest[..half])
        })
        .collect()
}

#[test]
fn diff_limits_to_paths() {
    let fx = Fixture::new();
    let store = fx.fetched_store("diff-paths");
    let limited = |paths: &[RepoPath]| store.diff(&fx.base, &fx.side, every_form(), paths);

    let dir = limited(&[path(b"dir")]).unwrap();
    let files: Vec<(&RepoPath, ChangeKind, Option<(u64, u64)>)> = dir
        .files
        .iter()
        .map(|file| (&file.path, file.kind, file.lines))
        .collect();
    assert_eq!(
        files,
        [(&path(b"dir/deep.txt"), ChangeKind::Added, Some((1, 0)))]
    );
    assert_eq!(
        patch_paths(dir.patch.as_deref().unwrap()),
        BTreeSet::from([path(b"dir/deep.txt")])
    );

    let two = limited(&[path(b"keep.txt"), path(b"gone.txt")]).unwrap();
    let named: BTreeSet<RepoPath> = two.files.iter().map(|file| file.path.clone()).collect();
    assert_eq!(
        named,
        BTreeSet::from([path(b"keep.txt"), path(b"gone.txt")])
    );
    assert_eq!(patch_paths(two.patch.as_deref().unwrap()), named);

    // A path is a whole component, and no character is a wildcard.
    for paths in [&[path(b"di")][..], &[path(b"*")], &[path(b"*.txt")]] {
        let none = limited(paths).unwrap();
        assert!(none.files.is_empty());
        assert_eq!(none.patch.as_deref(), Some(&b""[..]));
    }
    // The whole tree without paths.
    assert_eq!(limited(&[]).unwrap().files.len(), 10);

    for refused in [path(b""), path(b"a\0b")] {
        assert!(matches!(
            limited(&[path(b"keep.txt"), refused]),
            Err(GitError::Parse { .. })
        ));
    }
}
