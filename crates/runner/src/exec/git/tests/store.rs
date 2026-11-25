//! Creating and opening the store, the git version, and fetch.

use std::ffi::OsStr;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Stdio;

use mintswarm_domain::ids::RevId;
use mintswarm_domain::plane::Credential;
use mintswarm_domain::probe::ProbeValue;
use mintswarm_domain::vcs::{RefOrId, Remote, Vcs};

use super::super::fetch::credential_command;
use super::super::{GIT_REQUIREMENT, GitError, GitStore, store_path};
use super::{Fixture, fixture_command, fixture_git, git_probe, run_fixture, script, text};

fn mode(path: &Path) -> u32 {
    std::fs::metadata(path).unwrap().permissions().mode() & 0o7777
}

fn chmod(path: &Path, mode: u32) {
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
}

fn open(path: &Path) -> Result<GitStore, GitError> {
    GitStore::open(path, &git_probe(Some("2.45.0")))
}

#[test]
fn create_is_bare_with_mode_0700() {
    let fx = Fixture::new();
    let store = fx.store("created");
    let expected = fx.dir.path().join("state/repos/created.git");
    assert_eq!(store.path(), expected);
    assert_eq!(mode(&expected), 0o700);
    assert_eq!(
        text(&fixture_git(
            &expected,
            &["rev-parse", "--is-bare-repository"]
        )),
        "true"
    );
    assert!(store.warnings().is_empty());
    // Creating over an existing store fails.
    assert!(matches!(
        GitStore::create(&expected, &git_probe(Some("2.45.0"))),
        Err(GitError::Io { .. })
    ));
}

#[test]
fn store_path_refuses_names_that_are_not_one_component() {
    let state = Path::new("/state");
    assert_eq!(
        store_path(state, "mintswarm").unwrap(),
        Path::new("/state/repos/mintswarm.git")
    );
    for name in ["", ".", "..", ".hidden", "a/b", "/abs", "a\0b"] {
        assert!(
            matches!(store_path(state, name), Err(GitError::StoreName(_))),
            "{name:?}"
        );
    }
}

/// A clone of the fixture's source made with `args`, as a store would be.
fn clone(fx: &Fixture, name: &str, args: &[&str]) -> std::path::PathBuf {
    let dest = fx.dir.path().join(name);
    let url = format!("file://{}", fx.source.display());
    let mut all: Vec<&OsStr> = vec![
        OsStr::new("clone"),
        OsStr::new("--bare"),
        OsStr::new("--quiet"),
    ];
    all.extend(args.iter().map(OsStr::new));
    all.extend([OsStr::new(&url), dest.as_os_str()]);
    run_fixture(fixture_command(None), &all, b"");
    chmod(&dest, 0o700);
    dest
}

#[test]
fn open_refuses_shallow() {
    let fx = Fixture::new();
    let shallow = clone(&fx, "shallow.git", &["--depth", "1"]);
    assert!(matches!(open(&shallow), Err(GitError::Shallow(_))));
    let full = clone(&fx, "full.git", &[]);
    open(&full).unwrap();
}

#[test]
fn open_refuses_partial() {
    let fx = Fixture::new();
    fixture_git(&fx.source, &["config", "uploadpack.allowFilter", "true"]);
    let partial = clone(&fx, "partial.git", &["--filter=blob:none"]);
    assert!(matches!(open(&partial), Err(GitError::Partial { .. })));

    let settings: [&[(&str, &str)]; 3] = [
        &[
            ("core.repositoryformatversion", "1"),
            ("extensions.partialClone", "origin"),
        ],
        &[("remote.origin.promisor", "true")],
        &[("remote.origin.partialCloneFilter", "blob:none")],
    ];
    for (n, settings) in settings.into_iter().enumerate() {
        let store = fx.store(&format!("partial-{n}"));
        for (key, value) in settings {
            fixture_git(store.path(), &["config", key, value]);
        }
        assert!(
            matches!(open(store.path()), Err(GitError::Partial { .. })),
            "{settings:?}"
        );
    }

    let store = fx.store("promisor-false");
    fixture_git(store.path(), &["config", "remote.origin.promisor", "false"]);
    open(store.path()).unwrap();
}

