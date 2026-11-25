//! Tests of the git adapter, on fixture repositories built with git plumbing and fetched through
//! the test-only `file://` remote.

mod bundle;
mod read;
mod snapshot;
mod store;

use std::ffi::{OsStr, OsString};
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use mintswarm_domain::ids::RevId;
use mintswarm_domain::probe::{ProbeResult, ProbeValue};
use mintswarm_domain::vcs::{DiffOpts, RefOrId, Remote, RepoPath, RevRange, Vcs};
use tempfile::TempDir;

use super::env::{DROPPED_ENV, FIXED_ENV};
use super::{GitError, GitStore};

/// A `tool.git` probe result of `version`, or `unknown`.
pub(crate) fn git_probe(version: Option<&str>) -> ProbeResult {
    ProbeResult {
        namespace: "tool.git".parse().unwrap(),
        value: match version {
            Some(version) => ProbeValue::Known {
                version: Some(version.to_owned()),
                facts: Default::default(),
            },
            None => ProbeValue::Unknown,
        },
        observed_at: "2026-11-25T00:00:00Z".to_owned(),
    }
}

/// git for building fixtures, with no system or global config and fixed identities; `GIT_DIR`
/// is `git_dir` when given.
pub(crate) fn fixture_command(git_dir: Option<&Path>) -> Command {
    let mut command = Command::new("git");
    command.env_clear();
    if let Some(path) = std::env::var_os("PATH") {
        command.env("PATH", path);
    }
    command.envs([
        ("GIT_CONFIG_NOSYSTEM", "1"),
        ("GIT_CONFIG_GLOBAL", "/dev/null"),
        ("LC_ALL", "C"),
        ("GIT_AUTHOR_NAME", "Ada Lovelace"),
        ("GIT_AUTHOR_EMAIL", "ada@example.com"),
        ("GIT_COMMITTER_NAME", "Ada Lovelace"),
        ("GIT_COMMITTER_EMAIL", "ada@example.com"),
    ]);
    if let Some(dir) = git_dir {
        command.env("GIT_DIR", dir);
    }
    command
}

/// Run fixture git with `stdin` and return its output; it must succeed.
pub(crate) fn run_fixture<S: AsRef<OsStr>>(
    mut command: Command,
    args: &[S],
    stdin: &[u8],
) -> Vec<u8> {
    command.args(args);
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().unwrap();
    child.stdin.take().unwrap().write_all(stdin).unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "fixture git {:?} failed",
        args.iter()
            .map(|arg| arg.as_ref().to_owned())
            .collect::<Vec<_>>()
    );
    output.stdout
}

/// Fixture git on the repository at `git_dir`.
pub(crate) fn fixture_git<S: AsRef<OsStr>>(git_dir: &Path, args: &[S]) -> Vec<u8> {
    run_fixture(fixture_command(Some(git_dir)), args, b"")
}

/// A bare repository built with plumbing, so its ids do not depend on the filesystem.
pub(super) struct Fixture {
    pub dir: TempDir,
    pub source: PathBuf,
    /// The root commit, also the lightweight tag `v1`. The annotated tag `v2` and the tag
    /// `v2-outer` of `v2` end at `main`; the annotated tag `tree` is of a tree.
    pub base: RevId,
    /// Branch `main`: one commit on `base`, changing `README`.
    pub main: RevId,
    /// Branch `side`: one commit on `base` with every kind of change.
    pub side: RevId,
    /// Branch `merged`: `side` merged with `main`.
    pub merged: RevId,
    /// Branch `orphan`: a root commit sharing no history, with a non-ASCII subject.
    pub orphan: RevId,
}

/// One entry of a fixture tree: mode, type, id and name.
type Entry = (&'static str, &'static str, String, Vec<u8>);

/// The gitlink of the fixture: a commit that is not in the repository.
pub(super) const GITLINK: &str = "0123456789abcdef0123456789abcdef01234567";

