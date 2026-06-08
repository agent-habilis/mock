use std::process::ExitCode;

use clap::{Parser, Subcommand};
use xshell::Shell;

mod ci;
mod clean;
mod coverage;
mod fmt;
mod install;
mod lint;
mod proptest;
mod test;
mod util;

/// Task result; any `Err` is printed and turns into a non-zero exit.
pub(crate) type TaskOutcome = Result<(), Box<dyn std::error::Error>>;

/// Project task runner. Run `cargo task <task>`.
#[derive(Parser)]
#[command(bin_name = "cargo task")]
struct Cli {
    #[command(subcommand)]
    task: Task,
}

/// Variant doc comments *are* the `--help` text — no separate usage block to
/// drift. clap kebab-cases names, so the invocation surface stays stable.
#[derive(Subcommand)]
enum Task {
    /// Run the unit + integration tests.
    Test,
    /// Run the property-based (proptest) tests only.
    Proptest,
    /// Run the CI gate (fmt check + clippy + tests).
    Ci,
    /// Format source files.
    Fmt,
    /// Run clippy lints (`-D warnings`).
    Lint,
    /// Run tests with coverage instrumentation.
    Coverage,
    /// Remove build artifacts.
    Clean,
    /// Build and install the `ahm` binary.
    Install,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let sh = match Shell::new() {
        Ok(sh) => sh,
        Err(error) => {
            agent_habilis_mock::util::output::error(&error.to_string());
            return ExitCode::FAILURE;
        }
    };

    let outcome = match cli.task {
        Task::Test => test::run(&sh),
        Task::Proptest => proptest::run(&sh),
        Task::Ci => ci::run(&sh),
        Task::Fmt => fmt::run(&sh),
        Task::Lint => lint::run(&sh),
        Task::Coverage => coverage::run(&sh),
        Task::Clean => clean::run(&sh),
        Task::Install => install::run(&sh),
    };

    match outcome {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            agent_habilis_mock::util::output::error(&error.to_string());
            ExitCode::FAILURE
        }
    }
}
