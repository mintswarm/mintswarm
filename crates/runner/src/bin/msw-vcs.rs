//! `msw-vcs`, the read tool on a stage's `PATH`: a thin client of the per-attempt server.
//!
//! It finds the server's socket next to its own executable, sends its arguments as given, and
//! writes the reply's stdout and stderr and exits with its code. It reads no environment
//! variable. A failure to reach the server is `msw-vcs: vcs failure`, exit code 5.

use std::io::{self, Write};
use std::os::unix::ffi::OsStringExt;
use std::process::ExitCode;

use mintswarm_runner::exec::tools::{SOCKET, wire};

fn main() -> ExitCode {
    let argv: Vec<Vec<u8>> = std::env::args_os()
        .skip(1)
        .map(OsStringExt::into_vec)
        .collect();
    let reply = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join(SOCKET)))
        .and_then(|socket| wire::call(&socket, &argv).ok())
        .unwrap_or_else(|| wire::Reply {
            exit: 5,
            stdout: Vec::new(),
            stderr: b"msw-vcs: vcs failure\n".to_vec(),
        });
    let written = io::stdout()
        .lock()
        .write_all(&reply.stdout)
        .and_then(|()| io::stdout().lock().flush())
        .and_then(|()| io::stderr().lock().write_all(&reply.stderr));
    match written {
        Ok(()) => ExitCode::from(reply.exit),
        Err(_) => ExitCode::from(1),
    }
}
