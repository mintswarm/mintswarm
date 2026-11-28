//! Runs the built `mintswarm-server` binary.

use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

const BIN: &str = env!("CARGO_BIN_EXE_mintswarm-server");

/// clap's exit code for a usage error, which it gives before a subcommand runs.
const USAGE: i32 = 2;

/// The exit code of a `serve`, `migrate`, `webhooks` or `migrate new` run that fails,
/// `ExitCode::FAILURE`.
const FAILURE: i32 = 1;

fn code(status: ExitStatus) -> Option<i32> {
    status.code()
}

#[test]
fn help_lists_serve_and_migrate() {
    let output = Command::new(BIN)
        .arg("--help")
        .output()
        .expect("mintswarm-server --help runs");
    assert!(output.status.success(), "--help exits 0");
    let stdout = String::from_utf8(output.stdout).expect("help text is UTF-8");
    assert!(stdout.contains("serve"), "help lists serve: {stdout}");
    assert!(stdout.contains("migrate"), "help lists migrate: {stdout}");
}

/// Every variable that overrides a key of `mintswarm.yaml`, in both forms.
const OVERRIDES: [&str; 6] = [
    "MINTSWARM_LISTEN",
    "MINTSWARM_LISTEN_FILE",
    "MINTSWARM_DATABASE_URL",
    "MINTSWARM_DATABASE_URL_FILE",
    "MINTSWARM_DATABASE_PASSWORD",
    "MINTSWARM_DATABASE_PASSWORD_FILE",
];

/// A command for the binary run in `dir`, with none of the overrides set.
fn in_dir(dir: &std::path::Path) -> Command {
    let mut command = Command::new(BIN);
    command.current_dir(dir);
    for name in OVERRIDES {
        command.env_remove(name);
    }
    command
}

/// A loopback port nothing listens on.
fn closed_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("a free port");
    listener.local_addr().expect("bound").port()
}

/// Writes `mintswarm.yaml` in `dir` listening on `listen`, with a database nothing serves.
fn write_config(dir: &std::path::Path, listen: &str) -> std::path::PathBuf {
    let path = dir.join("mintswarm.yaml");
    let text = format!(
        "listen: {listen}\ndatabase:\n  url: postgres://mintswarm@127.0.0.1:{}/mintswarm\n",
        closed_port()
    );
    std::fs::write(&path, text).expect("the settings are written");
    path
}

#[test]
fn serve_and_migrate_without_a_config_file_fail_as_a_run() {
    for subcommand in ["serve", "migrate"] {
        let dir = tempfile::TempDir::new().expect("a temporary directory");
        let output = in_dir(dir.path())
            .arg(subcommand)
            .output()
            .expect("mintswarm-server runs");
        assert_eq!(
            code(output.status),
            Some(FAILURE),
            "{subcommand} without mintswarm.yaml fails as a run, not as usage"
        );
    }
}

#[test]
fn serve_refuses_a_non_loopback_address() {
    let dir = tempfile::TempDir::new().expect("a temporary directory");
    let config = write_config(dir.path(), "0.0.0.0:0");
    let mut child = in_dir(dir.path())
        .arg("serve")
        .arg("--config")
        .arg(&config)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("mintswarm-server starts");
    let mut stderr = child.stderr.take().expect("stderr is piped");
    let status = wait_bounded(&mut child, Duration::from_secs(5)).expect("serve exits at once");
    assert_eq!(code(status), Some(FAILURE));
    let mut text = String::new();
    std::io::Read::read_to_string(&mut stderr, &mut text).expect("stderr reads");
    assert!(text.contains("no auth provider is configured"), "{text}");
}

#[test]
fn serve_waiting_for_the_database_stops_on_sigterm() {
    let dir = tempfile::TempDir::new().expect("a temporary directory");
    write_config(dir.path(), "127.0.0.1:0");
    let mut child = in_dir(dir.path())
        .arg("serve")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("mintswarm-server starts");
    let mut stderr = BufReader::new(child.stderr.take().expect("stderr is piped"));
    let mut first = String::new();
    stderr
        .read_line(&mut first)
        .expect("the start line is read");
    assert!(first.contains("listening on"), "{first}");
    send_signal(&child, "-TERM");
    let status =
        wait_bounded(&mut child, Duration::from_secs(5)).expect("serve stops within 5 seconds");
    assert_eq!(code(status), Some(0), "a stop exits 0");
}

fn stdout_of(args: &[&str]) -> String {
    let output = Command::new(BIN)
        .args(args)
        .output()
        .expect("mintswarm-server runs");
    assert!(output.status.success(), "{args:?} exits 0");
    String::from_utf8(output.stdout).expect("help text is UTF-8")
}

#[test]
fn help_lists_webhooks() {
    let stdout = stdout_of(&["--help"]);
    assert!(stdout.contains("webhooks"), "help lists webhooks: {stdout}");
}

