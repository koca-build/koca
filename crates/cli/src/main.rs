#![allow(clippy::result_large_err)]

use clap::{Parser, ValueEnum};
use koca::BundleFormat;
use std::path::PathBuf;

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

impl OutputType {
    /// Get the bundle format for this output type.
    fn to_bundle_format(&self) -> BundleFormat {
        match self {
            Self::Deb => BundleFormat::Deb,
            Self::Rpm => BundleFormat::Rpm,
        }
    }
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