impl Fixture {
    pub(super) fn new() -> Fixture {
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
        let blob = |data: &[u8]| {
            text(&run_fixture(
                fixture_command(Some(&source)),
                &["hash-object", "-w", "--stdin"],
                data,
            ))
        };
        let file =
            |name: &[u8], data: &[u8]| -> Entry { ("100644", "blob", blob(data), name.to_vec()) };
        let tree = |entries: &[Entry]| {
            let mut input = Vec::new();
            for (mode, kind, id, name) in entries {
                input.extend_from_slice(format!("{mode} {kind} {id}\t").as_bytes());
                input.extend_from_slice(name);
                input.push(0);
            }
            text(&run_fixture(
                fixture_command(Some(&source)),
                &["mktree", "-z", "--missing"],
                &input,
            ))
        };
        let commit = |tree: &str, parents: &[&RevId], message: &str, time: u64| {
            let mut command = fixture_command(Some(&source));
            let date = format!("{time} +0000");
            command
                .env("GIT_AUTHOR_DATE", &date)
                .env("GIT_COMMITTER_DATE", &date);
            let mut args = vec![
                "commit-tree".to_owned(),
                tree.to_owned(),
                "-m".to_owned(),
                message.to_owned(),
            ];
            for parent in parents {
                args.extend(["-p".to_owned(), parent.0.clone()]);
            }
            RevId(text(&run_fixture(command, &args, b"")))
        };

        let nested = tree(&[file(b"nested.txt", b"n\n")]);
        let base_entries = vec![
            file(b"README", b"hello\n"),
            file(b"keep.txt", b"a\nb\nc\n"),
            file(b"gone.txt", b"bye\n"),
            file(b"script.sh", b"#!/bin/sh\n"),
            file(b"bin.dat", b"\0\x01\x02"),
            file(b"link", b"plain\n"),
            ("040000", "tree", nested.clone(), b"dir".to_vec()),
        ];
        let base = commit(&tree(&base_entries), &[], "base", 1_700_000_000);

        let readme_main = file(b"README", b"hello main\n");
        let mut main_entries = base_entries.clone();
        main_entries[0] = readme_main.clone();
        let main = commit(&tree(&main_entries), &[&base], "main work", 1_700_000_100);

        let deep = tree(&[file(b"nested.txt", b"n\n"), file(b"deep.txt", b"d\n")]);
        let mut side_entries = vec![
            file(b"README", b"hello\n"),
            file(b"keep.txt", b"a\nB\nc\nd\n"),
            (
                "100755",
                "blob",
                blob(b"#!/bin/sh\n"),
                b"script.sh".to_vec(),
            ),
            file(b"bin.dat", b"\0\x03"),
            ("120000", "blob", blob(b"README"), b"link".to_vec()),
            ("040000", "tree", deep, b"dir".to_vec()),
            file(b"new file.txt", b"x\n"),
            file(b"odd\xff\nname", b"y\n"),
            ("120000", "blob", blob(b"dir/nested.txt"), b"alias".to_vec()),
            ("160000", "commit", GITLINK.to_owned(), b"sub".to_vec()),
        ];
        let side = commit(
            &tree(&side_entries),
            &[&base],
            "side work\n\nbody line",
            1_700_000_200,
        );

        side_entries[0] = readme_main;
        let merged = commit(
            &tree(&side_entries),
            &[&side, &main],
            "merge main",
            1_700_000_300,
        );
        let orphan = commit(
            &tree(&[file(b"other", b"o\n")]),
            &[],
            "unrelated café",
            1_700_000_400,
        );

        for (name, id) in [
            ("refs/heads/main", &main),
            ("refs/heads/side", &side),
            ("refs/heads/merged", &merged),
            ("refs/heads/orphan", &orphan),
            ("refs/tags/v1", &base),
        ] {
            fixture_git(&source, &["update-ref", name, &id.0]);
        }
        let tag = |name: &str, kind: &str, id: &str| {
            let input = format!(
                "object {id}\ntype {kind}\ntag {name}\n\
                 tagger Ada Lovelace <ada@example.com> 1700000500 +0000\n\n{name}\n"
            );
            let tag = text(&run_fixture(
                fixture_command(Some(&source)),
                &["mktag"],
                input.as_bytes(),
            ));
            fixture_git(&source, &["update-ref", &format!("refs/tags/{name}"), &tag]);
        };
        tag("v2", "commit", &main.0);
        tag(
            "v2-outer",
            "tag",
            &text(&fixture_git(&source, &["rev-parse", "refs/tags/v2"])),
        );
        tag("tree", "tree", &nested);
        fixture_git(&source, &["symbolic-ref", "HEAD", "refs/heads/side"]);
        Fixture {
            dir,
            source,
            base,
            main,
            side,
            merged,
            orphan,
        }
    }