#[test]
fn open_refuses_wrong_mode_and_alternates() {
    let fx = Fixture::new();
    let store = fx.store("checked");
    let path = store.path();

    chmod(path, 0o755);
    assert!(matches!(
        open(path),
        Err(GitError::Mode { mode: 0o755, .. })
    ));
    chmod(path, 0o700);
    open(path).unwrap();

    let alternates = path.join("objects/info/alternates");
    std::fs::create_dir_all(alternates.parent().unwrap()).unwrap();
    std::fs::write(
        &alternates,
        format!("{}\n", fx.source.join("objects").display()),
    )
    .unwrap();
    assert!(matches!(open(path), Err(GitError::Alternates(_))));
    std::fs::write(&alternates, "").unwrap();
    open(path).unwrap();

    let attributes = path.join("info/attributes");
    std::fs::create_dir_all(attributes.parent().unwrap()).unwrap();
    std::fs::write(&attributes, "* -diff\n").unwrap();
    assert!(matches!(open(path), Err(GitError::Attributes(_))));
    std::fs::remove_file(&attributes).unwrap();
    std::fs::create_dir(&attributes).unwrap();
    assert!(matches!(open(path), Err(GitError::Attributes(_))));

    let plain = fx.dir.path().join("plain");
    std::fs::create_dir(&plain).unwrap();
    chmod(&plain, 0o700);
    assert!(matches!(open(&plain), Err(GitError::NotBare(_))));
    assert!(matches!(
        open(&fx.dir.path().join("missing")),
        Err(GitError::Io { .. })
    ));
}

#[test]
fn open_checks_git_version() {
    let fx = Fixture::new();
    let path = fx.store("versions").path().to_owned();

    let error = GitStore::open(&path, &git_probe(Some("2.44.2"))).unwrap_err();
    let GitError::Version { requirement, .. } = &error else {
        panic!("not a version error: {error:?}");
    };
    assert_eq!(requirement, ">= 2.45");
    assert_eq!(GIT_REQUIREMENT, ">= 2.45");
    assert!(error.to_string().contains(">= 2.45"));

    assert!(
        GitStore::open(&path, &git_probe(Some("2.45.0")))
            .unwrap()
            .warnings()
            .is_empty()
    );

    let unknown = GitStore::open(&path, &git_probe(None)).unwrap();
    assert_eq!(unknown.warnings().len(), 1);
    assert!(unknown.warnings()[0].contains(">= 2.45"));

    let mut no_version = git_probe(None);
    no_version.value = ProbeValue::Known {
        version: None,
        facts: Default::default(),
    };
    assert_eq!(
        GitStore::open(&path, &no_version).unwrap().warnings().len(),
        1
    );

    let mut other = git_probe(Some("2.50.0"));
    other.namespace = "tool.bash".parse().unwrap();
    assert!(matches!(
        GitStore::open(&path, &other),
        Err(GitError::Version { .. })
    ));

    // The version is checked before anything is created.
    let refused = fx.dir.path().join("state/repos/refused.git");
    assert!(matches!(
        GitStore::create(&refused, &git_probe(Some("2.44.2"))),
        Err(GitError::Version { .. })
    ));
    assert!(!refused.exists());
}

fn credential(token: &str) -> Credential {
    Credential {
        token: token.to_owned(),
        expires_at: "2026-11-25T01:00:00Z".to_owned(),
        permissions: vec!["contents:read".to_owned()],
    }
}

/// Every file under `dir`, recursively.
fn files(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let entry = entry.unwrap();
        let kind = entry.file_type().unwrap();
        if kind.is_dir() {
            files(&entry.path(), out);
        } else {
            out.push(entry.path());
        }
    }
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

#[test]
fn fetch_keeps_credential_out_of_argv_config_and_urls() {
    let fx = Fixture::new();
    let mut store = fx.store("fetched");
    let log = fx.dir.path().join("argv.log");
    let wrapper = fx.dir.path().join("git-wrapper");
    script(
        &wrapper,
        &format!(
            "for a in \"$@\"; do printf '%s\\n' \"$a\"; done >> '{}'\nexec git \"$@\"",
            log.display()
        ),
    );
    store.set_program(&wrapper);
    let token = format!("msw-test-token-{}", std::process::id());
    let wants = [
        RefOrId::Ref("refs/heads/side".to_owned()),
        RefOrId::Ref("refs/tags/v1".to_owned()),
        RefOrId::Id(fx.main.clone()),
    ];
    let report = store
        .fetch(&fx.remote(), &wants, Some(&credential(&token)))
        .unwrap();
    let fetched: Vec<(&str, &RevId)> = report
        .refs
        .iter()
        .map(|r| (r.name.as_str(), &r.id))
        .collect();
    let id_ref = format!("refs/mintswarm/fetched/ids/{}", fx.main.0);
    assert_eq!(
        fetched,
        [
            ("refs/mintswarm/fetched/origin/heads/side", &fx.side),
            ("refs/mintswarm/fetched/origin/tags/v1", &fx.base),
            (id_ref.as_str(), &fx.main),
        ]
    );

    let argv = std::fs::read(&log).unwrap();
    assert!(contains(&argv, b"fetch"), "the wrapper logged no fetch");
    assert!(!contains(&argv, token.as_bytes()), "the token is in argv");
    let mut all = Vec::new();
    files(store.path(), &mut all);
    for file in all {
        assert!(
            !contains(&std::fs::read(&file).unwrap(), token.as_bytes()),
            "the token is in {}",
            file.display()
        );
    }
    let config = fixture_git(store.path(), &["config", "--local", "--list", "-z"]);
    for entry in config.split(|&byte| byte == 0) {
        assert!(
            !entry.starts_with(b"remote.") && !entry.starts_with(b"url."),
            "{}",
            String::from_utf8_lossy(entry)
        );
    }
    assert!(!store.path().join("FETCH_HEAD").exists());
}

