//! The runner directory: `join`, `run` and `add-service` in the current directory, and the
//! service files they render.

#![cfg(test)]

use std::io::{BufRead, BufReader};
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

use mintswarm_config::load_runner_config;
use mintswarm_runner::exec::store::{OutboxMode, Store, StoreError, SystemClock};
use mintswarm_runner::runner_dir::{
    self, RunnerDirError, ServiceKind, add_service, claim, instructions, render_plist, render_unit,
    service_name,
};
use tempfile::TempDir;

/// The binary under test.
const BIN: &str = env!("CARGO_BIN_EXE_mintswarm-runner");

/// The environment variable a runner must not read, built in two halves so that a search for a
/// read of it finds none here.
const XDG_VAR: &str = concat!("XDG_", "STATE_HOME");

/// How long a test waits for `run` to report that it holds the runner directory.
const READY_BOUND: Duration = Duration::from_secs(30);

/// The line `run` writes to standard error once it holds the store.
const READY_LINE: &str = "mintswarm-runner: running in ";

/// A temporary directory and, inside it, a fresh directory `runner` of `mode`, both canonical.
fn runner_dir_with_mode(mode: u32) -> (TempDir, PathBuf) {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().canonicalize().unwrap().join("runner");
    std::fs::DirBuilder::new().mode(mode).create(&dir).unwrap();
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(mode)).unwrap();
    (tmp, dir)
}

/// The permission bits of `path`.
fn mode_of(path: &Path) -> u32 {
    std::fs::symlink_metadata(path)
        .unwrap()
        .permissions()
        .mode()
        & 0o7777
}

/// One top-level entry of a directory: its name, its mode and, for a file, its content.
type Entry = (String, u32, Option<Vec<u8>>);

/// The mode of `dir` and its top-level entries, sorted by name.
fn snapshot(dir: &Path) -> (u32, Vec<Entry>) {
    let mut entries: Vec<Entry> = std::fs::read_dir(dir)
        .unwrap()
        .map(|entry| {
            let path = entry.unwrap().path();
            let content = path.is_file().then(|| std::fs::read(&path).unwrap());
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            (name, mode_of(&path), content)
        })
        .collect();
    entries.sort();
    (mode_of(dir), entries)
}

/// The names in `dir`, sorted.
fn names_in(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

/// A child process that is killed and reaped when dropped, unless it has exited already.
struct Running(Child);

impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Start `mintswarm-runner run` in `dir`, with `home` as `HOME` and the XDG state home, and
/// a receiver of its standard error lines.
fn spawn_run(dir: &Path, home: &Path) -> (Running, Receiver<String>) {
    let mut child = Command::new(BIN)
        .arg("run")
        .current_dir(dir)
        .env("HOME", home)
        .env(XDG_VAR, home)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let stderr = child.stderr.take().unwrap();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stderr).lines() {
            let Ok(line) = line else { break };
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    (Running(child), rx)
}

/// Wait, at most [`READY_BOUND`], for `run` to report that it holds `dir`.
fn wait_ready(lines: &Receiver<String>, dir: &Path) {
    let deadline = Instant::now() + READY_BOUND;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match lines.recv_timeout(left) {
            Ok(line) if line.starts_with(READY_LINE) => {
                assert_eq!(Path::new(&line[READY_LINE.len()..]), dir, "{line}");
                return;
            }
            Ok(_) => {}
            Err(RecvTimeoutError::Timeout) => panic!("run did not report ready within the bound"),
            Err(RecvTimeoutError::Disconnected) => panic!("run exited before it was ready"),
        }
    }
}

/// Send SIGTERM to `running` and wait for it to exit.
fn terminate(mut running: Running) {
    let pid = running.0.id().to_string();
    let status = Command::new("kill").args(["-TERM", &pid]).status().unwrap();
    assert!(status.success(), "kill: {status}");
    let status = running.0.wait().unwrap();
    assert!(!status.success(), "run ended by itself: {status}");
}

#[test]
fn join_and_run_leave_home_untouched() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let dir = root.join("runner");
    let home = root.join("home");
    std::fs::create_dir(&dir).unwrap();
    std::fs::create_dir(&home).unwrap();

    let output = Command::new(BIN)
        .arg("join")
        .current_dir(&dir)
        .env("HOME", &home)
        .env(XDG_VAR, &home)
        .output()
        .unwrap();
    assert!(output.status.success(), "join: {output:?}");

    let (running, lines) = spawn_run(&dir, &home);
    wait_ready(&lines, &dir);
    terminate(running);

    let names = names_in(&dir);
    for name in [
        "runner.yaml",
        "lock",
        "runner.sqlite",
        "blobs",
        "repos",
        "snapshots",
        "worktrees",
        "tmp",
    ] {
        assert!(names.iter().any(|n| n == name), "{name} in {names:?}");
    }
    assert!(dir.join("blobs/sha256").is_dir());
    assert_eq!(names_in(&home), Vec::<String>::new());
}

