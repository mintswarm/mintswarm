//! The read operations and `diff`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use mintswarm_domain::ids::RevId;
use mintswarm_domain::vcs::{ChangeKind, DiffOpts, RepoPath, RevRange, TreeMode, Vcs};

use super::super::{DIFF_OPTIONS, GitError, GitStore};
use super::{Fixture, GITLINK, path};

fn ids(commits: &[mintswarm_domain::vcs::CommitInfo]) -> Vec<&RevId> {
    commits.iter().map(|commit| &commit.id).collect()
}

#[test]
fn resolve_refuses_revision_syntax() {
    let fx = Fixture::new();
    let store = fx.fetched_store("resolve");
    let id = &fx.side.0;
    assert_eq!(store.resolve(id).unwrap(), fx.side);
    let refused = [
        "HEAD".to_owned(),
        "side".to_owned(),
        "refs/heads/side".to_owned(),
        "v1".to_owned(),
        id[..12].to_owned(),
        id.to_uppercase(),
        format!("{id}0"),
        format!("{id}{id}")[..64].to_owned(),
        "0".repeat(40),
        format!("{}..{id}", fx.base.0),
        format!("{}...{id}", fx.base.0),
        format!("{id}^"),
        format!("{id}~1"),
        format!("{id}^{{tree}}"),
        format!("{id}@{{1}}"),
        ":/side".to_owned(),
        "-n".to_owned(),
        format!(" {id}"),
        format!("{id}\n"),
        String::new(),
    ];
    for rev in &refused {
        assert!(
            matches!(store.resolve(rev), Err(GitError::RevisionSyntax(_))),
            "{rev:?}"
        );
    }
    // A full id of a blob, or of nothing in the store, names no commit.
    let tree = store.list_tree(&fx.side).unwrap();
    assert!(matches!(
        store.resolve(&tree[0].blob.0),
        Err(GitError::NotFound(_))
    ));
    assert!(matches!(store.resolve(GITLINK), Err(GitError::NotFound(_))));
}

#[test]
fn parse_range_accepts_only_ids_and_two_dots() {
    let fx = Fixture::new();
    let store = fx.fetched_store("range");
    let (a, b) = (&fx.base.0, &fx.side.0);
    assert_eq!(
        store.parse_range(b).unwrap(),
        RevRange {
            from: None,
            to: fx.side.clone()
        }
    );
    assert_eq!(
        store.parse_range(&format!("{a}..{b}")).unwrap(),
        RevRange {
            from: Some(fx.base.clone()),
            to: fx.side.clone()
        }
    );
    for text in [
        format!("..{b}"),
        format!("{a}.."),
        format!("{a}...{b}"),
        format!("{a}..{b}..{a}"),
        "main..side".to_owned(),
        format!("{a}..HEAD"),
        "..".to_owned(),
    ] {
        assert!(
            matches!(store.parse_range(&text), Err(GitError::RevisionSyntax(_))),
            "{text}"
        );
    }
    assert!(matches!(
        store.parse_range(&format!("{a}..{GITLINK}")),
        Err(GitError::NotFound(_))
    ));
}

#[test]
fn read_blob_reads_files_only() {
    let fx = Fixture::new();
    let store = fx.fetched_store("blobs");
    let read = |bytes: &[u8]| store.read_blob(&fx.side, &path(bytes)).unwrap();
    assert_eq!(read(b"keep.txt"), Some(b"a\nB\nc\nd\n".to_vec()));
    assert_eq!(read(b"dir/deep.txt"), Some(b"d\n".to_vec()));
    assert_eq!(read(b"odd\xff\nname"), Some(b"y\n".to_vec()));
    assert_eq!(read(b"new file.txt"), Some(b"x\n".to_vec()));
    assert_eq!(read(b"link"), Some(b"README".to_vec()));
    assert_eq!(read(b"bin.dat"), Some(b"\0\x03".to_vec()));
    for missing in [
        &b"dir"[..],
        b"sub",
        b"gone.txt",
        b"nothing",
        b"",
        b"a\0b",
        b"*.txt",
        b"dir/",
    ] {
        assert_eq!(read(missing), None, "{}", String::from_utf8_lossy(missing));
    }
    assert!(matches!(
        store.read_blob(&RevId("HEAD".to_owned()), &path(b"README")),
        Err(GitError::RevisionSyntax(_))
    ));
}

