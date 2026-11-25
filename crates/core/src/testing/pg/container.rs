//! The container backend: `docker` or `podman` runs an image it already holds, never pulling,
//! with the data on a tmpfs, a random password, and the port on loopback assigned by the
//! runtime.

use std::fmt::Write as _;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use super::watchdog::{Cleanup, Watchdog};
use super::{PgHarnessError, Started};

/// The label every harness container carries, so that leaked ones can be found.
const LABEL: &str = "mintswarm.pg-tests=1";
/// The image's variable for the superuser's password, passed through the environment of the
/// runtime's client so that it never appears in a command line.
const PASSWORD_VAR: &str = "POSTGRES_PASSWORD";

/// A started container, for checking that it is still running.
#[derive(Debug, Clone)]
pub(crate) struct Container {
    /// The `docker` or `podman` executable.
    pub(crate) runtime: PathBuf,
    /// The name the harness gave it.
    pub(crate) name: String,
    /// The id the runtime printed for it.
    pub(crate) id: String,
}

impl Container {
    /// Returns [`PgHarnessError::ContainerExited`] with the container's logs once it is no
    /// longer running. Only whether `ps` prints anything is looked at.
    pub(crate) fn ensure_running(&self) -> Result<(), PgHarnessError> {
        let filter = format!("id={}", self.id);
        let ps = checked(
            &self.runtime,
            command(&self.runtime).args(["ps", "-q", "--filter", &filter]),
        )?;
        if !ps.stdout.iter().all(u8::is_ascii_whitespace) {
            return Ok(());
        }
        let logs = command(&self.runtime)
            .args(["logs", &self.name])
            .output()
            .map(|logs| combined(&logs))
            .unwrap_or_default();
        Err(PgHarnessError::ContainerExited { logs })
    }
}

/// Starts a container of `image` with `runtime`.
pub(crate) fn start(runtime: &Path, image: &str) -> Result<Started, PgHarnessError> {
    let inspect = command(runtime)
        .args(["image", "inspect", image])
        .output()
        .map_err(|source| spawn_error(runtime, source))?;
    if !inspect.status.success() {
        return Err(PgHarnessError::ImageMissing {
            image: image.to_owned(),
            output: combined(&inspect),
        });
    }

    let name = container_name();
    let password = password()?;
    // Started and armed with the name first, so that a container is never left without one.
    let mut watchdog = Watchdog::spawn(Cleanup::Container {
        runtime: runtime.to_owned(),
    })?;
    watchdog.arm(&name)?;
    let run = checked(
        runtime,
        command(runtime)
            .args(run_args(image, &name))
            .env(PASSWORD_VAR, &password),
    )?;
    let id = String::from_utf8_lossy(&run.stdout).trim().to_owned();
    if id.is_empty() || id.contains(char::is_whitespace) {
        return Err(failed(runtime, &run));
    }

    let port = checked(runtime, command(runtime).args(["port", &name, "5432/tcp"]))?;
    let port =
        parse_port(&String::from_utf8_lossy(&port.stdout)).ok_or_else(|| failed(runtime, &port))?;

    let mut config = tokio_postgres::Config::new();
    config
        .host("127.0.0.1")
        .port(port)
        .user("postgres")
        .password(password)
        .dbname("postgres");
    Ok(Started {
        config,
        dir: None,
        server: None,
        container: Some(Container {
            runtime: runtime.to_owned(),
            name,
            id,
        }),
        watchdog,
    })
}

/// The arguments of `<runtime> run`: detached, named, never pulled, data on a tmpfs, the
/// password from the client's environment, loopback only, and durability off. The container
/// is not run with `--rm`, so that its logs outlive it; the watchdog removes it.
fn run_args(image: &str, name: &str) -> Vec<String> {
    [
        "run",
        "-d",
        "--name",
        name,
        "--pull=never",
        "--label",
        LABEL,
        "--tmpfs",
        "/var/lib/postgresql/data",
        "-e",
        PASSWORD_VAR,
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

/// A container name unique to this process and moment.
fn container_name() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("msw-pg-{}-{nanos}", std::process::id())
}

/// A random password for one instance, from `/dev/urandom`, hex-encoded.
fn password() -> Result<String, PgHarnessError> {
    let mut bytes = [0_u8; 16];
    File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        // Writing to a `String` cannot fail.
        let _ = write!(text, "{byte:02x}");
    }
    Ok(text)
}

/// Reads the host port from the `127.0.0.1:<port>` line of `<runtime> port` output.
fn parse_port(text: &str) -> Option<u16> {
    text.lines()
        .find_map(|line| line.trim().strip_prefix("127.0.0.1:")?.parse().ok())
}

/// The runtime with its standard input closed.
fn command(runtime: &Path) -> Command {
    let mut command = Command::new(runtime);
    command.stdin(Stdio::null());
    command
}

/// Runs `command` and returns its output when it succeeds.
fn checked(runtime: &Path, command: &mut Command) -> Result<Output, PgHarnessError> {
    let output = command
        .output()
        .map_err(|source| spawn_error(runtime, source))?;
    if output.status.success() {
        Ok(output)
    } else {
        Err(failed(runtime, &output))
    }
}

/// Standard output followed by standard error, as text.
fn combined(output: &Output) -> String {
    let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    text
}

fn failed(runtime: &Path, output: &Output) -> PgHarnessError {
    PgHarnessError::CommandFailed {
        program: runtime.display().to_string(),
        status: output.status,
        output: combined(output),
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
        let args = run_args("postgres:17", "msw-pg-1-2");
        assert!(args.iter().any(|arg| arg == "--pull=never"));
        assert!(!args.iter().any(|arg| arg == "pull"));
        assert_eq!(args[0], "run");
    }

    #[test]
    fn run_turns_durability_off() {
        let args = run_args("postgres:17", "msw-pg-1-2").join(" ");
        assert!(args.contains("-c fsync=off"));
        assert!(args.contains("-c synchronous_commit=off"));
        assert!(args.contains("-c full_page_writes=off"));
    }

    #[test]
    fn run_is_named_and_keeps_the_password_off_the_command_line() {
        let args = run_args("postgres:17", "msw-pg-1-2");
        assert!(args.windows(2).any(|pair| pair == ["--name", "msw-pg-1-2"]));
        assert!(args.windows(2).any(|pair| pair == ["-e", PASSWORD_VAR]));
        assert!(!args.iter().any(|arg| arg.contains("trust")));
        assert!(
            !args
                .iter()
                .any(|arg| arg.starts_with(&format!("{PASSWORD_VAR}=")))
        );
    }

    #[test]
    fn passwords_differ() {
        let first = password().unwrap();
        let second = password().unwrap();
        assert_eq!(first.len(), 32);
        assert_ne!(first, second);
    }

    #[test]
    fn port_is_read_from_the_loopback_mapping() {
        assert_eq!(parse_port("127.0.0.1:49153\n"), Some(49153));
        assert_eq!(parse_port("[::1]:5000\n127.0.0.1:5001\n"), Some(5001));
        assert_eq!(parse_port("[::1]:5000\n"), None);
        assert_eq!(parse_port(""), None);
    }
}
