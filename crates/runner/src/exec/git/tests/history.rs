//! The history operations: path-limited diffs, merge bases, ancestry and commit records.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsStr;
use std::path::PathBuf;

use mintswarm_domain::ids::{RepoKey, RevId};
use mintswarm_domain::vcs::{
    ChangeKind, CommitInfo, DiffOpts, RefOrId, Remote, RepoPath, RevRange, Vcs,
};
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

    /// A tree of regular files, by path and content; a path with `/` is in a subtree.
    fn tree(&self, files: &[(&str, &[u8])]) -> String {
        let mut entries: BTreeMap<&str, Vec<(&str, &[u8])>> = BTreeMap::new();
        let mut input = Vec::new();
        for (name, data) in files {
            match name.split_once('/') {
                Some((dir, rest)) => entries.entry(dir).or_default().push((rest, data)),
                None => {
                    let line = format!("100644 blob {}\t{name}", self.blob(data));
                    input.extend_from_slice(line.as_bytes());
                    input.push(0);
                }
            }
        }
        for (dir, files) in entries {
            let line = format!("040000 tree {}\t{dir}", self.tree(&files));
            input.extend_from_slice(line.as_bytes());
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
        let key = RepoKey::new(name).unwrap();
        let path = super::super::store_path(&self.dir.path().join("state"), &key);
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

    for refused in [path(b""), path(b"a\0b"), path(b"dir/")] {
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

/// The ids of `commits`.
fn ids(commits: &[CommitInfo]) -> Vec<RevId> {
    commits.iter().map(|commit| commit.id.clone()).collect()
}

/// A change's path, kind and blob ids.
type Change = (RepoPath, ChangeKind, Option<RevId>, Option<RevId>);

fn changes(commit: &CommitInfo) -> Vec<Change> {
    commit
        .changes
        .iter()
        .map(|change| {
            assert!(change.lines.is_none());
            (
                change.path.clone(),
                change.kind,
                change.old_blob.clone(),
                change.new_blob.clone(),
            )
        })
        .collect()
}

/// The id fixture git gives `<commit>:<name>` in `store`.
fn blob_at(store: &GitStore, commit: &RevId, name: &str) -> Option<RevId> {
    git_ids(store, &["rev-parse", &format!("{}:{name}", commit.0)]).pop()
}

#[test]
fn commit_records_follow_the_first_parent_in_topological_order() {
    let history = History::new();
    let root_tree = history.tree(&[("README", b"r\n"), ("dir/a.txt", b"a\n")]);
    let root = history.commit(&root_tree, &[], "root\n", 1_700_000_000);
    // Author and committer dates differ, with offsets other than UTC.
    let main = history.commit_at(
        &history.tree(&[("README", b"main\n"), ("dir/a.txt", b"a\n")]),
        &[&root],
        b"subject only",
        "1700000000 +0530",
        "1700000500 -0700",
    );
    let side_message = b"side\n\n\nwith blank lines\n\n";
    let side = history.commit_at(
        &history.tree(&[
            ("README", b"r\n"),
            ("dir/a.txt", b"a\n"),
            ("dir/s.txt", b"s\n"),
        ]),
        &[&root],
        side_message,
        "1700000100 -0700",
        "1700000600 +0530",
    );
    let merged_tree = history.tree(&[
        ("README", b"main\n"),
        ("dir/a.txt", b"a\n"),
        ("dir/s.txt", b"s\n"),
    ]);
    let merge = history.commit(&merged_tree, &[&main, &side], "merge\n", 1_700_000_700);
    // An empty commit dated before its parents.
    let empty = history.commit(&merged_tree, &[&merge], "empty\n", 1_600_000_000);
    let other = history.commit(
        &history.tree(&[("README", b"r\n"), ("dir/a.txt", b"other\n")]),
        &[&root],
        "other\n",
        1_700_000_800,
    );
    // A merge that keeps its first parent's tree: no first-parent changes.
    let ours = history.commit(&merged_tree, &[&empty, &other], "ours\n", 1_700_000_900);
    let store = history.store("records", &[&ours]);

    let all = RevRange {
        from: None,
        to: ours.clone(),
    };
    let commits = store.log(&all, &[]).unwrap();
    assert_eq!(
        ids(&commits),
        git_ids(&store, &["rev-list", "--topo-order", &ours.0])
    );
    assert_eq!(commits.len(), 7);
    let position = |id: &RevId| commits.iter().position(|commit| &commit.id == id).unwrap();
    for commit in &commits {
        for parent in &commit.parents {
            assert!(position(&commit.id) < position(parent));
        }
    }
    let record = |id: &RevId| &commits[position(id)];

    // The author's date in UTC, whatever the committer's date and the offsets.
    assert_eq!(record(&main).authored_at, "2023-11-14T22:13:20Z");
    assert_eq!(record(&side).authored_at, "2023-11-14T22:15:00Z");
    assert_eq!(record(&empty).authored_at, "2020-09-13T12:26:40Z");
    // The message as stored, byte for byte.
    assert_eq!(record(&main).message.as_bytes(), b"subject only");
    assert_eq!(record(&side).message.as_bytes(), side_message);
    assert_eq!(record(&merge).message, "merge\n");
    assert_eq!(record(&merge).parents, [main.clone(), side.clone()]);
    assert_eq!(record(&merge).author.email, "ada@example.com");

    // A root commit against the empty tree.
    assert_eq!(
        changes(record(&root)),
        [
            (
                path(b"README"),
                ChangeKind::Added,
                None,
                blob_at(&store, &root, "README")
            ),
            (
                path(b"dir/a.txt"),
                ChangeKind::Added,
                None,
                blob_at(&store, &root, "dir/a.txt")
            ),
        ]
    );
    assert_eq!(
        changes(record(&main)),
        [(
            path(b"README"),
            ChangeKind::Modified,
            blob_at(&store, &root, "README"),
            blob_at(&store, &main, "README")
        )]
    );
    // A merge against its first parent only.
    assert_eq!(
        changes(record(&merge)),
        [(
            path(b"dir/s.txt"),
            ChangeKind::Added,
            None,
            blob_at(&store, &side, "dir/s.txt")
        )]
    );
    assert!(record(&empty).changes.is_empty());
    assert!(record(&ours).changes.is_empty());

    // `read_commit` gives the record `log` gives.
    for commit in &commits {
        assert_eq!(&store.read_commit(&commit.id).unwrap(), commit);
    }

    // With paths: the commits whose first-parent changes touch them, with those changes alone.
    let dir = store.log(&all, &[path(b"dir")]).unwrap();
    let expected: Vec<RevId> = ids(&commits)
        .into_iter()
        .filter(|id| [&merge, &side, &other, &root].contains(&id))
        .collect();
    assert_eq!(ids(&dir), expected);
    for commit in &dir {
        assert!(
            commit
                .changes
                .iter()
                .all(|change| change.path.0.starts_with(b"dir/"))
        );
    }
    let in_dir = |id: &RevId| dir.iter().find(|commit| &commit.id == id).unwrap();
    assert_eq!(changes(in_dir(&merge)), changes(record(&merge)));
    assert_eq!(changes(in_dir(&root)), changes(record(&root))[1..].to_vec());
    assert_eq!(
        ids(&store
            .log(&all, &[path(b"README"), path(b"dir/s.txt")])
            .unwrap()),
        ids(&commits)
            .into_iter()
            .filter(|id| [&merge, &main, &side, &root].contains(&id))
            .collect::<Vec<_>>()
    );
    for nothing in [path(b"di"), path(b"*"), path(b"READ")] {
        assert!(store.log(&all, &[nothing]).unwrap().is_empty());
    }
    for refused in [path(b""), path(b"dir/")] {
        assert!(matches!(
            store.log(&all, &[refused]),
            Err(GitError::Parse { .. })
        ));
    }

    // A range.
    let range = RevRange {
        from: Some(main.clone()),
        to: empty.clone(),
    };
    assert_eq!(
        ids(&store.log(&range, &[]).unwrap()),
        [empty.clone(), merge.clone(), side.clone()]
    );

    assert!(matches!(
        store.read_commit(&RevId(format!("{}~1", ours.0))),
        Err(GitError::RevisionSyntax(_))
    ));
    assert!(matches!(
        store.read_commit(&RevId(GITLINK.to_owned())),
        Err(GitError::NotFound(_))
    ));
}

#[test]
fn commit_records_need_a_valid_author_line() {
    let history = History::new();
    let tree = history.tree(&[("f", b"f\n")]);
    let root = history.commit(&tree, &[], "root\n", 1_700_000_000);
    let store = history.store("authors", &[&root]);
    let committer = "committer Ada Lovelace <ada@example.com> 1700000000 +0000";
    let craft = |author: Option<&str>| {
        let mut raw = format!("tree {tree}\nparent {}\n", root.0);
        if let Some(author) = author {
            raw.push_str(&format!("author {author}\n"));
        }
        raw.push_str(&format!("{committer}\n\ncrafted\n"));
        RevId(text(&run_fixture(
            fixture_command(Some(store.path())),
            &[
                "hash-object",
                "-t",
                "commit",
                "--literally",
                "-w",
                "--stdin",
            ],
            raw.as_bytes(),
        )))
    };

    // The name is everything before the last ` <`, as the shim's pattern reads it.
    let odd = store
        .read_commit(&craft(Some(
            "Odd <x> Name <odd@example.com> 1700000000 -0130",
        )))
        .unwrap();
    assert_eq!(odd.author.name, "Odd <x> Name");
    assert_eq!(odd.author.email, "odd@example.com");
    assert_eq!(odd.authored_at, "2023-11-14T22:13:20Z");
    assert_eq!(odd.message, "crafted\n");
    assert_eq!(odd.parents, std::slice::from_ref(&root));

    for author in [
        None,
        Some("Nobody 1700000000 +0000"),
        Some("Ada <ada@example.com>"),
        Some("Ada <ada@example.com> 1700000000"),
        Some("Ada <ada@example.com> 1700000000 0000"),
        Some("Ada <ada@example.com> 1700000000 +00000"),
        Some("Ada <ada@example.com> -1 +0000"),
        Some("Ada <ada@example.com> 99999999999999999999999 +0000"),
        Some("Ada <a>b@example.com> 1700000000 +0000"),
        Some("Ada<ada@example.com> 1700000000 +0000"),
    ] {
        let crafted = craft(author);
        assert!(
            matches!(store.read_commit(&crafted), Err(GitError::Parse { .. })),
            "{author:?}"
        );
        let range = RevRange {
            from: None,
            to: crafted,
        };
        assert!(
            matches!(store.log(&range, &[]), Err(GitError::Parse { .. })),
            "{author:?}"
        );
    }
}
