//! The `mintswarm-runner` binary: `join`, `run` and `add-service` in the runner directory, the
//! current directory, and the ported dogfood tools under `tool`.

use std::ffi::OsString;
use std::io;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use mintswarm_rules::exit::{self, Outcome, Unusable, Verdict};
use mintswarm_runner::runner_dir::{self, ServiceKind};
use mintswarm_runner::tool::{msw_vcs, plan_decision, plan_floor, pr_labels, save_output};

/// The mintswarm runner
#[derive(Debug, Parser)]
#[command(name = "mintswarm-runner", version, arg_required_else_help = true)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Make the current directory a runner directory: its store and a runner.yaml
    Join,
    /// Run in the current directory, which `join` made a runner directory
    Run,
    /// Write a systemd unit or launchd plist that runs the current runner directory
    AddService,
    /// Run a ported dogfood tool, with the command line of its Python script
    #[command(subcommand)]
    Tool(Tool),
}

#[derive(Debug, Subcommand)]
enum Tool {
    /// Save a stage's output document and result record from its reply or transcript
    #[command(
        disable_help_flag = true,
        override_usage = "mintswarm-runner tool save-output [--text] INPUT OUTPUT.md"
    )]
    SaveOutput {
        /// `[--text] INPUT OUTPUT.md`, checked by the tool itself
        #[arg(trailing_var_arg = true, allow_hyphen_values = true, num_args = 0..)]
        operands: Vec<OsString>,
    },
    /// Read the history of the working directory's job: msw-vcs log, diff and show
    #[command(
        name = "msw-vcs",
        disable_help_flag = true,
        override_usage = "mintswarm-runner tool msw-vcs (log | diff | show) ..."
    )]
    MswVcs {
        /// The arguments of `msw-vcs`, checked by the tool itself
        #[arg(trailing_var_arg = true, allow_hyphen_values = true, num_args = 0..)]
        operands: Vec<OsString>,
    },
    /// Apply the plan floor to a plan at a base commit: the port of `plan-approval/floor.py`
    #[command(
        name = "plan-floor",
        disable_help_flag = true,
        override_usage = "mintswarm-runner tool plan-floor --plan PATH --components NAMES --base REF [--issue-document PATH] [--analysis PATH] [--screen PATH]"
    )]
    PlanFloor {
        /// The arguments of `floor.py`, checked by the tool itself
        #[arg(trailing_var_arg = true, allow_hyphen_values = true, num_args = 0..)]
        operands: Vec<OsString>,
    },
    /// Decide whether a plan needs a human: the port of `plan-approval/decision.py`
    #[command(
        name = "plan-decision",
        disable_help_flag = true,
        override_usage = "mintswarm-runner tool plan-decision --issue N --components NAMES --plan PATH --issue-document PATH --analysis PATH --assessment PATH --assessment-artifact REF --base REF --harness NAME --model NAME [--screen PATH]"
    )]
    PlanDecision {
        /// The arguments of `decision.py`, checked by the tool itself
        #[arg(trailing_var_arg = true, allow_hyphen_values = true, num_args = 0..)]
        operands: Vec<OsString>,
    },
    /// Print the label changes for a PR from its diff at a base commit, without calling GitHub
    #[command(
        name = "pr-labels",
        disable_help_flag = true,
        override_usage = "mintswarm-runner tool pr-labels --base COMMIT --pr-labels PATH --repo-labels PATH [--head COMMIT] [--band BAND]"
    )]
    PrLabels {
        /// The tool's arguments, checked by the tool itself
        #[arg(trailing_var_arg = true, allow_hyphen_values = true, num_args = 0..)]
        operands: Vec<OsString>,
    },
}

/// The arguments after `tool <name>`, exactly as given, and the working directory; an unreadable
/// working directory is then not in a repository.
fn operands_and_cwd() -> (Vec<OsString>, PathBuf) {
    let argv: Vec<OsString> = std::env::args_os().skip(3).collect();
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    (argv, cwd)
}

