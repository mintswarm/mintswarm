//! `mintswarm-server serve`: the control plane's process. It refuses a listen address that is
//! not loopback, binds, serves `/healthz` and `/readyz`, and starts the plane under its leader
//! lock, running until it is stopped or loses the lock.

use std::future::{Future, IntoFuture};
use std::io;
use std::net::SocketAddr;
use std::time::Duration;

use axum::Router;
use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::get;
use mintswarm_config::PlaneConfig;
use mintswarm_core::plane::{self, DatabaseConfigError, Intervals, Readiness, StartError};
use tokio::sync::{oneshot, watch};

use crate::webhooks;

/// How long a stop waits for the health requests in progress.
const STOP_DEADLINE: Duration = Duration::from_secs(5);

/// Why `serve` could not run or stopped with an error.
#[derive(Debug, thiserror::Error)]
pub enum ServeError {
    /// The listen address is not loopback, and nothing could check who calls it.
    #[error("no auth provider is configured, so serve listens only on a loopback address")]
    NoAuthProvider,
    /// The database settings do not give a connection.
    #[error(transparent)]
    Database(#[from] DatabaseConfigError),
    /// The address could not be bound.
    #[error(transparent)]
    Listen(webhooks::ServeError),
    /// The plane could not start once it held the leader lock.
    #[error(transparent)]
    Start(#[from] StartError),
    /// The HTTP server stopped with an error.
    #[error("the HTTP server stopped: {reason}")]
    Serve {
        /// Why, in our words.
        reason: &'static str,
        /// The underlying error, when there is one.
        source: Option<io::Error>,
    },
    /// The leader lock's session ended or stopped answering, so another instance may lead.
    #[error("the leader lock was lost; another instance may lead now")]
    LeaderLost,
}

/// The health routes: `GET /healthz` answers `200` while the process runs, and `GET /readyz`
/// answers `200` when `readiness` is ready, and `503` otherwise, with a body naming the failing
/// checks. Other routes may be merged into it.
pub fn router(readiness: watch::Receiver<Readiness>) -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .with_state(readiness)
}

async fn healthz() -> &'static str {
    "ok\n"
}

async fn readyz(State(readiness): State<watch::Receiver<Readiness>>) -> (StatusCode, String) {
    let state = *readiness.borrow();
    if state.is_ready() {
        (StatusCode::OK, "ready\n".to_owned())
    } else {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            format!("not ready: {}\n", state.failing().join(", ")),
        )
    }
}

/// Refuses every address that is not loopback: no auth provider exists yet, so nothing could
/// check who calls the plane.
pub fn check_listen(addr: SocketAddr) -> Result<(), ServeError> {
    if addr.ip().is_loopback() {
        Ok(())
    } else {
        Err(ServeError::NoAuthProvider)
    }
}

/// Runs the plane of `config` until `stop` resolves.
///
/// It checks the listen address before anything connects, binds, prints the address on
/// stderr, and serves [`router`]. It then starts the plane as [`plane::start`] does: while it
/// waits for the leader lock it is a standby, whose `/readyz` answers `503`. A stop at any point
/// shuts the server down and returns `Ok`. An error from the start or the server, or a lost
/// lock once leader, shuts it down and returns the error.
pub async fn run(
    config: PlaneConfig,
    stop: impl Future<Output = ()> + Send,
) -> Result<(), ServeError> {
    check_listen(config.listen)?;
    let database = plane::database_config(&config.database)?;
    let listener = webhooks::bind(config.listen)
        .await
        .map_err(ServeError::Listen)?;
    let addr = listener.local_addr().unwrap_or(config.listen);
    eprintln!("mintswarm-server serve: listening on http://{addr}");

    let (readiness_tx, readiness) = watch::channel(Readiness::default());
    let (shutdown, shutdown_rx) = oneshot::channel::<()>();
    let mut server = tokio::spawn(
        axum::serve(listener, router(readiness))
            .with_graceful_shutdown(async move {
                let _ = shutdown_rx.await;
            })
            .into_future(),
    );

    tokio::pin!(stop);
    let start = plane::start(
        &database,
        &config.projects,
        &readiness_tx,
        Intervals::default(),
    );
    let started = tokio::select! {
        started = start => started.map_err(ServeError::from),
        () = &mut stop => return finish(server, shutdown, Ok(())).await,
        served = &mut server => return Err(server_error(served)),
    };
    let mut leader = match started {
        Ok(leader) => leader,
        Err(error) => return finish(server, shutdown, Err(error)).await,
    };
    let result = tokio::select! {
        () = &mut stop => Ok(()),
        () = leader.lost() => Err(ServeError::LeaderLost),
        served = &mut server => return Err(server_error(served)),
    };
    drop(leader);
    finish(server, shutdown, result).await
}

/// Shuts the server down, waits up to [`STOP_DEADLINE`] for the requests in progress, and gives
/// `result`, or the server's error when `result` is `Ok`.
async fn finish(
    server: tokio::task::JoinHandle<io::Result<()>>,
    shutdown: oneshot::Sender<()>,
    result: Result<(), ServeError>,
) -> Result<(), ServeError> {
    let _ = shutdown.send(());
    let served = tokio::time::timeout(STOP_DEADLINE, server).await;
    match (result, served) {
        (Err(error), _) => Err(error),
        (Ok(()), Ok(served)) => match served {
            Ok(Ok(())) => Ok(()),
            served => Err(server_error(served)),
        },
        // Requests still open at the deadline are left; the process is stopping.
        (Ok(()), Err(_)) => Ok(()),
    }
}

/// The error of a server task that ended.
fn server_error(served: Result<io::Result<()>, tokio::task::JoinError>) -> ServeError {
    match served {
        Ok(Ok(())) => ServeError::Serve {
            reason: "it ended without being stopped",
            source: None,
        },
        Ok(Err(source)) => ServeError::Serve {
            reason: "an I/O error",
            source: Some(source),
        },
        Err(_) => ServeError::Serve {
            reason: "its task failed",
            source: None,
        },
    }
}