#[test]
fn webhooks_help_lists_listen_and_log() {
    let stdout = stdout_of(&["webhooks", "--help"]);
    for option in [
        "--listen",
        "--log",
        "--secret-env",
        "--key-store",
        "--max-in-flight",
        "--max-queued",
        "--read-timeout",
    ] {
        assert!(
            stdout.contains(option),
            "webhooks help lists {option}: {stdout}"
        );
    }
}

#[test]
fn webhooks_without_a_secret_fails_before_creating_the_log() {
    let dir = tempfile::TempDir::new().expect("a temporary directory");
    let log = dir.path().join("deliveries.jsonl");
    let status = Command::new(BIN)
        .args(["webhooks", "--listen", "127.0.0.1:0", "--log"])
        .arg(&log)
        .env_remove("MSW_WEBHOOK_SECRET")
        .status()
        .expect("mintswarm-server runs");
    assert_eq!(
        code(status),
        Some(FAILURE),
        "webhooks without a secret fails as a run, not as usage"
    );
    assert!(!log.exists(), "the log is not created");
}

#[test]
fn webhooks_key_store_needs_service_and_account() {
    let dir = tempfile::TempDir::new().expect("a temporary directory");
    let log = dir.path().join("deliveries.jsonl");
    let status = Command::new(BIN)
        .args([
            "webhooks",
            "--listen",
            "127.0.0.1:0",
            "--key-store",
            "file",
            "--log",
        ])
        .arg(&log)
        .env_remove("MSW_WEBHOOK_SECRET")
        .status()
        .expect("mintswarm-server runs");
    assert_eq!(code(status), Some(USAGE), "--key-store alone is refused");
    assert!(!log.exists(), "the log is not created");
}

#[test]
fn webhooks_secret_env_conflicts_with_key_store() {
    let dir = tempfile::TempDir::new().expect("a temporary directory");
    let log = dir.path().join("deliveries.jsonl");
    let status = Command::new(BIN)
        .args([
            "webhooks",
            "--listen",
            "127.0.0.1:0",
            "--secret-env",
            "MSW_WEBHOOK_SECRET",
            "--key-store",
            "file",
            "--key-service",
            "mintswarm",
            "--key-account",
            "webhook",
            "--log",
        ])
        .arg(&log)
        .env("MSW_WEBHOOK_SECRET", "it's a secret to everybody")
        .status()
        .expect("mintswarm-server runs");
    assert_eq!(
        code(status),
        Some(USAGE),
        "--secret-env with --key-store is refused"
    );
    assert!(!log.exists(), "the log is not created");
}

#[test]
fn webhooks_limits_must_be_at_least_one() {
    for option in ["--max-in-flight", "--max-queued", "--read-timeout"] {
        let dir = tempfile::TempDir::new().expect("a temporary directory");
        let log = dir.path().join("deliveries.jsonl");
        let status = Command::new(BIN)
            .args(["webhooks", "--listen", "127.0.0.1:0", option, "0", "--log"])
            .arg(&log)
            .env("MSW_WEBHOOK_SECRET", "it's a secret to everybody")
            .status()
            .expect("mintswarm-server runs");
        assert_eq!(code(status), Some(USAGE), "{option} 0 is refused");
        assert!(!log.exists(), "the log is not created");
    }
}

#[test]
fn webhooks_exits_zero_on_sigterm_and_sigint() {
    for signal in ["-TERM", "-INT"] {
        let dir = tempfile::TempDir::new().expect("a temporary directory");
        let log = dir.path().join("deliveries.jsonl");
        let mut child = Command::new(BIN)
            .args(["webhooks", "--listen", "127.0.0.1:0", "--log"])
            .arg(&log)
            .env("MSW_WEBHOOK_SECRET", "it's a secret to everybody")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .expect("mintswarm-server starts");
        let mut stderr = BufReader::new(child.stderr.take().expect("stderr is piped"));
        let mut first = String::new();
        stderr
            .read_line(&mut first)
            .expect("the start line is read");
        assert!(log.exists(), "the log is open once it listens");
        let killed = Command::new("kill")
            .arg(signal)
            .arg(child.id().to_string())
            .status()
            .expect("kill runs");
        assert!(killed.success(), "the signal {signal} is sent");
        let status = child.wait().expect("mintswarm-server exits");
        assert_eq!(code(status), Some(0), "{signal} stops it cleanly");
    }
}

/// Sends `signal` to `child` with `kill`.
fn send_signal(child: &Child, signal: &str) {
    let killed = Command::new("kill")
        .arg(signal)
        .arg(child.id().to_string())
        .status()
        .expect("kill runs");
    assert!(killed.success(), "the signal {signal} is sent");
}

