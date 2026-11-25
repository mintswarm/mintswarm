//! Runs the built `mintswarm-server` binary.

use std::io::{BufRead, BufReader};
use std::process::{Command, ExitStatus, Stdio};

const BIN: &str = env!("CARGO_BIN_EXE_mintswarm-server");

/// clap's exit code for a usage error, which it gives before `webhooks` runs.
const USAGE: i32 = 2;

/// The exit code of a `webhooks` run that fails, `ExitCode::FAILURE`.
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

#[test]
fn each_subcommand_exits_zero() {
    for subcommand in ["serve", "migrate"] {
        let status = Command::new(BIN)
            .arg(subcommand)
            .status()
            .expect("mintswarm-server runs");
        assert!(status.success(), "{subcommand} exits 0");
    }
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
