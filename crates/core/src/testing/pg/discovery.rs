//! Finding a Postgres backend without downloading anything: local binaries first, then a
//! container runtime with an image it already holds.

use std::ffi::OsString;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use super::PgHarnessError;

/// The variable naming a directory that holds `initdb` and `postgres`; it wins over `PATH`.
pub(crate) const PG_BIN_VAR: &str = "MINTSWARM_PG_BIN";
/// The variable naming the container image; [`DEFAULT_IMAGE`] when unset.
pub(crate) const PG_IMAGE_VAR: &str = "MINTSWARM_PG_IMAGE";
/// The image used when [`PG_IMAGE_VAR`] is unset or empty.
pub(crate) const DEFAULT_IMAGE: &str = "postgres:17";

/// Where the test instance comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Backend {
    /// Local `initdb` and `postgres` binaries.
    Binaries {
        /// The directory that holds both.
        bindir: PathBuf,
    },
    /// A container runtime and an image it holds locally.
    Container {
        /// The `docker` or `podman` executable.
        runtime: PathBuf,
        /// The image to run.
        image: String,
    },
}

/// The parts of the process environment that discovery reads.
#[derive(Debug, Clone, Default)]
pub(crate) struct Environment {
    /// `MINTSWARM_PG_BIN`.
    pub(crate) pg_bin: Option<OsString>,
    /// `PATH`.
    pub(crate) path: Option<OsString>,
    /// `MINTSWARM_PG_IMAGE`.
    pub(crate) image: Option<String>,
    /// Whether the process runs as root, where `initdb` refuses to run.
    pub(crate) is_root: bool,
}

impl Environment {
    /// Reads the environment of the current process.
    pub(crate) fn from_process() -> Self {
        Self {
            pg_bin: std::env::var_os(PG_BIN_VAR),
            path: std::env::var_os("PATH"),
            image: std::env::var(PG_IMAGE_VAR).ok(),
            is_root: rustix::process::geteuid().is_root(),
        }
    }
}

/// Picks a backend: `initdb` and `postgres` from `MINTSWARM_PG_BIN`, then from `PATH`, then
/// from `pg_config --bindir` (run only when `pg_config` is on `PATH`), all skipped as root;
/// then `docker`, then `podman` on `PATH`.
pub(crate) fn discover(env: &Environment) -> Result<Backend, PgHarnessError> {
    if !env.is_root {
        if let Some(bindir) = find_binaries(env) {
            return Ok(Backend::Binaries { bindir });
        }
    }
    for runtime in ["docker", "podman"] {
        if let Some(runtime) = find_on_path(env.path.as_ref(), runtime) {
            let image = env
                .image
                .as_deref()
                .filter(|image| !image.is_empty())
                .unwrap_or(DEFAULT_IMAGE)
                .to_owned();
            return Ok(Backend::Container { runtime, image });
        }
    }
    Err(PgHarnessError::NoBackend)
}

fn find_binaries(env: &Environment) -> Option<PathBuf> {
    if let Some(dir) = env.pg_bin.as_ref().filter(|dir| !dir.is_empty()) {
        let dir = PathBuf::from(dir);
        if has_binaries(&dir) {
            return Some(dir);
        }
    }
    let path = env.path.as_ref()?;
    for dir in std::env::split_paths(path) {
        if has_binaries(&dir) {
            return Some(dir);
        }
    }
    let pg_config = find_on_path(Some(path), "pg_config")?;
    let output = Command::new(pg_config)
        .arg("--bindir")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let dir = PathBuf::from(String::from_utf8(output.stdout).ok()?.trim());
    has_binaries(&dir).then_some(dir)
}

fn has_binaries(dir: &Path) -> bool {
    is_executable(&dir.join("initdb")) && is_executable(&dir.join("postgres"))
}

fn find_on_path(path: Option<&OsString>, name: &str) -> Option<PathBuf> {
    std::env::split_paths(path?)
        .map(|dir| dir.join(name))
        .find(|candidate| is_executable(candidate))
}

