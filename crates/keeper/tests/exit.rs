//! Runs the built `mintswarm-keeper` binary.

use std::process::Command;

#[test]
fn keeper_exits_zero() {
    let status = Command::new(env!("CARGO_BIN_EXE_mintswarm-keeper"))
        .status()
        .expect("mintswarm-keeper runs");
    assert!(status.success(), "mintswarm-keeper exits 0");
}
