//! The mintswarm control-plane server binary.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use clap::builder::RangedU64ValueParser;
use clap::{Args, Parser, Subcommand, ValueEnum};
use mintswarm_config::{PlaneConfig, load_plane_config};
use mintswarm_core::{migrations, plane};
use mintswarm_github::KeyStore;
use mintswarm_server::serve;
use mintswarm_server::webhooks::{
    self, DEFAULT_IN_FLIGHT, DEFAULT_QUEUED, DEFAULT_READ_TIMEOUT, DEFAULT_SECRET_ENV, DeliveryLog,
    Limits, PATH, Receiver, SecretSource, Stopped,
};
use tokio::signal::unix::{Signal, SignalKind, signal};
use tokio::sync::oneshot;

/// The mintswarm control plane
#[derive(Debug, Parser)]
#[command(name = "mintswarm-server", version, arg_required_else_help = true)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

/// The subcommands.
#[derive(Debug, Subcommand)]
enum Command {
    /// Run the control-plane server
    Serve(ServeArgs),
    /// Apply the database migrations, or create a new one
    Migrate(MigrateArgs),
    /// Receive the GitHub App's webhooks and append each verified delivery to a log
    Webhooks(WebhooksArgs),
}

/// The default path of the plane's settings, in the current directory.
const DEFAULT_CONFIG: &str = "mintswarm.yaml";

/// The `serve` options.
#[derive(Debug, Args)]
struct ServeArgs {
    /// The plane's settings file
    #[arg(long, default_value = DEFAULT_CONFIG)]
    config: PathBuf,
}

/// The `migrate` options.
#[derive(Debug, Args)]
struct MigrateArgs {
    /// The plane's settings file, of which bare `migrate` uses the database
    #[arg(long, default_value = DEFAULT_CONFIG)]
    config: PathBuf,
    #[command(subcommand)]
    action: Option<MigrateAction>,
}

/// The `migrate` subcommands.
#[derive(Debug, Subcommand)]
enum MigrateAction {
    /// Create an empty migration named for the current UTC second, or the next free one, and
    /// print its path
    New {
        /// The migration's name: letters, digits and underscores
        name: String,
        /// The migration tree to create it in
        #[arg(long, default_value = "crates/core/migrations")]
        tree: PathBuf,
    },
}

/// The `webhooks` options.
#[derive(Debug, Args)]
struct WebhooksArgs {
    /// The address to listen on, such as 127.0.0.1:8787
    #[arg(long)]
    listen: SocketAddr,
    /// The file each verified delivery is appended to, as one JSON line
    #[arg(long)]
    log: PathBuf,
    /// The variable holding the webhook secret; cannot be given with --key-store
    #[arg(long, default_value = DEFAULT_SECRET_ENV, conflicts_with = "key_store")]
    secret_env: String,
    /// Read the webhook secret from this key store instead of the variable
    #[arg(long, value_enum, requires_all = ["key_service", "key_account"])]
    key_store: Option<StoreArg>,
    /// The key store's service name for the secret
    #[arg(long, requires = "key_store")]
    key_service: Option<String>,
    /// The key store's account name for the secret
    #[arg(long, requires = "key_store")]
    key_account: Option<String>,
    /// The most requests read at once, each holding up to 25 MiB until its signature is
    /// checked; one more gets 429
    #[arg(long, default_value_t = DEFAULT_IN_FLIGHT, value_parser = RangedU64ValueParser::<usize>::new().range(1..))]
    max_in_flight: usize,
    /// The most verified deliveries waiting for the log; one more gets 429
    #[arg(long, default_value_t = DEFAULT_QUEUED, value_parser = RangedU64ValueParser::<usize>::new().range(1..))]
    max_queued: usize,
    /// The most seconds a request may take to arrive whole and be answered, which is GitHub's own
    /// delivery timeout by default; a slower one gets 408 and gives back its place. A stop waits
    /// twice this for the requests in progress
    #[arg(long, value_name = "SECONDS", default_value_t = DEFAULT_READ_TIMEOUT.as_secs(), value_parser = RangedU64ValueParser::<u64>::new().range(1..))]
    read_timeout: u64,
}

/// The key stores `key-store.sh` writes to.
#[derive(Clone, Copy, Debug, ValueEnum)]
enum StoreArg {
    Keychain,
    SecretService,
    Pass,
    SystemdCreds,
    File,
}

impl From<StoreArg> for KeyStore {
    fn from(store: StoreArg) -> Self {
        match store {
            StoreArg::Keychain => Self::Keychain,
            StoreArg::SecretService => Self::SecretService,
            StoreArg::Pass => Self::Pass,
            StoreArg::SystemdCreds => Self::SystemdCreds,
            StoreArg::File => Self::File,
        }
    }
}

