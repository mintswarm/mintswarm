//! Runs the built `mintswarm-server` binary.

use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_mintswarm-server");

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
    for option in ["--listen", "--log", "--secret-env", "--key-store"] {
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
    assert!(
        !status.success(),
        "webhooks without a secret exits non-zero"
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
        .status()
        .expect("mintswarm-server runs");
    assert!(!status.success(), "--key-store alone is refused");
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
    assert!(
        !status.success(),
        "--secret-env with --key-store is refused"
    );
    assert!(!log.exists(), "the log is not created");
}