fn is_executable(path: &Path) -> bool {
    path.metadata()
        .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    fn fake(dir: &Path, names: &[&str]) {
        for name in names {
            let path = dir.join(name);
            fs::write(&path, "#!/bin/sh\nexit 1\n").unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        }
    }

    fn path_of(dirs: &[&Path]) -> Option<OsString> {
        Some(std::env::join_paths(dirs).unwrap())
    }

    #[test]
    fn empty_environment_has_no_backend() {
        let err = discover(&Environment::default()).unwrap_err();
        assert!(matches!(err, PgHarnessError::NoBackend));
    }

    #[test]
    fn no_backend_message_names_both() {
        let text = PgHarnessError::NoBackend.to_string();
        assert!(text.contains("Postgres binaries"));
        assert!(text.contains("container runtime"));
        assert!(text.contains("nothing is downloaded"));
    }

    #[test]
    fn override_wins_over_path() {
        let over = tempfile::tempdir().unwrap();
        let on_path = tempfile::tempdir().unwrap();
        fake(over.path(), &["initdb", "postgres"]);
        fake(on_path.path(), &["initdb", "postgres"]);
        let env = Environment {
            pg_bin: Some(over.path().into()),
            path: path_of(&[on_path.path()]),
            ..Environment::default()
        };
        assert_eq!(
            discover(&env).unwrap(),
            Backend::Binaries {
                bindir: over.path().to_owned()
            }
        );
    }

    #[test]
    fn binaries_on_path_are_found() {
        let on_path = tempfile::tempdir().unwrap();
        fake(on_path.path(), &["initdb", "postgres", "docker"]);
        let env = Environment {
            path: path_of(&[on_path.path()]),
            ..Environment::default()
        };
        assert_eq!(
            discover(&env).unwrap(),
            Backend::Binaries {
                bindir: on_path.path().to_owned()
            }
        );
    }

    #[test]
    fn initdb_without_postgres_is_not_binaries() {
        let on_path = tempfile::tempdir().unwrap();
        fake(on_path.path(), &["initdb"]);
        let env = Environment {
            path: path_of(&[on_path.path()]),
            ..Environment::default()
        };
        assert!(matches!(discover(&env), Err(PgHarnessError::NoBackend)));
    }

    #[test]
    fn root_falls_through_to_a_container() {
        let on_path = tempfile::tempdir().unwrap();
        fake(on_path.path(), &["initdb", "postgres", "docker"]);
        let env = Environment {
            path: path_of(&[on_path.path()]),
            is_root: true,
            ..Environment::default()
        };
        assert_eq!(
            discover(&env).unwrap(),
            Backend::Container {
                runtime: on_path.path().join("docker"),
                image: DEFAULT_IMAGE.to_owned()
            }
        );
    }

    #[test]
    fn a_runtime_without_binaries_gives_a_container() {
        let on_path = tempfile::tempdir().unwrap();
        fake(on_path.path(), &["podman"]);
        let env = Environment {
            path: path_of(&[on_path.path()]),
            image: Some("postgres:16".to_owned()),
            ..Environment::default()
        };
        assert_eq!(
            discover(&env).unwrap(),
            Backend::Container {
                runtime: on_path.path().join("podman"),
                image: "postgres:16".to_owned()
            }
        );
    }

    #[test]
    fn docker_comes_before_podman() {
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        fake(first.path(), &["podman"]);
        fake(second.path(), &["docker"]);
        let env = Environment {
            path: path_of(&[first.path(), second.path()]),
            ..Environment::default()
        };
        assert!(matches!(
            discover(&env).unwrap(),
            Backend::Container { runtime, .. } if runtime == second.path().join("docker")
        ));
    }

    #[test]
    fn pg_config_bindir_is_used() {
        let bindir = tempfile::tempdir().unwrap();
        let on_path = tempfile::tempdir().unwrap();
        fake(bindir.path(), &["initdb", "postgres"]);
        let script = on_path.path().join("pg_config");
        fs::write(
            &script,
            format!("#!/bin/sh\necho '{}'\n", bindir.path().display()),
        )
        .unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
        let env = Environment {
            path: path_of(&[on_path.path()]),
            ..Environment::default()
        };
        assert_eq!(
            discover(&env).unwrap(),
            Backend::Binaries {
                bindir: bindir.path().to_owned()
            }
        );
    }
}
