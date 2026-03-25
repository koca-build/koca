#![allow(clippy::result_large_err)]

mod cli;
mod create;
mod error;
mod internal;

use clap::Parser;
use cli::Cli;

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    let output = match cli {
        Cli::Create(create_args) => create::run(create_args).await,
        Cli::Internal(args) => internal::run(args).await,
    };

    if let Err(errs) = output {
        for err in errs.0 {
            zolt::errln!("{:?}", anyhow::Error::from(err));
        }
    }
}