impl WebhooksArgs {
    fn limits(&self) -> Limits {
        Limits {
            in_flight: self.max_in_flight,
            queued: self.max_queued,
            read_timeout: Duration::from_secs(self.read_timeout),
        }
    }

    fn secret_source(&self) -> SecretSource {
        match (self.key_store, &self.key_service, &self.key_account) {
            (Some(store), Some(service), Some(account)) => SecretSource::KeyStore {
                store: store.into(),
                service: service.clone(),
                account: account.clone(),
            },
            _ => SecretSource::Env {
                var: self.secret_env.clone(),
            },
        }
    }
}

/// The catch-all error of the binary's top level.
#[derive(Debug, thiserror::Error)]
enum MainError {
    #[error("cannot start the runtime")]
    Runtime,
    #[error("cannot watch for SIGINT and SIGTERM")]
    Signals,
    #[error(transparent)]
    Secret(#[from] webhooks::SecretError),
    #[error(transparent)]
    Log(#[from] webhooks::LogError),
    #[error(transparent)]
    Serve(#[from] webhooks::ServeError),
    #[error("cannot read {}", path.display())]
    ReadConfig { path: PathBuf },
    #[error("cannot load {}: {source}", path.display())]
    Config {
        path: PathBuf,
        source: mintswarm_config::Error,
    },
    #[error(transparent)]
    Plane(#[from] serve::ServeError),
    #[error(transparent)]
    Database(#[from] plane::DatabaseConfigError),
    #[error(transparent)]
    Migrate(#[from] migrations::ApplyError),
}

fn main() -> ExitCode {
    match Cli::parse().command {
        Command::Serve(args) => exit_code("serve", run_serve(&args)),
        Command::Migrate(args) => run_migrate(args),
        Command::Webhooks(args) => exit_code("webhooks", run_webhooks(&args)),
    }
}

/// Reads and loads the plane's settings at `path`, with the process's environment as the
/// overrides.
fn load_config(path: &std::path::Path) -> Result<PlaneConfig, MainError> {
    let text = std::fs::read_to_string(path).map_err(|_| MainError::ReadConfig {
        path: path.to_path_buf(),
    })?;
    load_plane_config(&text, |name| std::env::var_os(name)).map_err(|source| MainError::Config {
        path: path.to_path_buf(),
        source,
    })
}

/// A multi-threaded runtime with every driver on.
fn runtime() -> Result<tokio::runtime::Runtime, MainError> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|_| MainError::Runtime)
}

/// Loads the settings and runs the plane until SIGINT (Ctrl-C) or SIGTERM, or until it fails or
/// loses the leader lock. The signals are watched before it listens, so a stop always takes the
/// graceful path.
fn run_serve(args: &ServeArgs) -> Result<(), MainError> {
    let config = load_config(&args.config)?;
    runtime()?.block_on(async {
        let mut stop = Stop::watch()?;
        let stopped = async move {
            stop.next().await;
            eprintln!("mintswarm-server serve: stopping");
        };
        serve::run(config, stopped).await?;
        Ok(())
    })
}

/// Runs `migrate`: bare, it applies the embedded migrations to the database of the settings
/// and prints how many it applied; `new` mints a migration and prints its path.
fn run_migrate(args: MigrateArgs) -> ExitCode {
    let Some(MigrateAction::New { name, tree }) = args.action else {
        return exit_code("migrate", apply_migrations(&args.config));
    };
    let minted = match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(now) => migrations::mint(&tree, &name, now.as_secs()).map_err(|error| error.to_string()),
        Err(_) => Err("the system clock is before 1970".to_owned()),
    };
    match minted {
        Ok(path) => {
            println!("{}", path.display());
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("mintswarm-server migrate new: {error}");
            ExitCode::FAILURE
        }
    }
}

/// Applies the embedded migrations to the database the settings at `config` name. The whole
/// file must load, though only `database` is used.
fn apply_migrations(config: &std::path::Path) -> Result<(), MainError> {
    let config = load_config(config)?;
    let database = plane::database_config(&config.database)?;
    let outcome = runtime()?.block_on(plane::migrate(&database))?;
    for warning in &outcome.warnings {
        eprintln!("mintswarm-server migrate: warning: {warning}");
    }
    println!("applied {} migration(s)", outcome.applied.len());
    Ok(())
}

/// Reports a failed run of `subcommand` on stderr, and gives the exit code: 0 when it
/// succeeded, 1 otherwise.
fn exit_code(subcommand: &str, result: Result<(), MainError>) -> ExitCode {
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("mintswarm-server {subcommand}: {error}");
            ExitCode::FAILURE
        }
    }
}

/// Loads the secret and opens the log before binding, then serves until SIGINT (Ctrl-C) or
/// SIGTERM. A stop waits for the requests in progress until the stop deadline or a second
/// signal, drains the queue to the log, and says on stderr how it ended. When the log writer
/// failed, deliveries answered `202` may be lost, and the run fails with
/// [`webhooks::ServeError::Writer`] instead.
fn run_webhooks(args: &WebhooksArgs) -> Result<(), MainError> {
    runtime()?.block_on(async {
        let secret = webhooks::load_secret(&args.secret_source(), |var| std::env::var_os(var)).await?;
        let log = DeliveryLog::open(&args.log)?;
        let stop = Stop::watch()?;
        let listener = webhooks::bind(args.listen).await?;
        let addr = listener.local_addr().unwrap_or(args.listen);
        eprintln!(
            "mintswarm-server webhooks: listening on http://{addr}{PATH}, appending deliveries to {}",
            args.log.display()
        );
        let limits = args.limits();
        let deadline = limits.stop_deadline().as_secs();
        let (requested, forced) = stop.spawn(deadline);
        let (receiver, writer) = Receiver::start(secret, log, limits);
        let stopped = webhooks::serve(listener, receiver, writer, requested, forced).await?;
        eprintln!("{}", stopped_line(stopped, deadline));
        Ok(())
    })
}

/// The last stderr line of a stop whose queue was drained, after a stop deadline of `deadline`
/// seconds.
fn stopped_line(stopped: Stopped, deadline: u64) -> String {
    let how = match stopped {
        Stopped::Drained => "every request in progress was answered".to_owned(),
        Stopped::DeadlinePassed => format!(
            "the {deadline}-second stop deadline passed, and the requests still in progress were left unanswered"
        ),
        Stopped::Forced => {
            "a second signal came, and the requests still in progress were left unanswered"
                .to_owned()
        }
    };
    format!("mintswarm-server webhooks: stopped; {how}; every queued delivery was handled")
}

/// The signals that stop `webhooks` or `serve`, watched from before either listens, so a stop
/// always takes the subcommand's graceful path instead of killing it outright.
struct Stop {
    interrupt: Signal,
    terminate: Signal,
}

impl Stop {
    fn watch() -> Result<Self, MainError> {
        let watch = |kind| signal(kind).map_err(|_| MainError::Signals);
        Ok(Self {
            interrupt: watch(SignalKind::interrupt())?,
            terminate: watch(SignalKind::terminate())?,
        })
    }