/// Waits up to `bound` for `child` to exit, and kills it if it has not.
fn wait_bounded(child: &mut Child, bound: Duration) -> Option<ExitStatus> {
    let start = Instant::now();
    while start.elapsed() < bound {
        if let Some(status) = child.try_wait().expect("the child can be polled") {
            return Some(status);
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let _ = child.kill();
    let _ = child.wait();
    None
}

#[test]
fn webhooks_a_second_signal_ends_a_stop_held_by_a_stalled_connection() {
    let dir = tempfile::TempDir::new().expect("a temporary directory");
    let log = dir.path().join("deliveries.jsonl");
    // The default read timeout gives a 20-second stop deadline, longer than the bound below.
    let mut child = Command::new(BIN)
        .args(["webhooks", "--listen", "127.0.0.1:0", "--log"])
        .arg(&log)
        .env("MSW_WEBHOOK_SECRET", "it's a secret to everybody")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("mintswarm-server starts");
    let mut stderr = BufReader::new(child.stderr.take().expect("stderr is piped"));
    let mut first = String::new();
    stderr
        .read_line(&mut first)
        .expect("the start line is read");
    // The start line is ours: "... listening on http://<addr>/github/webhook, ...".
    let addr = first
        .split("http://")
        .nth(1)
        .and_then(|rest| rest.split('/').next())
        .expect("the start line gives the address")
        .to_owned();
    let mut stalled = TcpStream::connect(&addr).expect("the receiver accepts");
    stalled
        .write_all(b"POST /github/webhook HTTP/1.1\r\n")
        .expect("the partial head is sent");
    std::thread::sleep(Duration::from_millis(200));

    send_signal(&child, "-TERM");
    let mut stopping = String::new();
    stderr
        .read_line(&mut stopping)
        .expect("the stopping line is read");
    std::thread::sleep(Duration::from_millis(200));
    assert!(
        child.try_wait().expect("the child can be polled").is_none(),
        "the stalled connection holds the stop"
    );
    send_signal(&child, "-INT");
    let status = wait_bounded(&mut child, Duration::from_secs(10))
        .expect("the second signal ends the stop before the deadline");
    assert_eq!(code(status), Some(0), "a forced stop still exits 0");
    drop(stalled);
}

/// The seconds since the Unix epoch now.
fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("the clock is after 1970")
        .as_secs()
}

/// Runs `migrate new <name> --tree <tree>`.
fn migrate_new(name: &str, tree: &std::path::Path) -> std::process::Output {
    Command::new(BIN)
        .args(["migrate", "new", name, "--tree"])
        .arg(tree)
        .output()
        .expect("mintswarm-server runs")
}

/// The path `migrate new` printed, after checking it exited 0.
fn created_path(output: &std::process::Output) -> std::path::PathBuf {
    assert!(output.status.success(), "migrate new exits 0: {output:?}");
    let stdout = String::from_utf8(output.stdout.clone()).expect("the path is UTF-8");
    std::path::PathBuf::from(stdout.trim_end())
}

#[test]
fn migrate_help_lists_new() {
    let stdout = stdout_of(&["migrate", "--help"]);
    assert!(stdout.contains("new"), "migrate help lists new: {stdout}");
}

#[test]
fn migrate_new_names_the_file_for_the_utc_second() {
    let dir = tempfile::TempDir::new().expect("a temporary directory");
    let before = mintswarm_core::migrations::version_of(unix_now());
    let created = created_path(&migrate_new("add_jobs", dir.path()));
    let after = mintswarm_core::migrations::version_of(unix_now());
    assert!(created.is_file(), "{} is created", created.display());
    assert_eq!(
        created.parent(),
        Some(dir.path().join("mintswarm/migrations").as_path())
    );
    let file_name = created
        .file_name()
        .and_then(|name| name.to_str())
        .expect("the file name is UTF-8");
    let version = file_name
        .strip_suffix("_add_jobs.up.sql")
        .expect("the file is <version>_add_jobs.up.sql");
    assert!(
        before.as_str() <= version && version <= after.as_str(),
        "{before} <= {version} <= {after}"
    );
}

#[test]
fn migrate_new_twice_gives_two_files_and_keeps_the_first() {
    let dir = tempfile::TempDir::new().expect("a temporary directory");
    let first = created_path(&migrate_new("same", dir.path()));
    std::fs::write(&first, "CREATE TABLE a ();\n").expect("the first file is written");
    let second = created_path(&migrate_new("same", dir.path()));
    assert_ne!(first, second, "the second run uses another second");
    assert!(second.is_file(), "{} is created", second.display());
    assert_eq!(
        std::fs::read(&first).expect("the first file reads"),
        b"CREATE TABLE a ();\n"
    );
}

#[test]
fn migrate_new_refuses_an_invalid_name() {
    let dir = tempfile::TempDir::new().expect("a temporary directory");
    let output = migrate_new("bad-name", dir.path());
    assert_eq!(code(output.status), Some(FAILURE), "an invalid name fails");
    assert!(!dir.path().join("mintswarm").exists(), "nothing is created");
}

#[test]
fn migrate_new_refuses_a_tree_whose_parent_is_missing() {
    let dir = tempfile::TempDir::new().expect("a temporary directory");
    let parent = dir.path().join("missing");
    let output = migrate_new("add_jobs", &parent.join("migrations"));
    assert_eq!(code(output.status), Some(FAILURE), "a missing parent fails");
    let stderr = String::from_utf8(output.stderr).expect("the error is UTF-8");
    assert!(
        stderr.contains(&parent.display().to_string()),
        "the error names the missing folder: {stderr}"
    );
    assert!(!parent.exists(), "nothing is created");
}