#[test]
fn join_sets_mode_0700() {
    let (_tmp, dir) = runner_dir_with_mode(0o755);
    claim(&dir).unwrap();
    assert_eq!(mode_of(&dir), 0o700);
}

#[test]
fn join_writes_a_minimal_runner_yaml() {
    let (_tmp, dir) = runner_dir_with_mode(0o700);
    claim(&dir).unwrap();
    let text = std::fs::read_to_string(dir.join(runner_dir::RUNNER_YAML)).unwrap();
    load_runner_config(&text).unwrap();
    let content: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .collect();
    assert_eq!(content, ["{}"], "{text}");
}

#[test]
fn join_refuses_a_joined_directory() {
    let (_tmp, dir) = runner_dir_with_mode(0o700);
    claim(&dir).unwrap();
    let before = snapshot(&dir);
    let err = claim(&dir).unwrap_err();
    assert!(matches!(err, RunnerDirError::AlreadyJoined), "{err:?}");
    assert_eq!(snapshot(&dir), before);
}

#[test]
fn join_refuses_foreign_files() {
    let (_tmp, dir) = runner_dir_with_mode(0o755);
    std::fs::write(dir.join("notes.txt"), b"mine").unwrap();
    std::fs::create_dir(dir.join("src")).unwrap();
    let before = snapshot(&dir);
    let err = claim(&dir).unwrap_err();
    match &err {
        RunnerDirError::ForeignFiles(names) => {
            assert_eq!(names, &[PathBuf::from("notes.txt"), PathBuf::from("src")]);
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(snapshot(&dir), before);
    assert_eq!(mode_of(&dir), 0o755);
}

#[test]
fn join_accepts_a_directory_of_store_files() {
    let (_tmp, dir) = runner_dir_with_mode(0o700);
    drop(Store::open(&dir, SystemClock, OutboxMode::Suppressed).unwrap());
    for name in ["runner.sqlite-wal", "runner.sqlite-shm"] {
        let path = dir.join(name);
        if !path.exists() {
            std::fs::write(&path, b"").unwrap();
        }
    }
    std::fs::write(dir.join("mintswarm-runner-0123456789ab.service"), b"").unwrap();
    std::fs::write(dir.join("mintswarm-runner-0123456789ab.plist"), b"").unwrap();
    claim(&dir).unwrap();
    assert!(dir.join(runner_dir::RUNNER_YAML).is_file());
}

#[test]
fn join_refuses_a_service_file_name_outside_the_pattern() {
    for name in [
        "mintswarm-runner-0123456789a.service",
        "mintswarm-runner-0123456789AB.plist",
        "mintswarm-runner-0123456789ab.txt",
    ] {
        let (_tmp, dir) = runner_dir_with_mode(0o700);
        std::fs::write(dir.join(name), b"").unwrap();
        let err = claim(&dir).unwrap_err();
        assert!(
            matches!(err, RunnerDirError::ForeignFiles(_)),
            "{name}: {err:?}"
        );
    }
}

#[test]
fn run_refuses_an_unjoined_directory() {
    let (_tmp, dir) = runner_dir_with_mode(0o700);
    let err = runner_dir::open(&dir).unwrap_err();
    assert!(matches!(err, RunnerDirError::NotJoined), "{err:?}");
    assert_eq!(names_in(&dir), Vec::<String>::new());
}

#[test]
fn run_refuses_a_directory_with_another_mode() {
    let (_tmp, dir) = runner_dir_with_mode(0o700);
    claim(&dir).unwrap();
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
    let err = runner_dir::open(&dir).unwrap_err();
    assert!(
        matches!(
            err,
            RunnerDirError::Store(StoreError::Mode { mode: 0o755, .. })
        ),
        "{err:?}"
    );
}

#[test]
fn run_holds_the_lock_until_sigterm() {
    let (tmp, dir) = runner_dir_with_mode(0o700);
    let home = tmp.path().canonicalize().unwrap().join("home");
    std::fs::create_dir(&home).unwrap();
    claim(&dir).unwrap();

    let (first, lines) = spawn_run(&dir, &home);
    wait_ready(&lines, &dir);

    let second = Command::new(BIN)
        .arg("run")
        .current_dir(&dir)
        .output()
        .unwrap();
    assert_eq!(second.status.code(), Some(2), "{second:?}");
    let stderr = String::from_utf8_lossy(&second.stderr);
    assert!(stderr.contains("is locked by another runner"), "{stderr}");

    terminate(first);
    let (third, lines) = spawn_run(&dir, &home);
    wait_ready(&lines, &dir);
    terminate(third);
}

/// The value of the unit line `key=`, the first one.
fn unit_value<'t>(unit: &'t str, key: &str) -> &'t str {
    let prefix = format!("{key}=");
    unit.lines()
        .find_map(|line| line.strip_prefix(&prefix))
        .unwrap_or_else(|| panic!("no {key}= in {unit}"))
}