#[test]
fn list_tree_lists_every_entry() {
    let fx = Fixture::new();
    let store = fx.fetched_store("tree");
    let entries = store.list_tree(&fx.side).unwrap();
    let modes: BTreeMap<RepoPath, u32> = entries
        .iter()
        .map(|entry| (entry.path.clone(), entry.mode))
        .collect();
    let expected: BTreeMap<RepoPath, u32> = [
        (&b"README"[..], 0o100644),
        (b"alias", 0o120000),
        (b"bin.dat", 0o100644),
        (b"dir/deep.txt", 0o100644),
        (b"dir/nested.txt", 0o100644),
        (b"keep.txt", 0o100644),
        (b"link", 0o120000),
        (b"new file.txt", 0o100644),
        (b"odd\xff\nname", 0o100644),
        (b"script.sh", 0o100755),
        (b"sub", 0o160000),
    ]
    .into_iter()
    .map(|(name, mode)| (path(name), mode))
    .collect();
    assert_eq!(modes, expected);
    let sub = entries
        .iter()
        .find(|entry| entry.path == path(b"sub"))
        .unwrap();
    assert_eq!(sub.blob.0, GITLINK);
}

#[test]
fn log_lists_commits_in_topological_order() {
    let fx = Fixture::new();
    let store = fx.fetched_store("log");
    let range = RevRange {
        from: Some(fx.base.clone()),
        to: fx.merged.clone(),
    };
    let commits = store.log(&range, &[]).unwrap();
    let topo: Vec<RevId> = super::text(&super::fixture_git(
        store.path(),
        &[
            "rev-list",
            "--topo-order",
            &fx.merged.0,
            &format!("^{}", fx.base.0),
        ],
    ))
    .lines()
    .map(|line| RevId(line.to_owned()))
    .collect();
    assert_eq!(ids(&commits), topo.iter().collect::<Vec<_>>());
    assert_eq!(ids(&commits), [&fx.merged, &fx.main, &fx.side]);
    // Children come before their parents.
    for (at, commit) in commits.iter().enumerate() {
        for parent in &commit.parents {
            assert!(!ids(&commits[..at]).contains(&parent), "{}", parent.0);
        }
    }
    let merge = &commits[0];
    assert_eq!(merge.parents, [fx.side.clone(), fx.main.clone()]);
    assert_eq!(merge.message, "merge main\n");
    assert_eq!(merge.author.name, "Ada Lovelace");
    assert_eq!(merge.author.email, "ada@example.com");
    assert_eq!(merge.authored_at, "2023-11-14T22:18:20Z");
    let side = &commits[2];
    assert_eq!(side.message, "side work\n\nbody line\n");
    assert_eq!(side.authored_at, "2023-11-14T22:16:40Z");
    assert_eq!(side.parents, std::slice::from_ref(&fx.base));

    let all = RevRange {
        from: None,
        to: fx.merged.clone(),
    };
    assert_eq!(
        ids(&store.log(&all, &[path(b"keep.txt")]).unwrap()),
        [&fx.side, &fx.base]
    );
    assert_eq!(
        ids(&store.log(&all, &[path(b"odd\xff\nname")]).unwrap()),
        [&fx.side]
    );
    assert!(store.log(&all, &[path(b"*")]).unwrap().is_empty());
    let root = store
        .log(
            &RevRange {
                from: None,
                to: fx.base.clone(),
            },
            &[],
        )
        .unwrap();
    assert_eq!(ids(&root), [&fx.base]);
    assert!(root[0].parents.is_empty());
    let empty = RevRange {
        from: Some(fx.side.clone()),
        to: fx.side.clone(),
    };
    assert!(store.log(&empty, &[]).unwrap().is_empty());
    let bad = RevRange {
        from: Some(RevId("main".to_owned())),
        to: fx.side.clone(),
    };
    assert!(matches!(
        store.log(&bad, &[]),
        Err(GitError::RevisionSyntax(_))
    ));
}

#[test]
fn log_ignores_the_store_log_config() {
    let fx = Fixture::new();
    let store = fx.fetched_store("log-config");
    let all = RevRange {
        from: None,
        to: fx.orphan.clone(),
    };
    let side = RevRange {
        from: None,
        to: fx.side.clone(),
    };
    let observe = || {
        (
            store.log(&all, &[]).unwrap(),
            store.log(&side, &[path(b"keep.txt")]).unwrap(),
        )
    };
    let baseline = observe();
    assert_eq!(baseline.0[0].message, "unrelated café\n");
    for (key, value) in [
        ("log.showSignature", "true"),
        ("i18n.logOutputEncoding", "ISO-8859-1"),
        ("log.follow", "true"),
        ("diff.orderFile", "/nonexistent"),
        ("diff.renames", "copies"),
        ("diff.relative", "true"),
    ] {
        super::fixture_git(store.path(), &["config", key, value]);
    }
    assert_eq!(observe(), baseline);
}

#[test]
fn merge_bases_finds_the_common_ancestors() {
    let fx = Fixture::new();
    let store = fx.fetched_store("base");
    assert_eq!(
        store.merge_bases(&fx.main, &fx.side).unwrap(),
        std::slice::from_ref(&fx.base)
    );
    assert_eq!(
        store.merge_bases(&fx.merged, &fx.main).unwrap(),
        std::slice::from_ref(&fx.main)
    );
    assert!(store.merge_bases(&fx.side, &fx.orphan).unwrap().is_empty());
}

