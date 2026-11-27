//! The plan floor's fresh object store, against a job repository with a working tree that a
//! branch under review could have tampered with: the cases of `plan-approval/selftest.py`'s
//! `check_object_store`.

use std::ffi::OsString;
use std::os::unix::fs::PermissionsExt;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};

use mintswarm_domain::vcs::Vcs;
use tempfile::TempDir;

use super::super::read;
use super::super::{FreshStore, FreshStoreError, STORE_PREFIX};
use super::{CHILD, fixture_command, git_probe, path, run_fixture, run_in_child, text};

const CONFIG: &[u8] = b".mintswarm/config.yaml";

/// A job repository with a working tree: `root` ("init"), `parent` ("feat(cli): first", which
/// adds the strict config) and `base` ("chore: base"), then `looser` on top, whose config is
/// loose.
struct Job {
    dir: TempDir,
    repo: PathBuf,
    root: String,
    parent: String,
    base: String,
    looser: String,
}

impl Job {
    fn new() -> Job {
        Job::named("repo")
    }

    fn named(name: &str) -> Job {
        let dir = TempDir::new().unwrap();
        Job::in_dir(dir, name)
    }

    fn in_dir(dir: TempDir, name: &str) -> Job {
        let repo = dir.path().join(name);
        run_fixture(
            fixture_command(None),
            &[
                OsString::from("init"),
                OsString::from("-q"),
                OsString::from("-b"),
                OsString::from("master"),
                repo.clone().into_os_string(),
            ],
            b"",
        );
        let mut job = Job {
            dir,
            repo,
            root: String::new(),
            parent: String::new(),
            base: String::new(),
            looser: String::new(),
        };
        job.root = job.commit("init", None);
        job.parent = job.commit("feat(cli): first", Some(b"strict\n"));
        job.base = job.commit("chore: base", None);
        job.looser = job.commit("loose", Some(b"loose\n"));
        job
    }

    /// Fixture git in the working tree; its output without the final newline.
    fn git(&self, args: &[&str]) -> String {
        let mut command = fixture_command(None);
        command.current_dir(&self.repo);
        text(&run_fixture(command, args, b""))
    }

    /// A commit, with the config written first when given.
    fn commit(&self, message: &str, config: Option<&[u8]>) -> String {
        if let Some(config) = config {
            let file = self.repo.join(".mintswarm/config.yaml");
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(&file, config).unwrap();
            self.git(&["add", ".mintswarm/config.yaml"]);
        }
        self.git(&["commit", "-q", "--allow-empty", "-m", message]);
        self.git(&["rev-parse", "HEAD"])
    }

    fn git_dir(&self) -> PathBuf {
        self.repo.join(".git")
    }

    fn fresh(&self, reference: &str) -> Result<FreshStore, FreshStoreError> {
        FreshStore::from_checkout(&self.repo, reference, &git_probe(Some("2.45.0")))
    }

    /// Overwrite the loose object of the base's config with the loose object of the looser
    /// config; the original bytes, to restore.
    fn forge_config(&self) -> (PathBuf, Vec<u8>) {
        let loose = |id: &str| self.git_dir().join("objects").join(&id[..2]).join(&id[2..]);
        let strict = loose(&self.git(&[
            "rev-parse",
            &format!("{}:.mintswarm/config.yaml", self.base),
        ]));
        let forged = loose(&self.git(&[
            "rev-parse",
            &format!("{}:.mintswarm/config.yaml", self.looser),
        ]));
        let original = std::fs::read(&strict).unwrap();
        std::fs::set_permissions(&strict, std::fs::Permissions::from_mode(0o644)).unwrap();
        std::fs::copy(&forged, &strict).unwrap();
        (strict, original)
    }
}

/// The config at the fresh store's base.
fn config(fresh: &FreshStore) -> Vec<u8> {
    fresh
        .store()
        .read_blob(&fresh.base().commit, &path(CONFIG))
        .unwrap()
        .unwrap()
}

/// The raw subjects of the fresh store's base.
fn subjects(fresh: &FreshStore) -> String {
    String::from_utf8(read::subjects(fresh.store(), &fresh.base().commit).unwrap()).unwrap()
}

/// The `msw-floor-objects-` entries of `dir`.
fn stores_in(dir: &Path) -> Vec<String> {
    std::fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with(STORE_PREFIX))
        .collect()
}

