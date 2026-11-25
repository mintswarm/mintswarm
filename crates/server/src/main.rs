//! The mintswarm control-plane server binary.

use clap::Command;

/// The command line: `serve` and `migrate`, which only parse for now.
fn cli() -> Command {
    Command::new("mintswarm-server")
        .version(env!("CARGO_PKG_VERSION"))
        .about("The mintswarm control plane")
        .subcommand_required(true)
        .arg_required_else_help(true)
        .subcommand(Command::new("serve").about("Run the control-plane server"))
        .subcommand(Command::new("migrate").about("Apply the database migrations"))
}

fn main() {
    let matches = cli().get_matches();
    match matches.subcommand() {
        Some(("serve", _)) => {}
        Some(("migrate", _)) => {}
        _ => unreachable!("clap requires a known subcommand"),
    }
}

#[cfg(test)]
mod tests {
    use super::cli;

    #[test]
    fn command_definition_is_valid() {
        cli().debug_assert();
    }
}
