//! Tests of the tool directory and its server, over a trusted store filled with git plumbing.

use std::fs::Permissions;
use std::os::unix::fs::{FileTypeExt, PermissionsExt};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use mintswarm_domain::ids::RevId;
use tempfile::TempDir;

use super::wire::{self, Reply};
use super::{PREFIX, RequestRecord, RequestSink, ToolDir, ToolError, lock_new, sweep, try_lock};
use crate::exec::git::tests::{fixture_command, fixture_git, git_probe, run_fixture, text};
use crate::exec::git::{GitStore, store_path};

/// A store holding a job and what lies outside it.
struct Job {
    dir: TempDir,
    store: Arc<GitStore>,
    /// The root commit, the tag `v1`.
    root: RevId,
    /// The job's base, branch `master` and `origin/master`, a child of `root`.
    base: RevId,
    /// The job's head, a child of `root`.
    head: RevId,
    /// A child of `head`, not yet in the job.
    next: RevId,
    /// Another job's head, `refs/mintswarm/jobs/other/head`, a child of `root`.
    other: RevId,
    /// A commit no ref reaches, a child of `root`.
    dangling: RevId,
}

impl Job {
    fn new() -> Job {
        let dir = TempDir::new().unwrap();
        let path = store_path(&dir.path().join("state"), "repo").unwrap();
        let store = GitStore::create(&path, &git_probe(Some("2.45.0"))).unwrap();
        let git = |args: &[&str], input: &[u8]| {
            text(&run_fixture(fixture_command(Some(&path)), args, input))
        };
        let tree = |name: &str, data: &str| {
            let blob = git(&["hash-object", "-w", "--stdin"], data.as_bytes());
            git(
                &["mktree"],
                format!("100644 blob {blob}\t{name}\n").as_bytes(),
            )
        };
        let commit = |tree: &str, parent: Option<&RevId>, message: &str| {
            let mut command = fixture_command(Some(&path));
            command
                .env("GIT_AUTHOR_DATE", "1700000000 +0000")
                .env("GIT_COMMITTER_DATE", "1700000000 +0000");
            let mut args = vec!["commit-tree", tree, "-m", message];
            if let Some(parent) = parent {
                args.extend(["-p", &parent.0]);
            }
            RevId(text(&run_fixture(command, &args, b"")))
        };
        let root = commit(&tree("a.txt", "a\n"), None, "root");
        let base = commit(&tree("a.txt", "base\n"), Some(&root), "base");
        let head = commit(&tree("a.txt", "head\n"), Some(&root), "head");
        let next = commit(&tree("a.txt", "next\n"), Some(&head), "next");
        let other = commit(&tree("a.txt", "other\n"), Some(&root), "other");
        let dangling = commit(&tree("a.txt", "dangling\n"), Some(&root), "dangling");
        for (name, id) in [
            ("refs/heads/master", &base),
            ("refs/remotes/origin/master", &base),
            ("refs/tags/v1", &root),
            ("refs/mintswarm/jobs/this/head", &next),
            ("refs/mintswarm/jobs/other/head", &other),
        ] {
            fixture_git(&path, &["update-ref", name, &id.0]);
        }
        Job {
            dir,
            store: Arc::new(store),
            root,
            base,
            head,
            next,
            other,
            dangling,
        }
    }

    /// A stand-in client to copy into the tool directory.
    fn client(&self) -> PathBuf {
        let client = self.dir.path().join("client");
        std::fs::write(&client, "#!/bin/sh\nexit 0\n").unwrap();
        client
    }

    fn tools(&self, attempt: &str, sink: Arc<dyn RequestSink>) -> ToolDir {
        ToolDir::create(
            &unique(attempt),
            &self.client(),
            Arc::clone(&self.store),
            self.base.clone(),
            self.head.clone(),
            sink,
        )
        .unwrap()
    }
}

/// An attempt id no other test or run uses.
fn unique(name: &str) -> String {
    format!("t{}-{name}", std::process::id())
}

/// A sink that keeps every record.
#[derive(Default)]
struct Memory(Mutex<Vec<RequestRecord>>);

impl RequestSink for Memory {
    fn record(&self, request: &RequestRecord) {
        self.0.lock().unwrap().push(request.clone());
    }
}

fn nowhere() -> Arc<dyn RequestSink> {
    Arc::new(Memory::default())
}

fn call(tools: &ToolDir, args: &[&str]) -> Reply {
    let argv: Vec<Vec<u8>> = args.iter().map(|arg| arg.as_bytes().to_vec()).collect();
    wire::call(&tools.path().join("s"), &argv).unwrap()
}

fn mode(path: &Path) -> u32 {
    std::fs::symlink_metadata(path)
        .unwrap()
        .permissions()
        .mode()
        & 0o7777
}

