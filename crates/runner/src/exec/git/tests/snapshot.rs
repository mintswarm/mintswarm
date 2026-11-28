//! Read-only snapshots and the runner's cache of them.

use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

use mintswarm_domain::ids::{RepoKey, RevId};
use mintswarm_domain::vcs::{RefOrId, TreeMode, Vcs, Workspace};

use super::super::{GitError, GitStore, Snapshots, snapshots_path};
use super::{Fixture, commit_with, fixture_git, run_fixture, text};

/// Every entry under `root`, relative to it, with its metadata, never following a link.
fn walk(root: &Path) -> Vec<(Vec<u8>, std::fs::Metadata)> {
    let mut out = Vec::new();
    let mut pending = vec![PathBuf::new()];
    while let Some(relative) = pending.pop() {
        for entry in std::fs::read_dir(root.join(&relative)).unwrap() {
            let entry = entry.unwrap();
            let path = relative.join(entry.file_name());
            let meta = std::fs::symlink_metadata(root.join(&path)).unwrap();
            if meta.is_dir() {
                pending.push(path.clone());
            }
            out.push((path.as_os_str().as_bytes().to_vec(), meta));
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

fn mode(meta: &std::fs::Metadata) -> u32 {
    meta.permissions().mode() & 0o7777
}

/// `side`'s tree with its non-UTF-8 name made UTF-8, for file systems (APFS) that refuse it,
/// committed and fetched into `store`: nested directories, an executable, two symlinks and a
/// gitlink.
fn portable_side(fx: &Fixture, store: &GitStore) -> RevId {
    let listing = fixture_git(&fx.source, &["ls-tree", "-z", &fx.side.0]);
    let mut input = Vec::new();
    for record in listing.split(|&byte| byte == 0).filter(|r| !r.is_empty()) {
        let tab = record.iter().position(|&byte| byte == b'\t').unwrap();
        let name = &record[tab + 1..];
        let name: &[u8] = if name == b"odd\xff\nname" {
            b"odd\nname"
        } else {
            name
        };
        input.extend_from_slice(&record[..=tab]);
        input.extend_from_slice(name);
        input.push(0);
    }
    let tree = text(&run_fixture(
        super::fixture_command(Some(&fx.source)),
        &["mktree", "-z", "--missing"],
        &input,
    ));
    hostile(fx, store, "portable", &tree)
}

/// `EILSEQ`, which a file system that takes only UTF-8 names (APFS) gives for any other name.
#[cfg(target_os = "macos")]
const ILLEGAL_BYTE_SEQUENCE: i32 = 92;
/// `EILSEQ`, which a file system that takes only UTF-8 names gives for any other name.
#[cfg(not(target_os = "macos"))]
const ILLEGAL_BYTE_SEQUENCE: i32 = 84;

/// Whether `error` is the file system refusing a name, by its error class.
fn name_refused(error: &GitError) -> bool {
    match error {
        GitError::Io { source, .. } => {
            source.kind() == std::io::ErrorKind::InvalidFilename
                || source.raw_os_error() == Some(ILLEGAL_BYTE_SEQUENCE)
        }
        _ => false,
    }
}

/// `side` materialised read-only at `dest` with its non-UTF-8 name as is; where the file system
/// refuses that name, and only there, `portable_side` instead. On Linux the name must be taken.
fn materialise_side(fx: &Fixture, store: &GitStore, dest: &Path) -> (RevId, Workspace) {
    match store.materialise(&fx.side, dest, TreeMode::ReadOnly) {
        Ok(workspace) => (fx.side.clone(), workspace),
        Err(error) if !cfg!(target_os = "linux") && name_refused(&error) => {
            assert!(!dest.exists(), "a refused snapshot left {dest:?}");
            let side = portable_side(fx, store);
            let workspace = store.materialise(&side, dest, TreeMode::ReadOnly).unwrap();
            (side, workspace)
        }
        Err(error) => panic!("materialising side: {error:?}"),
    }
}

#[test]
fn read_only_snapshot_matches_the_commit() {
    let fx = Fixture::new();
    let store = fx.fetched_store("snapshot");
    let dest = fx.dir.path().join("snapshot");
    let (side, workspace) = materialise_side(&fx, &store, &dest);
    if side == fx.side {
        assert!(
            walk(&dest)
                .iter()
                .any(|(path, _)| path.as_slice() == b"odd\xff\nname"),
            "the non-UTF-8 name is missing"
        );
    }
    assert_eq!(
        workspace,
        Workspace {
            path: dest.clone(),
            parent: side.clone(),
            mode: TreeMode::ReadOnly,
        }
    );
    assert_eq!(mode(&std::fs::symlink_metadata(&dest).unwrap()), 0o555);

    let gitlinks: Vec<_> = store
        .list_tree(&side)
        .unwrap()
        .into_iter()
        .filter(|entry| entry.mode == 0o160_000)
        .collect();
    assert_eq!(gitlinks.len(), 1);
    let mut index = Vec::new();
    let mut kinds = (0, 0, 0, 0);
    for (path, meta) in walk(&dest) {
        let name = Path::new(OsStr::from_bytes(&path)).file_name().unwrap();
        assert!(
            !name.as_bytes().eq_ignore_ascii_case(b".git"),
            "a .git entry at {path:?}"
        );
        let full = dest.join(OsStr::from_bytes(&path));
        let (entry_mode, data) = if meta.file_type().is_symlink() {
            kinds.2 += 1;
            let target = std::fs::read_link(&full).unwrap();
            ("120000", target.as_os_str().as_bytes().to_vec())
        } else if meta.is_dir() {
            assert_eq!(mode(&meta), 0o555, "{path:?}");
            if let Some(gitlink) = gitlinks.iter().find(|entry| entry.path.0 == path) {
                kinds.3 += 1;
                assert_eq!(std::fs::read_dir(&full).unwrap().count(), 0);
                index.extend_from_slice(format!("160000 {}\t", gitlink.blob.0).as_bytes());
                index.extend_from_slice(&path);
                index.push(0);
            }
            continue;
        } else {
            let executable = mode(&meta) == 0o555;
            assert!(
                executable || mode(&meta) == 0o444,
                "{path:?} has mode {:o}",
                mode(&meta)
            );
            if executable {
                kinds.1 += 1;
            } else {
                kinds.0 += 1;
            }
            let entry_mode = if executable { "100755" } else { "100644" };
            (entry_mode, std::fs::read(&full).unwrap())
        };
        let id = text(&run_fixture(
            super::fixture_command(Some(&fx.source)),
            &["hash-object", "-w", "--stdin"],
            &data,
        ));
        index.extend_from_slice(format!("{entry_mode} {id}\t").as_bytes());
        index.extend_from_slice(&path);
        index.push(0);
    }
    // Regular files, an executable, two symlinks, a gitlink.
    assert!(kinds.0 > 3, "{kinds:?}");
    assert_eq!((kinds.1, kinds.2, kinds.3), (1, 2, 1));

    let index_file = fx.dir.path().join("rebuilt-index");
    let mut command = super::fixture_command(Some(&fx.source));
    command.env("GIT_INDEX_FILE", &index_file);
    run_fixture(command, &["update-index", "-z", "--index-info"], &index);
    let mut command = super::fixture_command(Some(&fx.source));
    command.env("GIT_INDEX_FILE", &index_file);
    let tree = text(&run_fixture(command, &["write-tree"], b""));
    let expected = text(&fixture_git(
        &fx.source,
        &["rev-parse", &format!("{}^{{tree}}", side.0)],
    ));
    assert_eq!(tree, expected);

    assert!(matches!(
        store.materialise(&fx.main, &dest, TreeMode::ReadOnly),
        Err(GitError::Exists(_))
    ));
    let other = fx.dir.path().join("writable");
    assert!(matches!(
        store.materialise(&side, &other, TreeMode::Writable),
        Err(GitError::Unsupported(_))
    ));
    assert!(!other.exists());
}

/// A root commit of `tree` in the fixture's source, fetched into `store` under
/// `refs/heads/<branch>`.
fn hostile(fx: &Fixture, store: &GitStore, branch: &str, tree: &str) -> RevId {
    let commit = commit_with(&fx.source, tree, &[], b"hostile\n", None, 1_700_001_000);
    fixture_git(
        &fx.source,
        &["update-ref", &format!("refs/heads/{branch}"), &commit.0],
    );
    store
        .fetch(
            &fx.remote(),
            &[RefOrId::Ref(format!("refs/heads/{branch}"))],
            None,
        )
        .unwrap();
    commit
}

/// A tree of `entries`, each `(mode, type, id, name)`, built with `mktree` in the source.
fn mktree(fx: &Fixture, entries: &[(&str, &str, &str, &[u8])]) -> String {
    let mut input = Vec::new();
    for (mode, kind, id, name) in entries {
        input.extend_from_slice(format!("{mode} {kind} {id}\t").as_bytes());
        input.extend_from_slice(name);
        input.push(0);
    }
    text(&run_fixture(
        super::fixture_command(Some(&fx.source)),
        &["mktree", "-z", "--missing"],
        &input,
    ))
}

#[test]
fn hostile_trees_are_refused() {
    let fx = Fixture::new();
    let store = fx.store("hostile-trees");
    let outside = fx.dir.path().join("outside");
    std::fs::create_dir(&outside).unwrap();
    let blob = text(&run_fixture(
        super::fixture_command(Some(&fx.source)),
        &["hash-object", "-w", "--stdin"],
        b"x\n",
    ));
    let target = text(&run_fixture(
        super::fixture_command(Some(&fx.source)),
        &["hash-object", "-w", "--stdin"],
        outside.as_os_str().as_bytes(),
    ));
    let file = |name: &'static [u8]| ("100644", "blob", blob.as_str(), name);
    let inner = mktree(&fx, &[file(b"x")]);
    let dot = mktree(&fx, &[file(b".")]);
    let trees = [
        ("dotdot", mktree(&fx, &[file(b"..")])),
        (
            "dot",
            mktree(&fx, &[("040000", "tree", dot.as_str(), b"d")]),
        ),
        (
            "dotgit",
            mktree(&fx, &[("040000", "tree", inner.as_str(), b".GiT")]),
        ),
        ("dotgit-file", mktree(&fx, &[file(b".git")])),
        (
            "under-symlink",
            mktree(
                &fx,
                &[
                    ("120000", "blob", target.as_str(), b"a"),
                    ("040000", "tree", inner.as_str(), b"a"),
                ],
            ),
        ),
        (
            "under-file",
            mktree(&fx, &[file(b"a"), ("040000", "tree", inner.as_str(), b"a")]),
        ),
    ];
    let parent = fx.dir.path().join("snapshots");
    std::fs::create_dir(&parent).unwrap();
    for (name, tree) in &trees {
        let commit = hostile(&fx, &store, name, tree);
        let dest = parent.join(name);
        let result = store.materialise(&commit, &dest, TreeMode::ReadOnly);
        assert!(
            matches!(result, Err(GitError::UnsafePath(_))),
            "{name}: {result:?}"
        );
        assert!(!dest.exists(), "{name}");
        assert_eq!(std::fs::read_dir(&parent).unwrap().count(), 0, "{name}");
        assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 0, "{name}");
    }
}

#[test]
fn snapshots_are_kept_while_pinned_or_the_work_head() {
    let fx = Fixture::new();
    let store = fx.fetched_store("shared");
    let runner = fx.dir.path().join("state");
    let root = snapshots_path(&runner, &RepoKey::new("shared").unwrap());
    assert_eq!(root, runner.join("snapshots/shared"));

    // What an earlier run left: a read-only tree and a temporary.
    let leftover = root.join(&fx.main.0).join("dir");
    std::fs::create_dir_all(&leftover).unwrap();
    std::fs::write(leftover.join("file"), b"old").unwrap();
    std::fs::set_permissions(
        leftover.join("file"),
        std::fs::Permissions::from_mode(0o444),
    )
    .unwrap();
    std::fs::set_permissions(&leftover, std::fs::Permissions::from_mode(0o555)).unwrap();
    std::fs::create_dir(root.join(".tmp-x-1-0")).unwrap();
    let snapshots = Snapshots::open(&root).unwrap();
    assert_eq!(std::fs::read_dir(&root).unwrap().count(), 0);
    assert_eq!(
        mode(&std::fs::symlink_metadata(&root).unwrap()),
        0o700,
        "the root's mode"
    );

    // Two clones of one commit pin one snapshot; it is evicted when the last guard drops.
    let first = snapshots.snapshot(&store, &fx.orphan).unwrap();
    let second = snapshots.snapshot(&store, &fx.orphan).unwrap();
    assert_eq!(*first, *second);
    assert_eq!(first.path, root.join(&fx.orphan.0));
    assert_eq!(first.parent, fx.orphan);
    assert_eq!(first.mode, TreeMode::ReadOnly);
    let path = first.path.clone();
    let inode = std::fs::metadata(&path).unwrap().ino();
    assert_eq!(std::fs::metadata(&second.path).unwrap().ino(), inode);
    drop(first);
    assert!(path.is_dir());
    assert_eq!(std::fs::metadata(&path).unwrap().ino(), inode);
    drop(second);
    assert!(!path.exists());

    // The work head is kept with no pins, and evicted when the head moves away.
    snapshots.set_work_head(&store, &fx.orphan).unwrap();
    let head = snapshots.snapshot(&store, &fx.orphan).unwrap();
    let head_path = head.path.clone();
    drop(head);
    assert!(head_path.is_dir());
    snapshots.set_work_head(&store, &fx.orphan).unwrap();
    assert!(head_path.is_dir());

    // A work head that is not a full id of a commit in the store is refused, and changes nothing.
    let abbreviated = RevId(fx.main.0[..12].to_owned());
    for refused in [&abbreviated, &RevId("HEAD".to_owned())] {
        assert!(
            matches!(
                snapshots.set_work_head(&store, refused),
                Err(GitError::RevisionSyntax(_))
            ),
            "{}",
            refused.0
        );
    }
    assert!(head_path.is_dir());
    let probe = snapshots.snapshot(&store, &fx.orphan).unwrap();
    drop(probe);
    assert!(head_path.is_dir(), "the work head is still the orphan");

    snapshots.set_work_head(&store, &fx.main).unwrap();
    assert!(!head_path.exists());

    // A pinned commit survives a move of the head, and is evicted when its guard drops.
    let pinned = snapshots.snapshot(&store, &fx.main).unwrap();
    snapshots.set_work_head(&store, &fx.base).unwrap();
    let pinned_path = pinned.path.clone();
    assert!(pinned_path.is_dir());
    drop(pinned);
    assert!(!pinned_path.exists());
    assert_eq!(std::fs::read_dir(&root).unwrap().count(), 0);

    // Only commits of the store are snapshotted.
    assert!(
        snapshots
            .snapshot(&store, &RevId("HEAD".to_owned()))
            .is_err()
    );
}
