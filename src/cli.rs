use clap::{Parser, Subcommand};
use std::env;
use std::ffi::OsString;

/// Koca's CLI interface. Usually you'd just run a subprocess and run a Koca
/// binary on the user's system, but this allows you to embed Koca directly into
/// your program if need be.
///
/// # Usage
/// If you'd like to use the arguments from [`std::env::args_os`], call
/// [`Cli::run`]. If you'd like to pass a custom set of arguments, call
/// [`Cli::run_with`].
#[derive(Parser)]
#[command(version, about)]
pub struct Cli {
    #[command(subcommand)]
    command: Command,
}

impl Cli {
    /// Run the CLI with the arguments in [`std::env::args_os`].
    ///
    /// # Returns
    /// The exit code of running the CLI.
    ///
    /// # Errors
    /// [`clap::Error`] is returned if there was an issue parsing the passed
    /// arguments.
    pub fn run() -> clap::error::Result<exitcode::ExitCode> {
        Self::run_with(env::args_os())
    }

    /// Run the CLI with custom arguments.
    ///
    /// # Returns
    /// The exit code of running the CLI.
    ///
    /// # Errors
    /// [`clap::Error`] is returned if there was an issue parsing the passed
    /// arguments.
    pub fn run_with<I, T>(args: I) -> clap::error::Result<exitcode::ExitCode>
    where
        I: IntoIterator<Item = T>,
        T: Into<OsString> + Clone,
    {
        let cli = Cli::try_parse_from(args)?;
        let exit_code = match cli.command {
            Command::Build => todo!(),
        };

        Ok(exit_code)
    }
}

#[derive(Subcommand)]
enum Command {
    #[command(about = tr::tr!("Build a package."))]
    Build,
}
