use std::path::Path;

use koca::{BuildFile, BundleFormat};

use crate::{
    error::{CliError, CliMultiError, CliMultiResult},
    CreateArgs, OutputType,
};
use zolt::Colorize;

// Run a bundle.
pub async fn run(create_args: CreateArgs) -> CliMultiResult<()> {
    let mut build_file = match BuildFile::parse_file(&create_args.build_file).await {
        Ok(file) => file,
        Err(errs) => {
            return Err(CliMultiError(
                errs.into_iter().map(|err| CliError::Koca { err }).collect(),
            ))
        }
    };

    // Run `build`.
    zolt::infoln!("Running {} stage...", koca::funcs::BUILD.bold().blue());
    if let Err(err) = build_file.run_build().await {
        return Err(CliError::Koca { err }.into());
    }

    // Run `package`.
    zolt::infoln!("Running {} stage...", koca::funcs::PACKAGE.bold().blue());
    if let Err(err) = build_file.run_package().await {
        return Err(CliError::Koca { err }.into());
    }

    // Run the bundle stage.
    let output_targets = match create_args.output_type {
        OutputType::Deb => vec![(BundleFormat::Deb, "deb")],
        OutputType::Rpm => vec![(BundleFormat::Rpm, "rpm")],
        OutputType::All => vec![(BundleFormat::Deb, "deb"), (BundleFormat::Rpm, "rpm")],
    };

    for (bundle_format, file_extension) in output_targets {
        let file_name = format!(
            "{}_{}.{}",
            build_file.pkgname(),
            build_file.version(),
            file_extension
        );

        zolt::infoln!(
            "Creating package into {}{}...",
            "./".blue().bold(),
            file_name.blue().bold()
        );

        if let Err(err) = build_file
            .bundle(bundle_format, Path::new(&file_name))
            .await
        {
            return Err(CliError::Koca { err }.into());
        }
    }

    zolt::infoln!("Package(s) created successfully.");

    Ok(())
}