#[test]
fn directory_socket_and_client_have_their_modes() {
    let job = Job::new();
    let tools = job.tools("modes", nowhere());
    let dir = tools.path().to_owned();
    assert_eq!(dir.parent().unwrap(), std::env::temp_dir());
    assert_eq!(mode(&dir), 0o700);
    assert_eq!(mode(&dir.join("msw-vcs")), 0o755);
    assert_eq!(mode(&dir.join("s")), 0o600);
    let socket = std::fs::symlink_metadata(dir.join("s")).unwrap();
    assert!(socket.file_type().is_socket());
    let mut names: Vec<String> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    assert_eq!(names, ["msw-vcs", "s"]);
    tools.close().unwrap();
    assert!(!dir.exists());
}

#[test]
fn unsafe_and_overlong_ids_are_refused() {
    let job = Job::new();
    let create = |attempt: &str| {
        ToolDir::create(
            attempt,
            &job.client(),
            Arc::clone(&job.store),
            job.base.clone(),
            job.head.clone(),
            nowhere(),
        )
    };
    for attempt in ["", ".hidden", "a/b", "a\0b", ".."] {
        assert!(
            matches!(create(attempt), Err(ToolError::AttemptId(_))),
            "{attempt:?}"
        );
    }
    let long = unique(&"x".repeat(120));
    assert!(matches!(create(&long), Err(ToolError::SocketPath(_))));
    assert!(
        !std::env::temp_dir()
            .join(format!("{PREFIX}{long}"))
            .exists()
    );
}

#[test]
fn an_existing_directory_or_symlink_is_refused_and_kept() {
    let job = Job::new();
    let create = |attempt: &str| {
        ToolDir::create(
            attempt,
            &job.client(),
            Arc::clone(&job.store),
            job.base.clone(),
            job.head.clone(),
            nowhere(),
        )
    };
    let attempt = unique("precreated");
    let dir = std::env::temp_dir().join(format!("{PREFIX}{attempt}"));
    std::fs::create_dir(&dir).unwrap();
    std::fs::write(dir.join("mine"), "x").unwrap();
    assert!(matches!(create(&attempt), Err(ToolError::Io { .. })));
    assert!(dir.join("mine").exists());
    std::fs::remove_dir_all(&dir).unwrap();

    let attempt = unique("symlink");
    let link = std::env::temp_dir().join(format!("{PREFIX}{attempt}"));
    let target = job.dir.path().join("target");
    std::fs::create_dir(&target).unwrap();
    std::os::unix::fs::symlink(&target, &link).unwrap();
    assert!(matches!(create(&attempt), Err(ToolError::Io { .. })));
    assert!(std::fs::symlink_metadata(&link).unwrap().is_symlink());
    assert_eq!(std::fs::read_dir(&target).unwrap().count(), 0);
    std::fs::remove_file(&link).unwrap();
}

#[test]
fn the_directory_is_removed_on_drop() {
    let job = Job::new();
    let tools = job.tools("drop", nowhere());
    let dir = tools.path().to_owned();
    assert_eq!(call(&tools, &["log"]).exit, 0);
    drop(tools);
    assert!(!dir.exists());
}

#[test]
fn a_killed_stage_or_client_leaves_nothing() {
    let job = Job::new();
    let tools = job.tools("killed", nowhere());
    let dir = tools.path().to_owned();

    // a stage process with the tool directory on its PATH, killed with SIGKILL
    let mut stage = std::process::Command::new("sleep")
        .arg("30")
        .env("PATH", format!("{}:/bin:/usr/bin", dir.display()))
        .spawn()
        .unwrap();
    stage.kill().unwrap();
    stage.wait().unwrap();

    // a client that dies mid-frame: half a request, then the connection drops
    let mut frame = Vec::new();
    wire::write_request(&mut frame, &[b"log".to_vec()]).unwrap();
    {
        let mut stream = UnixStream::connect(dir.join("s")).unwrap();
        std::io::Write::write_all(&mut stream, &frame[..frame.len() - 1]).unwrap();
    }
    // the server still answers
    assert_eq!(call(&tools, &["log"]).exit, 0);

    tools.close().unwrap();
    assert!(!dir.exists());
}

#[test]
fn sweep_removes_stale_directories_and_never_follows_a_symlink() {
    let job = Job::new();
    let temp = job.dir.path().join("tmp");
    let stale = temp.join(format!("{PREFIX}stale"));
    std::fs::create_dir_all(&stale).unwrap();
    std::fs::write(stale.join("s"), "").unwrap();
    std::fs::set_permissions(&stale, Permissions::from_mode(0o700)).unwrap();
    let target = job.dir.path().join("target");
    std::fs::create_dir(&target).unwrap();
    std::fs::write(target.join("kept"), "x").unwrap();
    let link = temp.join(format!("{PREFIX}link"));
    std::os::unix::fs::symlink(&target, &link).unwrap();
    let other = temp.join("other");
    std::fs::create_dir(&other).unwrap();

    assert_eq!(sweep(&temp), 1);
    assert!(!stale.exists());
    assert!(std::fs::symlink_metadata(&link).unwrap().is_symlink());
    assert!(target.join("kept").exists());
    assert!(other.exists());
    assert_eq!(sweep(&job.dir.path().join("missing")), 0);
}

