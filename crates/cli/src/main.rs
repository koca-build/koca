use anyhow::Context;
use clap::{Parser, ValueEnum};
use koca::BuildFile;
use std::path::PathBuf;

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

async fn run_build(build_args: &BuildArgs) -> anyhow::Result<()> {
    let build_file = BuildFile::from_file(&build_args.build_file)
        .await
        .context(format!(
            "Failed to read build file: `{}`",
            build_args.build_file.display()
        ))?;
    Ok(())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match cli {
        Cli::Build(build_args) => run_build(&build_args).await?,
    }

    Ok(())
}