/// `WorkingDirectory=` read back: `%%` is `%`.
fn unit_working_directory(unit: &str) -> String {
    unit_value(unit, "WorkingDirectory").replace("%%", "%")
}

/// `ExecStart=` read back as its words: a `"`-quoted word with `\` escapes, `%%` and `$$`, then
/// plain words.
fn unit_exec_start(unit: &str) -> Vec<String> {
    let value = unit_value(unit, "ExecStart");
    let rest = value.strip_prefix('"').expect("a quoted program");
    let mut program = String::new();
    let mut chars = rest.chars();
    loop {
        match chars.next().expect("an unterminated quote") {
            '\\' => program.push(chars.next().expect("an escape")),
            '"' => break,
            '%' => {
                assert_eq!(chars.next(), Some('%'), "a lone % in {value}");
                program.push('%');
            }
            '$' => {
                assert_eq!(chars.next(), Some('$'), "a lone $ in {value}");
                program.push('$');
            }
            c => program.push(c),
        }
    }
    let mut words = vec![program];
    words.extend(chars.as_str().split_whitespace().map(str::to_owned));
    words
}

/// XML text with its entities read back.
fn xml_unescape(text: &str) -> String {
    text.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

/// The `<string>` values right after `<key>key</key>` in `plist`, read back: one for a string,
/// every one for an array.
fn plist_strings(plist: &str, key: &str) -> Vec<String> {
    let marker = format!("<key>{key}</key>");
    let start = plist.find(&marker).unwrap_or_else(|| panic!("no {key}")) + marker.len();
    let rest = plist[start..].trim_start();
    let (body, single) = match rest.strip_prefix("<array>") {
        Some(array) => (&array[..array.find("</array>").unwrap()], false),
        None => (rest, true),
    };
    let mut values = Vec::new();
    let mut body = body;
    while let Some(open) = body.find("<string>") {
        let after = &body[open + "<string>".len()..];
        let close = after.find("</string>").unwrap();
        values.push(xml_unescape(&after[..close]));
        body = &after[close..];
        if single {
            break;
        }
    }
    values
}

#[test]
fn unit_has_working_directory_and_exec_start() {
    let dir = Path::new("/srv/runners/one");
    let binary = Path::new("/usr/local/bin/mintswarm-runner");
    let unit = render_unit(dir, binary).unwrap();
    assert_eq!(unit_value(&unit, "WorkingDirectory"), "/srv/runners/one");
    assert_eq!(
        unit_value(&unit, "ExecStart"),
        "\"/usr/local/bin/mintswarm-runner\" run"
    );
    assert_eq!(
        unit_exec_start(&unit),
        ["/usr/local/bin/mintswarm-runner", "run"]
    );
}

#[test]
fn plist_has_working_directory_and_program_arguments() {
    let dir = Path::new("/srv/runners/one");
    let binary = Path::new("/usr/local/bin/mintswarm-runner");
    let plist = render_plist(dir, binary).unwrap();
    assert_eq!(
        plist_strings(&plist, "WorkingDirectory"),
        ["/srv/runners/one"]
    );
    assert_eq!(
        plist_strings(&plist, "ProgramArguments"),
        ["/usr/local/bin/mintswarm-runner", "run"]
    );
}

/// A runner directory and a binary with a space, `%`, `&` and `<`, and more that needs escaping.
const ODD_DIR: &str = "/srv/my runners/100% & <one>";
const ODD_BINARY: &str = "/opt/a b/50%&<$\"\\/mintswarm-runner";

#[test]
fn unit_escapes_special_characters() {
    let unit = render_unit(Path::new(ODD_DIR), Path::new(ODD_BINARY)).unwrap();
    let label = service_name(Path::new(ODD_DIR));
    let expected = format!(
        "# A systemd user unit for one mintswarm runner directory, rendered by `mintswarm-runner add-service`.\n\
         [Unit]\n\
         Description=mintswarm runner {label}\n\
         \n\
         [Service]\n\
         Type=simple\n\
         WorkingDirectory=/srv/my runners/100%% & <one>\n\
         ExecStart=\"/opt/a b/50%%&<$$\\\"\\\\/mintswarm-runner\" run\n\
         Restart=on-failure\n\
         \n\
         [Install]\n\
         WantedBy=default.target\n"
    );
    assert_eq!(unit, expected);
    assert_eq!(unit_working_directory(&unit), ODD_DIR);
    assert_eq!(unit_exec_start(&unit), [ODD_BINARY, "run"]);
}

#[test]
fn plist_escapes_special_characters() {
    let plist = render_plist(Path::new(ODD_DIR), Path::new(ODD_BINARY)).unwrap();
    let label = service_name(Path::new(ODD_DIR));
    let expected = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
         <!-- A launchd agent for one mintswarm runner directory, rendered by `mintswarm-runner add-service`. -->\n\
         <plist version=\"1.0\">\n\
         <dict>\n\
         \t<key>Label</key>\n\
         \t<string>{label}</string>\n\
         \t<key>ProgramArguments</key>\n\
         \t<array>\n\
         \t\t<string>/opt/a b/50%&amp;&lt;$&quot;\\/mintswarm-runner</string>\n\
         \t\t<string>run</string>\n\
         \t</array>\n\
         \t<key>WorkingDirectory</key>\n\
         \t<string>/srv/my runners/100% &amp; &lt;one&gt;</string>\n\
         \t<key>RunAtLoad</key>\n\
         \t<true/>\n\
         \t<key>KeepAlive</key>\n\
         \t<true/>\n\
         </dict>\n\
         </plist>\n"
    );
    assert_eq!(plist, expected);
    assert_eq!(plist_strings(&plist, "WorkingDirectory"), [ODD_DIR]);
    assert_eq!(
        plist_strings(&plist, "ProgramArguments"),
        [ODD_BINARY, "run"]
    );
}