/// `error` and its sources, each after a `: `.
fn chain(error: &dyn std::error::Error) -> String {
    let mut text = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        text.push_str(&format!(": {cause}"));
        source = cause.source();
    }
    text
}

/// The current directory, the runner directory of every command.
fn runner_dir() -> Result<PathBuf, Unusable> {
    std::env::current_dir()
        .map_err(|err| Unusable::new(format!("cannot read the current directory: {err}")))
}

/// `mintswarm-runner join`.
fn join() -> Result<Verdict, Unusable> {
    let dir = runner_dir()?;
    runner_dir::claim(&dir).map_err(|err| Unusable::new(chain(&err)))?;
    println!("joined {}", dir.display());
    Ok(Verdict::Clean)
}

/// `mintswarm-runner run`: hold the runner directory until a signal ends the process.
fn run() -> Unusable {
    let dir = match runner_dir() {
        Ok(dir) => dir,
        Err(err) => return err,
    };
    match runner_dir::open(&dir) {
        Ok((_config, _store)) => {
            eprintln!("mintswarm-runner: running in {}", dir.display());
            // The job loop attaches here. Until then the store, and with it the lock, is held
            // until SIGINT or SIGTERM ends the process and the kernel drops the lock.
            loop {
                std::thread::park();
            }
        }
        Err(err) => Unusable::new(chain(&err)),
    }
}

/// `mintswarm-runner add-service`.
fn add_service() -> Result<Verdict, Unusable> {
    let dir = runner_dir()?;
    let binary = std::env::current_exe()
        .and_then(std::path::absolute)
        .map_err(|err| Unusable::new(format!("cannot find this binary: {err}")))?;
    let (path, instructions) = runner_dir::add_service(&dir, &binary, ServiceKind::host())
        .map_err(|err| Unusable::new(chain(&err)))?;
    println!("wrote {}\n\n{instructions}", path.display());
    Ok(Verdict::Clean)
}

/// Print an unusable outcome's message to standard error.
fn report(outcome: Outcome) -> Outcome {
    if let Err(err) = &outcome {
        eprintln!("mintswarm-runner: {err}");
    }
    outcome
}

fn main() -> ExitCode {
    let outcome = match Cli::parse().command {
        Command::Join => report(join()),
        Command::Run => report(Err(run())),
        Command::AddService => report(add_service()),
        Command::Tool(Tool::SaveOutput { .. }) => {
            // clap drops a `--` separator, which save-output.py rejects; the tool checks the
            // arguments after `tool save-output` exactly as given
            let operands: Vec<OsString> = std::env::args_os().skip(3).collect();
            save_output::run(
                &operands,
                &mut io::stdout().lock(),
                &mut io::stderr().lock(),
            )
        }
        Command::Tool(Tool::MswVcs { .. }) => {
            // as for save-output: the arguments after `tool msw-vcs`, exactly as given
            let (argv, cwd) = operands_and_cwd();
            let code = msw_vcs::run(
                &argv,
                &cwd,
                &mut io::stdout().lock(),
                &mut io::stderr().lock(),
            );
            return ExitCode::from(code);
        }
        Command::Tool(Tool::PlanFloor { .. }) => {
            let (argv, cwd) = operands_and_cwd();
            plan_floor::run(
                &argv,
                &cwd,
                &mut io::stdout().lock(),
                &mut io::stderr().lock(),
            )
        }
        Command::Tool(Tool::PlanDecision { .. }) => {
            let (argv, cwd) = operands_and_cwd();
            plan_decision::run(
                &argv,
                &cwd,
                &mut io::stdout().lock(),
                &mut io::stderr().lock(),
            )
        }
        Command::Tool(Tool::PrLabels { .. }) => {
            let (argv, cwd) = operands_and_cwd();
            pr_labels::run(
                &argv,
                &cwd,
                &mut io::stdout().lock(),
                &mut io::stderr().lock(),
            )
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