#[test]
fn diff_reports_kinds_modes_and_lines() {
    let fx = Fixture::new();
    let store = fx.fetched_store("diff");
    let plain = store
        .diff(&fx.base, &fx.side, DiffOpts::default(), &[])
        .unwrap();
    let kinds: BTreeMap<RepoPath, ChangeKind> = plain
        .files
        .iter()
        .map(|file| (file.path.clone(), file.kind))
        .collect();
    let expected: BTreeMap<RepoPath, ChangeKind> = [
        (&b"alias"[..], ChangeKind::Added),
        (b"bin.dat", ChangeKind::Modified),
        (b"dir/deep.txt", ChangeKind::Added),
        (b"gone.txt", ChangeKind::Deleted),
        (b"keep.txt", ChangeKind::Modified),
        (b"link", ChangeKind::TypeChanged),
        (b"new file.txt", ChangeKind::Added),
        (b"odd\xff\nname", ChangeKind::Added),
        (b"script.sh", ChangeKind::Modified),
        (b"sub", ChangeKind::Added),
    ]
    .into_iter()
    .map(|(name, kind)| (path(name), kind))
    .collect();
    assert_eq!(kinds, expected);
    assert!(plain.patch.is_none());
    assert!(plain.files.iter().all(|file| file.old_mode.is_none()
        && file.new_mode.is_none()
        && file.old_blob.is_none()
        && file.new_blob.is_none()
        && file.lines.is_none()));

    let full = store
        .diff(
            &fx.base,
            &fx.side,
            DiffOpts {
                numstat: true,
                added: true,
                raw: true,
                patch: true,
            },
            &[],
        )
        .unwrap();
    let file = |name: &[u8]| {
        full.files
            .iter()
            .find(|file| file.path == path(name))
            .unwrap()
    };
    assert_eq!(
        (file(b"script.sh").old_mode, file(b"script.sh").new_mode),
        (Some(0o100644), Some(0o100755))
    );
    assert_eq!(
        (file(b"gone.txt").old_mode, file(b"gone.txt").new_mode),
        (Some(0o100644), None)
    );
    assert_eq!(
        (file(b"sub").old_mode, file(b"sub").new_mode),
        (None, Some(0o160000))
    );
    // The blob ids are fixture git's, and `None` on a missing side.
    let blob = |commit: &RevId, name: &str| {
        Some(RevId(super::text(&super::fixture_git(
            &fx.source,
            &["rev-parse", &format!("{}:{name}", commit.0)],
        ))))
    };
    for name in ["keep.txt", "script.sh", "bin.dat", "link"] {
        assert_eq!(
            (
                file(name.as_bytes()).old_blob.clone(),
                file(name.as_bytes()).new_blob.clone()
            ),
            (blob(&fx.base, name), blob(&fx.side, name)),
            "{name}"
        );
    }
    assert_eq!(
        (
            file(b"gone.txt").old_blob.clone(),
            file(b"gone.txt").new_blob.clone()
        ),
        (blob(&fx.base, "gone.txt"), None)
    );
    assert_eq!(
        (
            file(b"new file.txt").old_blob.clone(),
            file(b"new file.txt").new_blob.clone()
        ),
        (None, blob(&fx.side, "new file.txt"))
    );
    assert_eq!(
        (file(b"sub").old_blob.clone(), file(b"sub").new_blob.clone()),
        (None, Some(RevId(GITLINK.to_owned())))
    );
    assert_eq!(file(b"keep.txt").lines, Some((2, 1)));
    assert_eq!(file(b"gone.txt").lines, Some((0, 1)));
    assert_eq!(file(b"bin.dat").lines, None);
    assert!(!full.patch.unwrap().is_empty());

    assert!(
        store
            .diff(&fx.side, &fx.side, DiffOpts::default(), &[])
            .unwrap()
            .files
            .is_empty()
    );
    let bad = RevId(format!("{}^", fx.side.0));
    assert!(matches!(
        store.diff(&bad, &fx.side, DiffOpts::default(), &[]),
        Err(GitError::RevisionSyntax(_))
    ));
}

#[test]
fn write_operations_are_unsupported() {
    let fx = Fixture::new();
    let store = fx.store("unsupported");
    let result = store.materialise(&fx.side, Path::new("/nonexistent"), TreeMode::Writable);
    assert!(matches!(result, Err(GitError::Unsupported(_))));
}

