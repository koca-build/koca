use clap::{Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

#[derive(Clone, ValueEnum)]
pub enum OutputType {
    /// The ".deb" output type.
    Deb,
    /// The ".rpm" output type.
    Rpm,
    /// Output both ".deb" and ".rpm" package types.
    All,
}

#[derive(Parser)]
pub struct CreateArgs {
    /// The path to the build file.
    pub build_file: PathBuf,
    /// The output file type.
    #[arg(long, value_enum, default_value_t = OutputType::All)]
    pub output_type: OutputType,
}

#[derive(Parser)]
pub struct PackageArgs {
    /// The path to the build file.
    pub build_file: PathBuf,
    /// The output file type.
    #[arg(long, value_enum, default_value_t = OutputType::All)]
    pub output_type: OutputType,
}

#[derive(Parser)]
pub struct InternalArgs {
    #[command(subcommand)]
    pub command: InternalCommand,
}

#[derive(Subcommand)]
pub enum InternalCommand {
    Package(PackageArgs),
}

#[derive(Parser)]
pub enum Cli {
    /// Create a package from a build script.
    Create(CreateArgs),
    /// Internal commands (hidden from help)
    #[command(hide = true)]
    Internal(InternalArgs),
}
