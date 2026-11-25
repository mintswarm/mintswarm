//! The mintswarm control-plane server binary.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand, ValueEnum};
use mintswarm_github::KeyStore;
use mintswarm_server::webhooks::{
    self, DEFAULT_SECRET_ENV, DeliveryLog, PATH, Receiver, SecretSource,
};

/// The mintswarm control plane
#[derive(Debug, Parser)]
#[command(name = "mintswarm-server", version, arg_required_else_help = true)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

/// The subcommands. `serve` and `migrate` only parse for now.
#[derive(Debug, Subcommand)]
enum Command {
    /// Run the control-plane server
    Serve,
    /// Apply the database migrations
    Migrate,
    /// Receive the GitHub App's webhooks and append each verified delivery to a log
    Webhooks(WebhooksArgs),
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
    /// The variable holding the webhook secret, used when --key-store is not given
    #[arg(long, default_value = DEFAULT_SECRET_ENV)]
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
    #[error(transparent)]
    Secret(#[from] webhooks::SecretError),
    #[error(transparent)]
    Log(#[from] webhooks::LogError),
    #[error(transparent)]
    Serve(#[from] webhooks::ServeError),
}

fn main() -> ExitCode {
    match Cli::parse().command {
        Command::Serve => ExitCode::SUCCESS,
        Command::Migrate => ExitCode::SUCCESS,
        Command::Webhooks(args) => match run_webhooks(&args) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("mintswarm-server webhooks: {error}");
                ExitCode::FAILURE
            }
        },
    }
}

/// Loads the secret and opens the log before binding, then serves until stopped.
fn run_webhooks(args: &WebhooksArgs) -> Result<(), MainError> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|_| MainError::Runtime)?;
    runtime.block_on(async {
        let secret = webhooks::load_secret(&args.secret_source(), |var| std::env::var_os(var)).await?;
        let log = DeliveryLog::open(&args.log)?;
        let listener = webhooks::bind(args.listen).await?;
        let addr = listener.local_addr().unwrap_or(args.listen);
        eprintln!(
            "mintswarm-server webhooks: listening on http://{addr}{PATH}, appending deliveries to {}",
            args.log.display()
        );
        webhooks::serve(listener, Receiver::new(secret, log)).await?;
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::Cli;
    use clap::CommandFactory;

    #[test]
    fn command_definition_is_valid() {
        Cli::command().debug_assert();
    }
}