#[test]
fn credential_helper_answers_only_github_https() {
    let fx = Fixture::new();
    let store = fx.store("helper");
    let token = format!("msw-helper-token-{}", std::process::id());
    let password = format!("password={token}\n");
    for (request, answered) in [
        ("protocol=https\nhost=github.com\n\n", true),
        ("protocol=https\nhost=example.com\n\n", false),
        ("protocol=http\nhost=github.com\n\n", false),
    ] {
        let mut command =
            credential_command(&store, Some(&credential(&token)), &["credential", "fill"]);
        command.stdin(Stdio::piped());
        let mut child = command.spawn().unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(request.as_bytes())
            .unwrap();
        let output = child.wait_with_output().unwrap();
        assert_eq!(
            contains(&output.stdout, password.as_bytes()),
            answered,
            "{request:?}"
        );
        assert!(!contains(&output.stderr, token.as_bytes()));
        if !answered {
            assert!(!contains(&output.stdout, token.as_bytes()));
        }
    }
    // Without a credential no helper answers, even for github.com.
    let mut command = credential_command(&store, None, &["credential", "fill"]);
    command.stdin(Stdio::piped());
    let mut child = command.spawn().unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"protocol=https\nhost=github.com\n\n")
        .unwrap();
    assert!(!contains(
        &child.wait_with_output().unwrap().stdout,
        b"password="
    ));
}

#[test]
fn fetch_refuses_bad_urls() {
    let fx = Fixture::new();
    let store = fx.store("urls");
    let wants = [RefOrId::Ref("refs/heads/side".to_owned())];
    for url in [
        "https://user:secret@github.com/o/r.git",
        "https://user@github.com/o/r.git",
        "http://github.com/o/r.git",
        "ssh://git@github.com/o/r.git",
        "git@github.com:o/r.git",
        "https://github.com/o/r.git?x=1",
        "https://github.com/o/r.git#frag",
        "https:///o/r.git",
        "https://github.com/o r.git",
        "ext::sh -c touch% /tmp/x",
        "-uhttps://github.com/o/r.git",
    ] {
        let remote = Remote {
            name: "origin".to_owned(),
            url: url.to_owned(),
        };
        assert!(
            matches!(store.fetch(&remote, &wants, None), Err(GitError::Url(_))),
            "{url}"
        );
    }
    // A store that does not allow the test-only remote refuses file://.
    let strict = GitStore::open(store.path(), &git_probe(Some("2.45.0"))).unwrap();
    assert!(matches!(
        strict.fetch(&fx.remote(), &wants, None),
        Err(GitError::Url(_))
    ));
}

#[test]
fn fetch_refuses_bad_names() {
    let fx = Fixture::new();
    let store = fx.store("names");
    for name in [
        "side",
        "HEAD",
        "refs/heads/",
        "refs/remotes/origin/side",
        "refs/heads/a..b",
        "refs/heads/x:y",
        "+refs/heads/side",
        "refs/heads/*",
        "refs/heads/side.lock",
        "refs/heads/-x/../y",
    ] {
        let wants = [RefOrId::Ref(name.to_owned())];
        assert!(
            matches!(
                store.fetch(&fx.remote(), &wants, None),
                Err(GitError::RefName(_))
            ),
            "{name}"
        );
    }
    for remote in ["", "ids", "..", "-x", "a/b", "a b"] {
        let remote = Remote {
            name: remote.to_owned(),
            url: fx.remote().url,
        };
        let wants = [RefOrId::Ref("refs/heads/side".to_owned())];
        assert!(matches!(
            store.fetch(&remote, &wants, None),
            Err(GitError::RefName(_))
        ));
    }
    for id in ["abc", "HEAD", &fx.main.0.to_uppercase()] {
        let wants = [RefOrId::Id(RevId(id.to_owned()))];
        assert!(
            matches!(
                store.fetch(&fx.remote(), &wants, None),
                Err(GitError::RevisionSyntax(_))
            ),
            "{id}"
        );
    }
    assert!(
        store
            .fetch(&fx.remote(), &[], None)
            .unwrap()
            .refs
            .is_empty()
    );
}
