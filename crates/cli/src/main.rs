#![allow(clippy::result_large_err)]

use clap::{Parser, ValueEnum};
use std::path::PathBuf;

mod create;
mod error;

#[derive(Clone, ValueEnum)]
enum OutputType {
    /// The ".deb" output type.
    Deb,
    /// The ".rpm" output type.
    Rpm,
    /// Output both ".deb" and ".rpm" package types.
    All,
}

#[derive(Parser)]
struct CreateArgs {
    /// The path to the build file.
    build_file: PathBuf,
    /// The output file type.
    #[arg(long, value_enum, default_value_t = OutputType::Deb)]
    output_type: OutputType,
}

#[derive(Parser)]
enum Cli {
    /// Create a package from a build script.
    Create(CreateArgs),
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    let output = match cli {
        Cli::Create(create_args) => create::run(create_args).await,
    };

    if let Err(errs) = output {
        for err in errs.0 {
            zolt::errln!("{:?}", anyhow::Error::from(err));
        }
    }
}