    /// Resolves on the next SIGINT or SIGTERM.
    async fn next(&mut self) {
        tokio::select! {
            _ = self.interrupt.recv() => {}
            _ = self.terminate.recv() => {}
        }
    }

    /// Watches the signals on a task, and gives two futures: the first resolves on the first
    /// signal, the second on the one after it. A stop waits up to `deadline` seconds.
    fn spawn(
        mut self,
        deadline: u64,
    ) -> (
        impl Future<Output = ()> + Send + 'static,
        impl Future<Output = ()> + Send + 'static,
    ) {
        let (first_tx, first) = oneshot::channel::<()>();
        let (second_tx, second) = oneshot::channel::<()>();
        tokio::spawn(async move {
            self.next().await;
            eprintln!(
                "mintswarm-server webhooks: stopping; waiting up to {deadline} seconds for the requests in progress (a second Ctrl-C or SIGTERM stops waiting), then draining the queue"
            );
            let _ = first_tx.send(());
            self.next().await;
            eprintln!(
                "mintswarm-server webhooks: second signal; no longer waiting for the requests in progress, draining the queue"
            );
            let _ = second_tx.send(());
        });
        let requested = async move {
            // The task only ends early if the runtime is shutting down, which is a stop too.
            let _ = first.await;
        };
        let forced = async move {
            if second.await.is_err() {
                std::future::pending::<()>().await;
            }
        };
        (requested, forced)
    }
}

#[cfg(test)]
mod tests {
    use std::process::ExitCode;

    use super::{Cli, MainError, exit_code};
    use clap::CommandFactory;
    use mintswarm_server::webhooks::ServeError;

    #[test]
    fn command_definition_is_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn a_failed_log_writer_fails_the_run() {
        let failed = MainError::Serve(ServeError::Writer {
            reason: "it panicked",
        });
        assert_eq!(exit_code("webhooks", Err(failed)), ExitCode::FAILURE);
        assert_eq!(exit_code("webhooks", Ok(())), ExitCode::SUCCESS);
    }
}