#[test]
fn an_ordinary_base_fetches_and_peels() {
    let job = Job::new();
    let fresh = job.fresh(&job.base).unwrap();
    assert_eq!(fresh.base().reference, job.base);
    assert_eq!(fresh.base().commit.0, job.base);
    assert_eq!(config(&fresh), b"strict\n");
    assert_eq!(subjects(&fresh), "chore: base\0feat(cli): first\0init\0");
    let dir = fresh.dir().to_owned();
    assert!(
        dir.file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with(STORE_PREFIX)
    );
    assert!(fresh.store().path().starts_with(&dir));
    drop(fresh);
    assert!(!dir.exists());

    let by_name = job.fresh("master~1").unwrap();
    assert_eq!(by_name.base().reference, "master~1");
    assert_eq!(by_name.base().commit.0, job.base);
}

#[test]
fn an_annotated_tag_peels_and_a_tree_is_refused() {
    let job = Job::new();
    job.git(&["tag", "-a", "v1", "-m", "v1", &job.base]);
    let fresh = job.fresh("v1").unwrap();
    assert_eq!(fresh.base().commit.0, job.base);
    assert_eq!(fresh.base().reference, "v1");

    let tree = job.git(&["rev-parse", &format!("{}^{{tree}}", job.base)]);
    let refused = job.fresh(&tree).unwrap_err();
    assert!(
        matches!(refused, FreshStoreError::Unresolved(_)),
        "{refused:?}"
    );
    assert_eq!(
        refused.to_string(),
        format!("base ref '{tree}' does not resolve to a commit")
    );
    let missing = job.fresh("no-such-ref").unwrap_err();
    assert_eq!(
        missing.unusable().message(),
        "base ref 'no-such-ref' does not resolve to a commit"
    );
}

#[test]
fn replace_refs_and_grafts_do_not_reach_the_store() {
    let job = Job::new();
    // a replace ref on the base, which plain git follows
    job.git(&[
        "update-ref",
        &format!("refs/replace/{}", job.base),
        &job.looser,
    ]);
    assert_eq!(
        job.git(&["show", &format!("{}:.mintswarm/config.yaml", job.base)]),
        "loose"
    );
    assert_eq!(config(&job.fresh(&job.base).unwrap()), b"strict\n");
    job.git(&["update-ref", "-d", &format!("refs/replace/{}", job.base)]);

    // info/grafts cutting the base from its parents
    let grafts = job.git_dir().join("info/grafts");
    std::fs::create_dir_all(grafts.parent().unwrap()).unwrap();
    std::fs::write(&grafts, format!("{}\n", job.base)).unwrap();
    assert_eq!(
        job.git(&["log", "--format=%s", &job.base]),
        "chore: base",
        "plain git reads the grafts"
    );
    assert!(subjects(&job.fresh(&job.base).unwrap()).contains("feat(cli): first"));
    std::fs::remove_file(&grafts).unwrap();

    // git replace --graft
    job.git(&["replace", "--graft", &job.base]);
    assert_eq!(job.git(&["log", "--format=%s", &job.base]), "chore: base");
    assert!(subjects(&job.fresh(&job.base).unwrap()).contains("feat(cli): first"));
}

#[test]
fn a_forged_loose_object_fails_the_fetch() {
    let job = Job::new();
    let (forged, _) = job.forge_config();
    assert_eq!(
        job.git(&["show", &format!("{}:.mintswarm/config.yaml", job.base)]),
        "loose"
    );
    let refused = job.fresh(&job.base).unwrap_err();
    assert!(
        matches!(refused, FreshStoreError::Fetch { .. }),
        "{refused:?}"
    );
    assert!(refused.to_string().contains("fetch"), "{refused}");
    assert!(forged.exists());
}

#[test]
fn a_forged_shallow_file_is_refused_naming_the_fetch() {
    let job = Job::new();
    std::fs::write(job.git_dir().join("shallow"), format!("{}\n", job.parent)).unwrap();
    assert_eq!(job.git(&["rev-parse", "--is-shallow-repository"]), "true");
    let refused = job.fresh(&job.base).unwrap_err();
    let message = refused.to_string();
    assert!(message.contains("the fetch"), "{message}");
    assert!(!message.contains("hint:"), "{message}");
    assert!(!message.contains(&job.root), "{message}");
}

#[test]
fn a_partial_clone_is_refused() {
    let job = Job::new();
    job.git(&["config", "extensions.partialClone", "origin"]);
    let refused = job.fresh(&job.base).unwrap_err();
    assert!(
        matches!(refused, FreshStoreError::Partial(_)),
        "{refused:?}"
    );
    assert!(refused.to_string().contains("partial clone"), "{refused}");
}

