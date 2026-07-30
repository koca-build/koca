#![allow(clippy::result_large_err)]

mod cli;
mod components;
mod create;
mod discover;
mod error;
mod handler;

use clap::Parser;
use cli::Cli;
use error::CliError;
use koca::KocaError;

fn main() {
    koca::init();

    let runtime = tokio::runtime::Runtime::new().expect("failed to build tokio runtime");
    let exit_code = runtime.block_on(run());
    std::process::exit(exit_code);
}

async fn run() -> i32 {
    let cli = Cli::parse();

    let output = match cli {
        Cli::Create(create_args) => create::run(create_args).await,
    };

    if let Err(errs) = output {
        for err in errs.0 {
            match err {
                // A failed/cancelled elevation is a user-facing condition, not a
                // bug — show it as one clean line, no error chain.
                CliError::Koca {
                    err: KocaError::ElevationFailed,
                } => {
                    zolt::errln!("{}", KocaError::ElevationFailed);
                }
                err => zolt::errln!("{:?}", anyhow::Error::from(err)),
            }
        }
        return 1;
    }
    0
}
