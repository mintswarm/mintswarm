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