    /// The source as the test-only remote.
    pub(super) fn remote(&self) -> Remote {
        Remote {
            name: "origin".to_owned(),
            url: format!("file://{}", self.source.display()),
        }
    }

    /// A new store under the fixture's state directory, created with git 2.45.0.
    pub(super) fn store(&self, name: &str) -> GitStore {
        let path = super::store_path(&self.dir.path().join("state"), name).unwrap();
        let mut store = GitStore::create(&path, &git_probe(Some("2.45.0"))).unwrap();
        store.allow_file_remote();
        store
    }

    /// A store with every branch of the fixture fetched.
    pub(super) fn fetched_store(&self, name: &str) -> GitStore {
        let store = self.store(name);
        let wants = ["main", "side", "merged", "orphan"]
            .map(|branch| RefOrId::Ref(format!("refs/heads/{branch}")));
        store.fetch(&self.remote(), &wants, None).unwrap();
        store
    }
}

/// A commit of `tree` on `parents` in the repository at `git_dir`, made by `commit-tree` with
/// `message` as given on its standard input, at `time` with the fixture's identities, and with an
/// `encoding` header when `encoding` names one.
pub(super) fn commit_with(
    git_dir: &Path,
    tree: &str,
    parents: &[&str],
    message: &[u8],
    encoding: Option<&str>,
    time: u64,
) -> RevId {
    let mut command = fixture_command(Some(git_dir));
    let date = format!("{time} +0000");
    command
        .env("GIT_AUTHOR_DATE", &date)
        .env("GIT_COMMITTER_DATE", &date);
    let mut args = Vec::new();
    if let Some(encoding) = encoding {
        args.extend(["-c".to_owned(), format!("i18n.commitEncoding={encoding}")]);
    }
    args.extend(["commit-tree".to_owned(), tree.to_owned()]);
    for parent in parents {
        args.extend(["-p".to_owned(), (*parent).to_owned()]);
    }
    RevId(text(&run_fixture(command, &args, message)))
}

/// Output as text without its final newline.
pub(crate) fn text(out: &[u8]) -> String {
    String::from_utf8(out.to_vec())
        .unwrap()
        .trim_end()
        .to_owned()
}

/// A repository path.
pub(crate) fn path(bytes: &[u8]) -> RepoPath {
    RepoPath(bytes.to_vec())
}

