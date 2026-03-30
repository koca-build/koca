use std::path::Path;

use koca::{BuildFile, BundleFormat};

use crate::{
    cli::{OutputType, PackageArgs},
    error::{CliError, CliMultiError, CliMultiResult},
};
use zolt::Colorize;

pub async fn run(args: PackageArgs) -> CliMultiResult<()> {
    let mut build_file = match BuildFile::parse_file(&args.build_file).await {
        Ok(file) => file,
        Err(errs) => {
            return Err(CliMultiError(
                errs.into_iter().map(|err| CliError::Koca { err }).collect(),
            ))
        }
    };

    // Run `package`.
    zolt::infoln!("Running {} stage...", koca::funcs::PACKAGE.bold().blue());
    if let Err(err) = build_file.run_package().await {
        return Err(CliError::Koca { err }.into());
    }

    // Run the bundle stage.
    let output_targets = match args.output_type {
        OutputType::Deb => vec![(BundleFormat::Deb, "deb")],
        OutputType::Rpm => vec![(BundleFormat::Rpm, "rpm")],
        OutputType::All => vec![(BundleFormat::Deb, "deb"), (BundleFormat::Rpm, "rpm")],
    };

    for (bundle_format, file_extension) in output_targets {
        let arch = build_file.arch()[0].clone();
        let arch_str = match bundle_format {
            BundleFormat::Deb => arch.get_deb_string(),
            BundleFormat::Rpm => arch.get_rpm_string(),
        };

        let file_name = format!(
            "{}_{}_{}.{}",
            build_file.pkgname(),
            build_file.version(),
            arch_str,
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
