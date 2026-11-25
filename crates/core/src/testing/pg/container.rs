//! The container backend: `docker` or `podman` runs an image it already holds, never pulling,
//! with the data on a tmpfs and the port on loopback assigned by the runtime.

use std::path::Path;
use std::process::{Command, Output, Stdio};

use super::watchdog::{Cleanup, Watchdog};
use super::{PgHarnessError, Started};

/// The label every harness container carries, so that leaked ones can be found.
const LABEL: &str = "mintswarm.pg-tests=1";

/// Starts a container of `image` with `runtime`.
pub(crate) fn start(runtime: &Path, image: &str) -> Result<Started, PgHarnessError> {
    let inspect = Command::new(runtime)
        .args(["image", "inspect", image])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|source| spawn_error(runtime, source))?;
    if !inspect.success() {
        return Err(PgHarnessError::ImageMissing {
            image: image.to_owned(),
        });
    }

    // Started first, so that a container is never left without one.
    let mut watchdog = Watchdog::spawn(Cleanup::Container {
        runtime: runtime.to_owned(),
    })?;
    let run = output(runtime, &run_args(image))?;
    let id = String::from_utf8_lossy(&run.stdout).trim().to_owned();
    if id.is_empty() || id.contains(char::is_whitespace) {
        return Err(failed(runtime, run));
    }
    watchdog.arm(&id)?;

    let port = output(runtime, &["port", &id, "5432/tcp"])?;
    let port =
        parse_port(&String::from_utf8_lossy(&port.stdout)).ok_or_else(|| failed(runtime, port))?;

    let mut config = tokio_postgres::Config::new();
    config
        .host("127.0.0.1")
        .port(port)
        .user("postgres")
        .dbname("postgres");
    Ok(Started {
        config,
        dir: None,
        server: None,
        container_id: Some(id),
        watchdog,
    })
}

/// The arguments of `<runtime> run`: detached, removed on stop, never pulled, data on a tmpfs,
/// trust authentication, loopback only, and durability off.
fn run_args(image: &str) -> Vec<String> {
    [
        "run",
        "-d",
        "--rm",
        "--pull=never",
        "--label",
        LABEL,
        "--tmpfs",
        "/var/lib/postgresql/data",
        "-e",
        "POSTGRES_HOST_AUTH_METHOD=trust",
        "-p",
        "127.0.0.1::5432",
        image,
        "-c",
        "fsync=off",
        "-c",
        "synchronous_commit=off",
        "-c",
        "full_page_writes=off",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect()
}

/// Reads the host port from `<runtime> port` output such as `127.0.0.1:49153`.
fn parse_port(text: &str) -> Option<u16> {
    text.lines()
        .find_map(|line| line.trim().rsplit_once(':')?.1.parse().ok())
}

/// Runs the runtime and returns its output when it succeeds.
fn output<S: AsRef<std::ffi::OsStr>>(runtime: &Path, args: &[S]) -> Result<Output, PgHarnessError> {
    let output = Command::new(runtime)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(|source| spawn_error(runtime, source))?;
    if output.status.success() {
        Ok(output)
    } else {
        Err(failed(runtime, output))
    }
}

fn failed(runtime: &Path, output: Output) -> PgHarnessError {
    let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    PgHarnessError::CommandFailed {
        program: runtime.display().to_string(),
        status: output.status,
        output: text,
    }
}

fn spawn_error(runtime: &Path, source: std::io::Error) -> PgHarnessError {
    PgHarnessError::Spawn {
        program: runtime.display().to_string(),
        source,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_never_pulls() {
        let args = run_args("postgres:17");
        assert!(args.iter().any(|arg| arg == "--pull=never"));
        assert!(!args.iter().any(|arg| arg == "pull"));
        assert_eq!(args[0], "run");
    }

    #[test]
    fn run_turns_durability_off() {
        let args = run_args("postgres:17").join(" ");
        assert!(args.contains("-c fsync=off"));
        assert!(args.contains("-c synchronous_commit=off"));
        assert!(args.contains("-c full_page_writes=off"));
    }

    #[test]
    fn port_is_read_from_the_first_mapping() {
        assert_eq!(parse_port("127.0.0.1:49153\n"), Some(49153));
        assert_eq!(parse_port("[::1]:5000\n127.0.0.1:5001\n"), Some(5000));
        assert_eq!(parse_port(""), None);
    }
}
