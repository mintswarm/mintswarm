//! The harness's error enum. Errors from other crates and the operating system are kept as
//! sources only; every message is the harness's own.

use std::io;
use std::process::ExitStatus;
use std::time::Duration;

/// Why the harness could not provide a test database.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum PgHarnessError {
    /// Neither Postgres binaries nor a container runtime was found.
    #[error(
        "no Postgres for tests: neither Postgres binaries (initdb and postgres, from \
         MINTSWARM_PG_BIN, PATH or pg_config --bindir) nor a container runtime (docker or \
         podman on PATH) is available; nothing is downloaded"
    )]
    NoBackend,
    /// The container runtime does not hold the image locally, and the harness never pulls, or
    /// the runtime could not be reached to ask.
    #[error(
        "the Postgres image {image} is not present locally or the container runtime could not \
         be reached; nothing is pulled; the runtime's output:\n{output}"
    )]
    ImageMissing {
        /// The image the harness looked for.
        image: String,
        /// What the runtime wrote to standard output and standard error.
        output: String,
    },
    /// The container stopped before it accepted connections.
    #[error("the test Postgres container exited before it was ready; its logs:\n{logs}")]
    ContainerExited {
        /// What the container wrote, as far as the runtime still holds it.
        logs: String,
    },
    /// A program could not be started.
    #[error("could not start {program}")]
    Spawn {
        /// The program that failed to start.
        program: String,
        /// The operating system's error.
        source: io::Error,
    },
    /// A program the harness ran exited unsuccessfully.
    #[error("{program} exited unsuccessfully ({status}); its output:\n{output}")]
    CommandFailed {
        /// The program that failed.
        program: String,
        /// Its exit status.
        status: ExitStatus,
        /// What it wrote to its log, standard output or standard error.
        output: String,
    },
    /// `initdb` could not create the data directory.
    #[error("initdb could not create the test cluster; its log:\n{log}")]
    InitFailed {
        /// The contents of `initdb`'s log.
        log: String,
    },
    /// The instance did not accept connections in time.
    #[error("the test Postgres instance was not ready after {waited:?}")]
    NotReady {
        /// How long the harness waited.
        waited: Duration,
    },
    /// A connection to the instance or a statement on it failed.
    #[error("could not work with the test Postgres instance")]
    Connect {
        /// The client's error.
        source: tokio_postgres::Error,
    },
    /// The migration tree could not be applied to the test database.
    #[error("could not apply the migration tree to the test database")]
    Migrate {
        /// The run's error.
        source: crate::migrations::ApplyError,
    },
    /// A file or process operation failed.
    #[error("a file or process operation of the Postgres test harness failed")]
    Io {
        /// The operating system's error.
        #[from]
        source: io::Error,
    },
}
