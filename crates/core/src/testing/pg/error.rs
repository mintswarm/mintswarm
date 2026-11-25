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
    /// The container runtime does not hold the image locally, and the harness never pulls.
    #[error("the Postgres image {image} is not present locally; nothing is pulled")]
    ImageMissing {
        /// The image the harness looked for.
        image: String,
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
    /// A file or process operation failed.
    #[error("a file or process operation of the Postgres test harness failed")]
    Io {
        /// The operating system's error.
        #[from]
        source: io::Error,
    },
}