#[test]
fn a_percent_in_the_path_reads_the_right_repository() {
    let job = Job::named("pct%41x");
    // a decoy at the path git would decode the URL to
    run_fixture(
        fixture_command(None),
        &[
            OsString::from("init"),
            OsString::from("-q"),
            job.dir.path().join("pctAx").into_os_string(),
        ],
        b"",
    );
    let fresh = job.fresh(&job.base).unwrap();
    assert_eq!(config(&fresh), b"strict\n");
}

#[test]
fn quote_encodes_as_urllib_parse_quote() {
    assert_eq!(
        super::super::fresh_store::quote(b"/a b/%41/~x_y.z-w"),
        "/a%20b/%2541/~x_y.z-w"
    );
    assert_eq!(
        super::super::fresh_store::quote("/caf\u{e9}".as_bytes()),
        "/caf%C3%A9"
    );
}

/// Run in a child with `TMPDIR` at a directory of the test's own, and `GIT_DIR` and
/// `GIT_OBJECT_DIRECTORY` at a decoy: after a success, after each refusal and after a panic, no
/// `msw-floor-objects-` entry is left there, and the decoy is never read.
#[test]
fn nothing_is_left_in_tmpdir_and_a_decoy_git_dir_is_ignored() {
    const TEST: &str =
        "exec::git::tests::fresh_store::nothing_is_left_in_tmpdir_and_a_decoy_git_dir_is_ignored";
    if std::env::var_os(CHILD).is_some() {
        child(Path::new(&std::env::var_os(CHILD).unwrap()));
        return;
    }
    let dir = TempDir::new().unwrap();
    let tmp = dir.path().join("tmp");
    std::fs::create_dir(&tmp).unwrap();
    let decoy = dir.path().join("decoy");
    run_fixture(
        fixture_command(None),
        &[
            OsString::from("init"),
            OsString::from("-q"),
            decoy.clone().into_os_string(),
        ],
        b"",
    );
    let envs = [
        ("TMPDIR", tmp.clone().into_os_string()),
        ("GIT_DIR", decoy.join(".git").into_os_string()),
        (
            "GIT_OBJECT_DIRECTORY",
            decoy.join(".git/objects").into_os_string(),
        ),
    ];
    run_in_child(TEST, dir.path(), &envs, &[]);
    assert!(stores_in(&tmp).is_empty());
}

/// The child of [`nothing_is_left_in_tmpdir_and_a_decoy_git_dir_is_ignored`].
fn child(done: &Path) {
    let tmp = PathBuf::from(std::env::var_os("TMPDIR").unwrap());
    // the fixture lives outside TMPDIR, beside the marker file
    let job = Job::in_dir(
        tempfile::Builder::new()
            .prefix("job-")
            .tempdir_in(done.parent().unwrap())
            .unwrap(),
        "repo",
    );
    let empty = || assert!(stores_in(&tmp).is_empty(), "{:?}", stores_in(&tmp));

    let fresh = job.fresh(&job.base).unwrap();
    assert_eq!(config(&fresh), b"strict\n");
    assert_eq!(stores_in(&tmp).len(), 1);
    assert!(fresh.dir().starts_with(&tmp));
    drop(fresh);
    empty();

    assert!(job.fresh("no-such-ref").is_err());
    empty();

    let caught = catch_unwind(AssertUnwindSafe(|| {
        let fresh = job.fresh(&job.base).unwrap();
        assert_eq!(stores_in(&tmp).len(), 1);
        if fresh.base().commit.0 == job.base {
            panic!("a panic while the store is held");
        }
    }));
    assert!(caught.is_err());
    empty();

    std::fs::write(job.git_dir().join("shallow"), format!("{}\n", job.parent)).unwrap();
    assert!(job.fresh(&job.base).is_err());
    empty();
    std::fs::remove_file(job.git_dir().join("shallow")).unwrap();

    let (forged, original) = job.forge_config();
    assert!(matches!(
        job.fresh(&job.base),
        Err(FreshStoreError::Fetch { .. })
    ));
    empty();
    std::fs::set_permissions(&forged, std::fs::Permissions::from_mode(0o644)).unwrap();
    std::fs::write(&forged, original).unwrap();

    job.git(&["config", "extensions.partialClone", "origin"]);
    assert!(matches!(
        job.fresh(&job.base),
        Err(FreshStoreError::Partial(_))
    ));
    empty();

    std::fs::write(done, "done").unwrap();
}