/// Write an executable script.
pub(super) fn script(path: &Path, body: &str) {
    std::fs::write(path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

/// Set in the child that a test runs in by [`run_in_child`].
const CHILD: &str = "MSW_TEST_GIT_CHILD";

/// Run the test `test` again in a child process, with `envs` set in its real environment and
/// `removed` unset; `std::env::set_var` would need unsafe code. The child writes `done` to the
/// file `CHILD` names, so a filter that matched nothing cannot pass.
fn run_in_child(test: &str, dir: &Path, envs: &[(&str, OsString)], removed: &[&str]) {
    let done = dir.join("child-done");
    let mut command = Command::new(std::env::current_exe().unwrap());
    command.args([test, "--exact", "--nocapture", "--test-threads=1"]);
    command.env(CHILD, &done);
    for (name, value) in envs {
        command.env(name, value);
    }
    for name in removed {
        command.env_remove(name);
    }
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "the child failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(std::fs::read_to_string(&done).unwrap(), "done");
}

/// The variables design §8 names.
const SECTION_8: &[&str] = &[
    "GIT_CONFIG_NOSYSTEM",
    "GIT_CONFIG_GLOBAL",
    "GIT_ATTR_NOSYSTEM",
    "GIT_CONFIG_PARAMETERS",
    "GIT_CONFIG_COUNT",
    "GIT_EXTERNAL_DIFF",
    "GIT_DIFF_OPTS",
    "GIT_DIR",
    "GIT_OBJECT_DIRECTORY",
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_NO_REPLACE_OBJECTS",
    "GIT_GRAFT_FILE",
    "GIT_NO_LAZY_FETCH",
];

/// The variables the hostile test sets, or unsets, in the child.
const HOSTILE: &[&str] = &[
    "GIT_DIR",
    "GIT_OBJECT_DIRECTORY",
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_CONFIG_PARAMETERS",
    "GIT_CONFIG_COUNT",
    "GIT_CONFIG_KEY_0",
    "GIT_CONFIG_VALUE_0",
    "GIT_CONFIG_KEY_1",
    "GIT_CONFIG_VALUE_1",
    "GIT_CONFIG_GLOBAL",
    "GIT_CONFIG_NOSYSTEM",
    "GIT_CONFIG_SYSTEM",
    "GIT_ATTR_NOSYSTEM",
    "GIT_EXTERNAL_DIFF",
    "GIT_DIFF_OPTS",
    "GIT_GRAFT_FILE",
    "GIT_NO_REPLACE_OBJECTS",
    "GIT_NO_LAZY_FETCH",
    "GIT_TRACE",
    "GIT_DEFAULT_REF_FORMAT",
    "GIT_SMART_HTTP",
    "GIT_SSL_VERSION",
    "GIT_SSL_CIPHER_LIST",
    "GIT_SSL_NO_VERIFY",
    "GIT_HTTP_PROXY_AUTHMETHOD",
    "GIT_HTTP_USER_AGENT",
    "GIT_HTTP_LOW_SPEED_LIMIT",
];

#[test]
fn hostile_set_covers_section_8() {
    let fixed: Vec<&str> = FIXED_ENV.iter().map(|(name, _)| *name).collect();
    for name in SECTION_8 {
        assert!(
            HOSTILE.contains(name),
            "{name} is not set by the hostile test"
        );
        assert!(
            DROPPED_ENV.contains(name) || fixed.contains(name),
            "{name} is neither dropped nor fixed"
        );
    }
}

/// Everything the hostile test compares, from one store.
fn observe(store: &GitStore, fx: &Fixture) -> String {
    let all = DiffOpts {
        numstat: true,
        added: true,
        raw: true,
        patch: true,
    };
    let range = RevRange {
        from: Some(fx.base.clone()),
        to: fx.merged.clone(),
    };
    let to_side = RevRange {
        from: None,
        to: fx.side.clone(),
    };
    format!(
        "{:?}\n{:?}\n{:?}\n{:?}\n{:?}\n{:?}\n{:?}\n{:?}",
        store.resolve(&fx.side.0).unwrap(),
        store.list_tree(&fx.side).unwrap(),
        store.read_blob(&fx.side, &path(b"keep.txt")).unwrap(),
        store.log(&range, &[]).unwrap(),
        store.log(&to_side, &[path(b"keep.txt")]).unwrap(),
        store.merge_base(&fx.main, &fx.side).unwrap(),
        store.diff(&fx.base, &fx.side, all.clone()).unwrap(),
        store.diff(&fx.main, &fx.merged, all).unwrap(),
    )
}

#[test]
fn hostile_parent_environment_has_no_effect() {
    const TEST: &str = "exec::git::tests::hostile_parent_environment_has_no_effect";
    if let Some(done) = std::env::var_os(CHILD) {
        hostile_child(Path::new(&done));
        return;
    }
    let fx = Fixture::new();
    let store = fx.fetched_store("hostile");
    // A replacement that would show `main` in place of `side`, were replace refs read.
    fixture_git(
        store.path(),
        &[
            "update-ref",
            &format!("refs/replace/{}", fx.side.0),
            &fx.main.0,
        ],
    );
    // Without the adapter's environment git reads the replacement; the adapter does not.
    let replaced = format!("{}:README", fx.side.0);
    assert_eq!(
        fixture_git(store.path(), &["cat-file", "blob", &replaced]),
        b"hello main\n"
    );
    assert_eq!(
        store.read_blob(&fx.side, &path(b"README")).unwrap(),
        Some(b"hello\n".to_vec())
    );
    assert!(
        store
            .list_tree(&fx.side)
            .unwrap()
            .iter()
            .any(|entry| entry.path == path(b"alias"))
    );
    let baseline = observe(&store, &fx);

    // Another repository, with a commit the store does not have.
    let dir = fx.dir.path();
    let other = dir.join("other.git");
    run_fixture(
        fixture_command(None),
        &[
            OsStr::new("init"),
            OsStr::new("--bare"),
            OsStr::new("--quiet"),
            other.as_os_str(),
        ],
        b"",
    );
    let tree = text(&run_fixture(
        fixture_command(Some(&other)),
        &["mktree"],
        b"",
    ));
    let extra = text(&run_fixture(
        fixture_command(Some(&other)),
        &["commit-tree", &tree, "-m", "extra"],
        b"",
    ));
    let config = dir.join("hostile.gitconfig");
    std::fs::write(&config, "[diff]\n\tnoprefix = true\n[core]\n\tabbrev = 7\n").unwrap();
    let graft = dir.join("grafts");
    std::fs::write(&graft, format!("{} {}\n", fx.side.0, fx.orphan.0)).unwrap();
    let external = dir.join("external-diff");
    script(
        &external,
        &format!("touch '{}'", dir.join("external-ran").display()),
    );
    std::fs::write(dir.join("baseline"), &baseline).unwrap();
    std::fs::write(
        dir.join("ids"),
        format!(
            "{}\n{}\n{}\n{}\n{}\n{}\n",
            fx.base.0, fx.main.0, fx.side.0, fx.merged.0, fx.orphan.0, extra
        ),
    )
    .unwrap();

    let os = |value: &str| OsString::from(value);
    let values: Vec<(&str, OsString)> = vec![
        ("GIT_DIR", other.clone().into()),
        ("GIT_OBJECT_DIRECTORY", other.join("objects").into()),
        (
            "GIT_ALTERNATE_OBJECT_DIRECTORIES",
            other.join("objects").into(),
        ),
        (
            "GIT_CONFIG_PARAMETERS",
            os("'diff.noprefix'='true' 'core.abbrev'='7'"),
        ),
        ("GIT_CONFIG_COUNT", os("2")),
        ("GIT_CONFIG_KEY_0", os("diff.noprefix")),
        ("GIT_CONFIG_VALUE_0", os("true")),
        ("GIT_CONFIG_KEY_1", os("core.abbrev")),
        ("GIT_CONFIG_VALUE_1", os("7")),
        ("GIT_CONFIG_GLOBAL", config.clone().into()),
        ("GIT_CONFIG_NOSYSTEM", os("0")),
        ("GIT_CONFIG_SYSTEM", config.into()),
        ("GIT_ATTR_NOSYSTEM", os("0")),
        ("GIT_EXTERNAL_DIFF", external.into()),
        ("GIT_DIFF_OPTS", os("--unified=0")),
        ("GIT_GRAFT_FILE", graft.into()),
        ("GIT_NO_LAZY_FETCH", os("0")),
        ("GIT_TRACE", dir.join("trace").into()),
        ("GIT_DEFAULT_REF_FORMAT", os("reftable")),
        ("GIT_SMART_HTTP", os("0")),
        ("GIT_SSL_VERSION", os("tlsv1.0")),
        ("GIT_SSL_CIPHER_LIST", os("NULL")),
        ("GIT_SSL_NO_VERIFY", os("1")),
        ("GIT_HTTP_PROXY_AUTHMETHOD", os("basic")),
        ("GIT_HTTP_USER_AGENT", os("hostile")),
        ("GIT_HTTP_LOW_SPEED_LIMIT", os("1")),
        ("MSW_TEST_GIT_STORE", store.path().into()),
        ("MSW_TEST_GIT_DIR", dir.into()),
    ];
    let removed = ["GIT_NO_REPLACE_OBJECTS"];
    let mut covered: Vec<&str> = values
        .iter()
        .map(|(name, _)| *name)
        .chain(removed)
        .collect();
    covered.retain(|name| !name.starts_with("MSW_TEST_"));
    covered.sort_unstable();
    let mut hostile = HOSTILE.to_vec();
    hostile.sort_unstable();
    assert_eq!(covered, hostile);
    run_in_child(TEST, dir, &values, &removed);
}

/// The child of [`hostile_parent_environment_has_no_effect`], in the hostile environment.
fn hostile_child(done: &Path) {
    let dir = PathBuf::from(std::env::var_os("MSW_TEST_GIT_DIR").unwrap());
    let store_path = PathBuf::from(std::env::var_os("MSW_TEST_GIT_STORE").unwrap());
    let ids = std::fs::read_to_string(dir.join("ids")).unwrap();
    let ids: Vec<RevId> = ids.lines().map(|id| RevId(id.to_owned())).collect();
    let [base, main, side, merged, orphan, extra] = ids.as_slice() else {
        panic!("six ids expected");
    };
    let store = GitStore::open(&store_path, &git_probe(Some("2.45.0"))).unwrap();
    let fx = Fixture {
        dir: TempDir::new().unwrap(),
        source: PathBuf::new(),
        base: base.clone(),
        main: main.clone(),
        side: side.clone(),
        merged: merged.clone(),
        orphan: orphan.clone(),
    };
    let baseline = std::fs::read_to_string(dir.join("baseline")).unwrap();
    assert_eq!(observe(&store, &fx), baseline);
    assert!(matches!(
        store.resolve(&extra.0),
        Err(GitError::NotFound(_))
    ));
    assert!(!dir.join("external-ran").exists(), "the external diff ran");
    assert!(!dir.join("trace").exists(), "git traced to a file");
    // A store created here still has the files ref format.
    let created = super::store_path(&dir.join("state"), "created").unwrap();
    GitStore::create(&created, &git_probe(Some("2.45.0"))).unwrap();
    assert!(created.join("refs/heads").is_dir());
    assert!(!created.join("reftable").exists());

    let command = store.command(["status"]);
    let envs: Vec<(&OsStr, Option<&OsStr>)> = command.get_envs().collect();
    let value = |name: &str| {
        envs.iter()
            .find(|(key, _)| *key == OsStr::new(name))
            .map(|(_, value)| *value)
    };
    for name in DROPPED_ENV {
        match *name {
            "GIT_DIR" => assert_eq!(value(name), Some(Some(store_path.as_os_str()))),
            "GIT_ATTR_SOURCE" => assert!(matches!(value(name), Some(Some(_)))),
            _ => assert_eq!(value(name), Some(None), "{name} is not dropped"),
        }
    }
    for (name, fixed) in FIXED_ENV {
        assert_eq!(
            value(name),
            Some(Some(OsStr::new(fixed))),
            "{name} is not fixed"
        );
    }
    // Every hostile variable is removed or replaced, whatever the lists above hold.
    for name in HOSTILE {
        let parent = std::env::var_os(name);
        match value(name) {
            Some(None) => {}
            Some(Some(set)) => assert_ne!(Some(set), parent.as_deref(), "{name} is inherited"),
            None => panic!("{name} is inherited"),
        }
    }
    assert_eq!(value("GIT_NO_REPLACE_OBJECTS"), Some(Some(OsStr::new("1"))));
    std::fs::write(done, "done").unwrap();
}
