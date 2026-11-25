//! The history operations: path-limited diffs, merge bases, ancestry and commit records.

use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::path::PathBuf;

use mintswarm_domain::ids::RevId;
use mintswarm_domain::vcs::{ChangeKind, DiffOpts, RefOrId, Remote, RepoPath, Vcs};
use tempfile::TempDir;

use super::super::{GitError, GitStore};
use super::{Fixture, GITLINK, fixture_command, fixture_git, git_probe, path, run_fixture, text};

/// A bare source repository built with plumbing, and stores fetched from it.
struct History {
    dir: TempDir,
    source: PathBuf,
}

impl History {
    fn new() -> History {
        let dir = TempDir::new().unwrap();
        let source = dir.path().join("source.git");
        run_fixture(
            fixture_command(None),
            &[
                OsStr::new("init"),
                OsStr::new("--bare"),
                OsStr::new("--quiet"),
                source.as_os_str(),
            ],
            b"",
        );
        History { dir, source }
    }

    /// Fixture git on the source with `stdin`.
    fn git(&self, args: &[&str], stdin: &[u8]) -> String {
        text(&run_fixture(
            fixture_command(Some(&self.source)),
            args,
            stdin,
        ))
    }

    fn blob(&self, data: &[u8]) -> String {
        self.git(&["hash-object", "-w", "--stdin"], data)
    }

    /// A tree of regular files, by name and content.
    fn tree(&self, files: &[(&str, &[u8])]) -> String {
        let mut input = Vec::new();
        for (name, data) in files {
            input.extend_from_slice(format!("100644 blob {}\t{name}", self.blob(data)).as_bytes());
            input.push(0);
        }
        self.git(&["mktree", "-z"], &input)
    }

    /// A commit of `tree` on `parents` with `message` as given, authored at `author` and committed
    /// at `committer`, each `<epoch> <offset>`.
    fn commit_at(
        &self,
        tree: &str,
        parents: &[&RevId],
        message: &[u8],
        author: &str,
        committer: &str,
    ) -> RevId {
        let mut command = fixture_command(Some(&self.source));
        command
            .env("GIT_AUTHOR_DATE", author)
            .env("GIT_COMMITTER_DATE", committer);
        let mut args = vec!["commit-tree".to_owned(), tree.to_owned()];
        for parent in parents {
            args.extend(["-p".to_owned(), parent.0.clone()]);
        }
        RevId(text(&run_fixture(command, &args, message)))
    }

    /// [`Self::commit_at`] with both dates `time` in UTC.
    fn commit(&self, tree: &str, parents: &[&RevId], message: &str, time: u64) -> RevId {
        let date = format!("{time} +0000");
        self.commit_at(tree, parents, message.as_bytes(), &date, &date)
    }

    /// A store with the commits `tips` fetched from the source.
    fn store(&self, name: &str, tips: &[&RevId]) -> GitStore {
        let mut wants = Vec::new();
        for (at, tip) in tips.iter().enumerate() {
            let branch = format!("refs/heads/t{at}");
            fixture_git(&self.source, &["update-ref", &branch, &tip.0]);
            wants.push(RefOrId::Ref(branch));
        }
        let path = super::super::store_path(&self.dir.path().join("state"), name).unwrap();
        let mut store = GitStore::create(&path, &git_probe(Some("2.45.0"))).unwrap();
        store.allow_file_remote();
        let remote = Remote {
            name: "origin".to_owned(),
            url: format!("file://{}", self.source.display()),
        };
        store.fetch(&remote, &wants, None).unwrap();
        store
    }
}

/// Fixture git's ids for `args` on `store`, one a line.
fn git_ids(store: &GitStore, args: &[&str]) -> Vec<RevId> {
    text(&fixture_git(store.path(), args))
        .lines()
        .map(|line| RevId(line.to_owned()))
        .collect()
}

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
    let [deep] = dir.files.as_slice() else {
        panic!("one change expected, not {}", dir.files.len());
    };
    assert_eq!(
        (&deep.path, deep.kind, deep.lines),
        (&path(b"dir/deep.txt"), ChangeKind::Added, Some((1, 0)))
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

#[test]
fn merge_bases_lists_every_base() {
    let history = History::new();
    let root = history.commit(&history.tree(&[("f", b"0\n")]), &[], "root", 1_700_000_000);
    let a1 = history.commit(
        &history.tree(&[("f", b"0\n"), ("a", b"a\n")]),
        &[&root],
        "a1",
        1_700_000_100,
    );
    let b1 = history.commit(
        &history.tree(&[("f", b"0\n"), ("b", b"b\n")]),
        &[&root],
        "b1",
        1_700_000_200,
    );
    let both = history.tree(&[("f", b"0\n"), ("a", b"a\n"), ("b", b"b\n")]);
    // A criss-cross: each side merges the other's first commit.
    let a2 = history.commit(&both, &[&a1, &b1], "a2", 1_700_000_300);
    let b2 = history.commit(&both, &[&b1, &a1], "b2", 1_700_000_400);
    let unrelated = history.commit(
        &history.tree(&[("u", b"u\n")]),
        &[],
        "unrelated",
        1_700_000_500,
    );
    let store = history.store("bases", &[&a2, &b2, &unrelated]);

    let bases: BTreeSet<RevId> = store.merge_bases(&a2, &b2).unwrap().into_iter().collect();
    assert_eq!(bases, BTreeSet::from([a1.clone(), b1.clone()]));
    let git: BTreeSet<RevId> = git_ids(&store, &["merge-base", "--all", &a2.0, &b2.0])
        .into_iter()
        .collect();
    assert_eq!(bases, git);
    assert_eq!(
        store.merge_bases(&a1, &b1).unwrap(),
        std::slice::from_ref(&root)
    );
    assert!(store.merge_bases(&a2, &unrelated).unwrap().is_empty());

    assert!(store.is_ancestor(&a2, &a2).unwrap());
    assert!(store.is_ancestor(&root, &a2).unwrap());
    assert!(store.is_ancestor(&b1, &a2).unwrap());
    assert!(!store.is_ancestor(&a2, &root).unwrap());
    assert!(!store.is_ancestor(&a2, &b2).unwrap());
    assert!(!store.is_ancestor(&unrelated, &a2).unwrap());

    let syntax = RevId(format!("{}^", a2.0));
    let missing = RevId(GITLINK.to_owned());
    assert!(matches!(
        store.merge_bases(&syntax, &b2),
        Err(GitError::RevisionSyntax(_))
    ));
    assert!(matches!(
        store.merge_bases(&a2, &missing),
        Err(GitError::NotFound(_))
    ));
    assert!(matches!(
        store.is_ancestor(&syntax, &b2),
        Err(GitError::RevisionSyntax(_))
    ));
    assert!(matches!(
        store.is_ancestor(&a2, &missing),
        Err(GitError::NotFound(_))
    ));
}