#[test]
fn placeholder_text_in_a_path_is_not_substituted() {
    let dir = Path::new("/srv/{{label}}/{{program}}");
    let binary = Path::new("/opt/{{working_directory}}/mintswarm-runner");
    let unit = render_unit(dir, binary).unwrap();
    assert_eq!(unit_working_directory(&unit), dir.to_str().unwrap());
    assert_eq!(unit_exec_start(&unit)[0], binary.to_str().unwrap());
    let plist = render_plist(dir, binary).unwrap();
    assert_eq!(
        plist_strings(&plist, "WorkingDirectory"),
        [dir.to_str().unwrap()]
    );
    assert_eq!(
        plist_strings(&plist, "ProgramArguments")[0],
        binary.to_str().unwrap()
    );
}

#[test]
fn unit_refuses_unsafe_working_directory_edges() {
    let binary = Path::new("/usr/local/bin/mintswarm-runner");
    for dir in [
        " /srv/one",
        "/srv/one ",
        "/srv/one\t",
        "/srv/one\\",
        "/srv/o\nne",
    ] {
        let err = render_unit(Path::new(dir), binary).unwrap_err();
        assert!(
            matches!(err, RunnerDirError::UnrenderablePath(_)),
            "{dir:?}: {err:?}"
        );
    }
    let err = render_unit(Path::new("/srv/one"), Path::new("/bin/a\u{7f}b")).unwrap_err();
    assert!(
        matches!(err, RunnerDirError::UnrenderablePath(_)),
        "{err:?}"
    );
    let err = render_plist(Path::new("/srv/o\u{1}ne"), binary).unwrap_err();
    assert!(
        matches!(err, RunnerDirError::UnrenderablePath(_)),
        "{err:?}"
    );
}

#[test]
fn names_follow_the_hash_pattern_and_differ() {
    let binary = Path::new("/usr/local/bin/mintswarm-runner");
    let mut labels = Vec::new();
    for dir in ["/srv/runners/one", "/srv/runners/two"] {
        let name = service_name(Path::new(dir));
        let hash = name.strip_prefix("mintswarm-runner-").expect(&name);
        assert_eq!(hash.len(), 12, "{name}");
        assert!(
            hash.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')),
            "{name}"
        );
        let plist = render_plist(Path::new(dir), binary).unwrap();
        assert_eq!(plist_strings(&plist, "Label"), std::slice::from_ref(&name));
        let unit = render_unit(Path::new(dir), binary).unwrap();
        assert_eq!(
            unit_value(&unit, "Description"),
            format!("mintswarm runner {name}")
        );
        labels.push(name);
    }
    assert_ne!(labels[0], labels[1]);
}

