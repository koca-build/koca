use std::path::Path;

use koca::BuildFile;

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
    let file_extension = match create_args.output_type {
        OutputType::Deb => "deb",
        OutputType::Rpm => "rpm",
    };
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
    let bundle_res = build_file
        .bundle(
            create_args.output_type.to_bundle_format(),
            Path::new(&file_name),
        )
        .await;
    if let Err(err) = bundle_res {
        return Err(CliError::Koca { err }.into());
    }

    zolt::infoln!("Package created successfully.");

    Ok(())
}
