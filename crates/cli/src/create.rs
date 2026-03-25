use koca::BuildFile;
use std::process::Command;

use crate::{
    cli::{CreateArgs, OutputType},
    error::{CliError, CliMultiError, CliMultiResult},
};
use zolt::Colorize;

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

    // Delegate `package` + `bundle` to a fakeroot subprocess.
    let output_type_str = match create_args.output_type {
        OutputType::Deb => "deb",
        OutputType::Rpm => "rpm",
        OutputType::All => "all",
    };

    let exe = std::env::current_exe().map_err(|err| CliError::Io { err })?;
    let status = Command::new("fakeroot")
        .arg(exe)
        .arg("internal")
        .arg("package")
        .arg(&create_args.build_file)
        .arg("--output-type")
        .arg(output_type_str)
        .status()
        .map_err(|err| {
            if err.kind() == std::io::ErrorKind::NotFound {
                CliError::FakerootNotFound
            } else {
                CliError::Io { err }
            }
        })?;

    if !status.success() {
        return Err(CliError::PackageFailed.into());
    }

    Ok(())
}
