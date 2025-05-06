use clap::Parser;
use std::path::PathBuf;

#[derive(Parser)]
struct BuildArgs {
    /// The path to the build file.
    build_file: PathBuf,
}

#[derive(Parser)]
enum Cli {
    /// Build a package.
    Build(BuildArgs),
}

fn main() {
    let cli = Cli::parse();
}
