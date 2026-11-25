//! The `mintswarm-runner` binary: for now, the ported dogfood tools under `tool`.

use std::ffi::OsString;
use std::io;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use mintswarm_rules::exit;
use mintswarm_runner::tool::save_output;

/// The mintswarm runner
#[derive(Debug, Parser)]
#[command(name = "mintswarm-runner", version, arg_required_else_help = true)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Run a ported dogfood tool, with the command line of its Python script
    #[command(subcommand)]
    Tool(Tool),
}

#[derive(Debug, Subcommand)]
enum Tool {
    /// Save a stage's output document and result record from its reply or transcript
    #[command(disable_help_flag = true, override_usage = "save-output [--text] INPUT OUTPUT.md")]
    SaveOutput {
        /// `[--text] INPUT OUTPUT.md`, checked by the tool itself
        #[arg(trailing_var_arg = true, allow_hyphen_values = true, num_args = 0..)]
        operands: Vec<OsString>,
    },
}

fn main() -> ExitCode {
    let outcome = match Cli::parse().command {
        Command::Tool(Tool::SaveOutput { .. }) => {
            // clap drops a `--` separator, which save-output.py rejects; the tool checks the
            // arguments after `tool save-output` exactly as given
            let operands: Vec<OsString> = std::env::args_os().skip(3).collect();
            save_output::run(&operands, &mut io::stdout().lock(), &mut io::stderr().lock())
        }
    };
    ExitCode::from(exit::code(&outcome))
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