#[test]
fn a_live_directory_survives_a_sweep() {
    let job = Job::new();
    // a live tool directory holds its lock, so a sweep, of any runner, cannot take it
    let tools = job.tools("live", nowhere());
    assert!(try_lock(tools.path()).unwrap().is_none());

    // a sweep skips a held directory and removes the one nobody holds
    let temp = job.dir.path().join("tmp");
    let held = temp.join(format!("{PREFIX}held"));
    let stale = temp.join(format!("{PREFIX}stale"));
    std::fs::create_dir_all(&held).unwrap();
    std::fs::write(held.join("s"), "").unwrap();
    std::fs::create_dir(&stale).unwrap();
    let lock = try_lock(&held).unwrap().unwrap();
    assert_eq!(sweep(&temp), 1);
    assert!(held.join("s").exists());
    assert!(!stale.exists());

    // once released, it is stale too
    drop(lock);
    assert_eq!(sweep(&temp), 1);
    assert!(!held.exists());

    let dir = tools.path().to_owned();
    tools.close().unwrap();
    assert!(!dir.exists());
}

#[test]
fn a_directory_a_sweep_took_first_is_refused_and_kept() {
    let job = Job::new();
    let temp = job.dir.path().join("tmp");
    std::fs::create_dir(&temp).unwrap();

    // the sweep holds the lock between the mkdir and the lock
    let taken = temp.join(format!("{PREFIX}taken"));
    std::fs::create_dir(&taken).unwrap();
    let lock = try_lock(&taken).unwrap().unwrap();
    assert!(matches!(lock_new(&taken), Err(ToolError::Held(_))));
    assert!(taken.exists());
    drop(lock);

    // the sweep removed it before the lock
    let gone = temp.join(format!("{PREFIX}gone"));
    assert!(matches!(lock_new(&gone), Err(ToolError::Held(_))));
    assert!(!gone.exists());

    // nobody took it: it is locked
    let free = temp.join(format!("{PREFIX}free"));
    std::fs::create_dir(&free).unwrap();
    let _lock = lock_new(&free).unwrap();
    assert!(try_lock(&free).unwrap().is_none());
}

#[test]
fn refs_outside_the_job_are_refused_and_other_commits_unknown() {
    let job = Job::new();
    let tools = job.tools("scope", nowhere());
    for (args, arg) in [
        (["log", "master"], "master"),
        (["show", "v1"], "v1"),
        (["log", "origin/master"], "origin/master"),
        (["show", "HEAD"], "HEAD"),
        (
            ["log", "refs/mintswarm/jobs/other/head"],
            "refs/mintswarm/jobs/other/head",
        ),
        (["log", "head~1"], "head~1"),
    ] {
        let reply = call(&tools, &args);
        assert_eq!(reply.exit, 2, "{args:?}");
        assert!(reply.stdout.is_empty());
        assert_eq!(
            reply.stderr,
            format!("msw-vcs: refused revision: {arg}\n").into_bytes()
        );
    }
    for id in [&job.other, &job.dangling, &job.next] {
        let reply = call(&tools, &["show", &id.0]);
        assert_eq!(reply.exit, 3, "{}", id.0);
        assert_eq!(
            reply.stderr,
            format!("msw-vcs: unknown revision or path: {}\n", id.0).into_bytes()
        );
    }
    for id in [&job.root, &job.base, &job.head] {
        assert_eq!(call(&tools, &["show", &id.0]).exit, 0, "{}", id.0);
    }
}

#[test]
fn every_request_reaches_the_sink_in_order() {
    let job = Job::new();
    let sink = Arc::new(Memory::default());
    let tools = job.tools("sink", Arc::clone(&sink) as Arc<dyn RequestSink>);
    let calls: [&[&str]; 4] = [&["log"], &["log", "master"], &["show", &job.other.0], &[]];
    for args in calls {
        call(&tools, args);
    }
    let attempt = unique("sink");
    let records = sink.0.lock().unwrap().clone();
    let expected: Vec<RequestRecord> = calls
        .iter()
        .zip([0, 2, 3, 2])
        .map(|(args, exit)| RequestRecord {
            attempt: attempt.clone(),
            argv: args.iter().map(|arg| arg.as_bytes().to_vec()).collect(),
            exit,
        })
        .collect();
    assert_eq!(records, expected);
}

#[test]
fn set_head_widens_the_scope() {
    let job = Job::new();
    let tools = job.tools("head", nowhere());
    assert_eq!(call(&tools, &["show", &job.next.0]).exit, 3);
    tools.set_head(job.next.clone());
    assert_eq!(call(&tools, &["show", &job.next.0]).exit, 0);
    let log = String::from_utf8(call(&tools, &["log"]).stdout).unwrap();
    let commits: Vec<&str> = log
        .lines()
        .filter_map(|line| line.strip_prefix("commit "))
        .collect();
    assert_eq!(commits, [job.next.0.as_str(), job.head.0.as_str()]);
}
