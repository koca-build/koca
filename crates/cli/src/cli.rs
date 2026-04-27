use clap::{Parser, Subcommand, ValueEnum};
use koca::BundleFormat;
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

impl OutputType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Deb => "deb",
            Self::Rpm => "rpm",
            Self::All => "all",
        }
    }

    pub fn bundle_formats(&self) -> Vec<BundleFormat> {
        match self {
            Self::Deb => vec![BundleFormat::Deb],
            Self::Rpm => vec![BundleFormat::Rpm],
            Self::All => vec![BundleFormat::Deb, BundleFormat::Rpm],
        }
    }
}

#[derive(Parser)]
pub struct CreateArgs {
    /// The path to the build file.
    pub build_file: PathBuf,
    /// The output file type.
    #[arg(long, value_enum, default_value_t = OutputType::All)]
    pub output_type: OutputType,
    /// Override distro detection (e.g. "arch", "debian:12").
    #[arg(long)]
    pub target: Option<String>,
    /// Remove makedepends after a successful build.
    #[arg(long)]
    pub rm_deps: bool,
    /// Skip interactive confirmation prompts.
    #[arg(long)]
    pub noconfirm: bool,
}

#[derive(Parser)]
pub struct PackageArgs {
    /// The path to the build file.
    pub build_file: PathBuf,
    /// The output file type.
    #[arg(long, value_enum, default_value_t = OutputType::All)]
    pub output_type: OutputType,
    /// Only package these sub-packages (omit to package all).
    #[arg(long)]
    pub package: Vec<String>,
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