/// `landing/floor.py`'s `DIFF_OPTIONS`, read from the file without running it.
fn floor_diff_options() -> Vec<String> {
    let floor =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.mintswarm/dogfood/landing/floor.py");
    let source = std::fs::read_to_string(floor).unwrap();
    let start = source.find("\nDIFF_OPTIONS = (\n").unwrap();
    let body = &source[start..];
    let body = &body[..body.find("\n)\n").unwrap()];
    body.lines()
        .skip(2)
        .map(|line| {
            let line = line.trim();
            line.strip_prefix('"')
                .and_then(|line| line.strip_suffix("\","))
                .unwrap()
                .to_owned()
        })
        .collect()
}

#[test]
fn diff_options_equal_landing_floor() {
    assert_eq!(floor_diff_options(), DIFF_OPTIONS);
}

/// What the landing floor's three diff commands give for `base...head`: modes and kind by path,
/// line counts by path, and the added paths.
type FloorDiff = (
    BTreeMap<RepoPath, (u32, u32, u8)>,
    BTreeMap<RepoPath, Option<(u64, u64)>>,
    BTreeSet<RepoPath>,
);

fn floor_diff(store: &GitStore, base: &RevId, head: &RevId) -> FloorDiff {
    let options = floor_diff_options();
    let span = format!("{}...{}", base.0, head.0);
    let run = |form: &[&str]| {
        let mut args: Vec<&str> = vec!["diff"];
        args.extend(form);
        args.extend(options.iter().map(String::as_str));
        args.push(&span);
        store.git(args).unwrap()
    };
    let records = |out: &[u8]| -> Vec<Vec<u8>> {
        let out = out.strip_suffix(b"\0").unwrap_or(out);
        out.split(|&byte| byte == 0)
            .filter(|_| !out.is_empty())
            .map(<[u8]>::to_vec)
            .collect()
    };
    let raw = records(&run(&["--raw", "--no-abbrev"]));
    let mut modes = BTreeMap::new();
    for pair in raw.chunks(2) {
        let header = String::from_utf8(pair[0].clone()).unwrap();
        let fields: Vec<&str> = header[1..].split(' ').collect();
        let mode = |text: &str| u32::from_str_radix(text, 8).unwrap();
        modes.insert(
            RepoPath(pair[1].clone()),
            (mode(fields[0]), mode(fields[1]), fields[4].as_bytes()[0]),
        );
    }
    let mut lines = BTreeMap::new();
    for record in records(&run(&["--numstat"])) {
        let parts: Vec<&[u8]> = record.splitn(3, |&byte| byte == b'\t').collect();
        let count = |bytes: &[u8]| std::str::from_utf8(bytes).unwrap().parse::<u64>().ok();
        let counts = count(parts[0]).zip(count(parts[1]));
        lines.insert(RepoPath(parts[2].to_vec()), counts);
    }
    let added = records(&run(&["--name-only", "--diff-filter=A"]))
        .into_iter()
        .map(RepoPath)
        .collect();
    (modes, lines, added)
}

#[test]
fn diff_equals_landing_floor_diff() {
    let fx = Fixture::new();
    let store = fx.fetched_store("floor");
    for (base, head) in [
        (&fx.main, &fx.side),
        (&fx.base, &fx.merged),
        (&fx.side, &fx.main),
    ] {
        let (modes, lines, added) = floor_diff(&store, base, head);
        assert!(!modes.is_empty());
        let bases = store.merge_bases(base, head).unwrap();
        let [fork] = bases.as_slice() else {
            panic!("one merge base expected, not {}", bases.len());
        };
        let ours = store
            .diff(
                fork,
                head,
                DiffOpts {
                    numstat: true,
                    added: true,
                    raw: true,
                    patch: false,
                },
                &[],
            )
            .unwrap();
        let letter = |kind: ChangeKind| match kind {
            ChangeKind::Added => b'A',
            ChangeKind::Modified => b'M',
            ChangeKind::Deleted => b'D',
            ChangeKind::TypeChanged => b'T',
            ChangeKind::Renamed => b'R',
            ChangeKind::Copied => b'C',
        };
        let our_modes: BTreeMap<RepoPath, (u32, u32, u8)> = ours
            .files
            .iter()
            .map(|file| {
                (
                    file.path.clone(),
                    (
                        file.old_mode.unwrap_or(0),
                        file.new_mode.unwrap_or(0),
                        letter(file.kind),
                    ),
                )
            })
            .collect();
        let our_lines: BTreeMap<RepoPath, Option<(u64, u64)>> = ours
            .files
            .iter()
            .map(|file| (file.path.clone(), file.lines))
            .collect();
        let our_added: BTreeSet<RepoPath> = ours
            .files
            .iter()
            .filter(|file| file.kind == ChangeKind::Added)
            .map(|file| file.path.clone())
            .collect();
        assert_eq!(our_modes, modes);
        assert_eq!(our_lines, lines);
        assert_eq!(our_added, added);
    }
}
