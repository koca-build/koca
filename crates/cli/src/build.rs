use std::path::{Path, PathBuf};

use koca::{BuildFile, BuildFileOpts};

use crate::{
    bins,
    error::{CliError, CliMultiError, CliMultiResult, CliResult},
    BuildArgs,
};
use zolt::Colorize;

// Run a build.
pub async fn run(build_args: BuildArgs) -> CliMultiResult<()> {
    // Default to the system's binaries for needed programs.
    let mut nfpm_path = PathBuf::from(bins::nfpm::BIN_NAME);
    let mut yq_path = PathBuf::from(bins::yq::BIN_NAME);

    // If `nfpm` isn't installed or isn't a valid version for us, download it.
    if bins::nfpm::needs_install() {
        zolt::infoln!("Caching {}...", bins::nfpm::BIN_NAME.blue().bold());
        let nfpm_bin = match bins::nfpm::download().await {
            Ok(bytes) => bytes,
            Err(err) => return Err(err.into()),
        };
        match bins::nfpm::install(&nfpm_bin) {
            Ok(path) => nfpm_path = path,
            Err(err) => return Err(err.into()),
        }
    }

    // If `yq` isn't installed or isn't a valid version for us, download it.
    if bins::yq::needs_install() {
        zolt::infoln!("Caching {}...", bins::yq::BIN_NAME.blue().bold());
        let yq_bin = match bins::yq::download().await {
            Ok(bytes) => bytes,
            Err(err) => return Err(err.into()),
        };
        match bins::yq::install(&yq_bin) {
            Ok(path) => yq_path = path,
            Err(err) => return Err(err.into()),
        }
    }

    // Parse the build file.
    let build_opts = BuildFileOpts {
        nfpm: nfpm_path,
        yq: yq_path,
    };
    let mut build_file = match BuildFile::parse_file(&build_args.build_file, build_opts).await {
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

    koca::bundle::nfpm::get_nfpm_files(Path::new("koca/pkg"));

    Ok(())
}
