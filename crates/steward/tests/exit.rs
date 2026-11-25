//! Runs the built `mintswarm-steward` binary.

use std::process::Command;

#[test]
fn steward_exits_zero() {
    let status = Command::new(env!("CARGO_BIN_EXE_mintswarm-steward"))
        .status()
        .expect("mintswarm-steward runs");
    assert!(status.success(), "mintswarm-steward exits 0");
}
