//! The mintswarm control-plane server binary.

use clap::{Parser, Subcommand};

/// The mintswarm control plane
#[derive(Debug, Parser)]
#[command(name = "mintswarm-server", version, arg_required_else_help = true)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

/// The subcommands, which only parse for now.
#[derive(Debug, Subcommand)]
enum Command {
    /// Run the control-plane server
    Serve,
    /// Apply the database migrations
    Migrate,
}

fn main() {
    match Cli::parse().command {
        Command::Serve => {}
        Command::Migrate => {}
    }
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
