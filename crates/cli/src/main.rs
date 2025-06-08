use clap::{Parser, ValueEnum};
use koca::BuildFile;
use std::path::PathBuf;
use zolt::Colorize;

mod bins;
mod build;
mod dirs;
mod error;
mod http;

use error::CliError;

#[derive(Clone, ValueEnum)]
enum OutputType {
    /// The ".deb" output type.
    Deb,
    /// The ".rpm" output type.
    Rpm,
}

#[derive(Parser)]
struct BuildArgs {
    /// The path to the build file.
    build_file: PathBuf,
    /// The output file type.
    #[arg(long, value_enum, default_value_t = OutputType::Deb)]
    output_type: OutputType,
}

#[derive(Parser)]
enum Cli {
    /// Build a package.
    Build(BuildArgs),
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    let output = match cli {
        Cli::Build(build_args) => build::run(build_args).await,
    };

    if let Err(errs) = output {
        for err in errs.0 {
            zolt::errln!("{:?}", anyhow::Error::from(err));
        }
    }
}