#[test]
fn add_service_from_a_relative_path_writes_an_absolute_directory() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let dir = root.join("rel");
    std::fs::create_dir(&dir).unwrap();
    claim(&dir).unwrap();
    // A relative link to the binary, run by a relative path from a relative directory.
    let binary = Path::new(BIN).canonicalize().unwrap();
    std::os::unix::fs::symlink(&binary, root.join("link")).unwrap();
    let output = Command::new("sh")
        .args(["-c", "cd rel && exec ../link add-service"])
        .current_dir(&root)
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");

    let name = service_name(&dir);
    let (working_directory, program) = match ServiceKind::host() {
        ServiceKind::Systemd => {
            let unit = std::fs::read_to_string(dir.join(format!("{name}.service"))).unwrap();
            (
                unit_working_directory(&unit),
                unit_exec_start(&unit).remove(0),
            )
        }
        ServiceKind::Launchd => {
            let plist = std::fs::read_to_string(dir.join(format!("{name}.plist"))).unwrap();
            (
                plist_strings(&plist, "WorkingDirectory").remove(0),
                plist_strings(&plist, "ProgramArguments").remove(0),
            )
        }
    };
    assert_eq!(Path::new(&working_directory), dir);
    assert!(Path::new(&program).is_absolute(), "{program}");
    assert_eq!(Path::new(&program).canonicalize().unwrap(), binary);
}

#[test]
fn add_service_refuses_an_unjoined_directory() {
    let (_tmp, dir) = runner_dir_with_mode(0o700);
    for kind in [ServiceKind::Systemd, ServiceKind::Launchd] {
        let err = add_service(&dir, Path::new(BIN), kind).unwrap_err();
        assert!(matches!(err, RunnerDirError::NotJoined), "{err:?}");
        assert_eq!(names_in(&dir), Vec::<String>::new());
    }
}

#[test]
fn add_service_refuses_to_overwrite() {
    let (_tmp, dir) = runner_dir_with_mode(0o700);
    claim(&dir).unwrap();
    for kind in [ServiceKind::Systemd, ServiceKind::Launchd] {
        let (path, _) = add_service(&dir, Path::new(BIN), kind).unwrap();
        assert_eq!(path.parent(), Some(dir.as_path()));
        std::fs::write(&path, b"edited by hand").unwrap();
        let err = add_service(&dir, Path::new(BIN), kind).unwrap_err();
        assert!(
            matches!(&err, RunnerDirError::ServiceExists(existing) if *existing == path),
            "{err:?}"
        );
        assert_eq!(std::fs::read(&path).unwrap(), b"edited by hand");
    }
}

#[test]
fn instructions_copy_before_bootstrap_and_mention_lingering() {
    let name = "mintswarm-runner-0123456789ab";
    let agent = format!("~/Library/LaunchAgents/{name}.plist");
    let text = instructions(
        ServiceKind::Launchd,
        Path::new("/srv/it's here/mintswarm-runner-0123456789ab.plist"),
        name,
    );
    let copy = format!("cp '/srv/it'\\''s here/mintswarm-runner-0123456789ab.plist' {agent}\n");
    let bootstrap = format!("launchctl bootstrap gui/$(id -u) {agent}\n");
    let copy_at = text.find(&copy).unwrap_or_else(|| panic!("{text}"));
    let bootstrap_at = text.find(&bootstrap).unwrap_or_else(|| panic!("{text}"));
    assert!(copy_at < bootstrap_at, "{text}");
    assert!(
        text.contains("repeat the cp after `mintswarm-runner add-service` is run again"),
        "{text}"
    );

    let text = instructions(
        ServiceKind::Systemd,
        Path::new("/srv/one/mintswarm-runner-0123456789ab.service"),
        name,
    );
    assert!(
        text.contains("systemctl --user link '/srv/one/mintswarm-runner-0123456789ab.service'\n"),
        "{text}"
    );
    assert!(
        text.contains(&format!("systemctl --user enable --now {name}\n")),
        "{text}"
    );
    assert!(text.contains("lingering"), "{text}");
    assert!(text.contains("loginctl enable-linger"), "{text}");
}
