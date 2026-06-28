use std::str::FromStr;

use koca::distro::Distro;
use koca::BuildFile;

use crate::cli::CreateArgs;
use crate::error::{CliError, CliMultiError, CliMultiResult};
use crate::handler::plain;

pub async fn run(args: CreateArgs) -> CliMultiResult<()> {
    let build_file_path = match &args.build_file {
        Some(p) => p.clone(),
        None => crate::discover::find_build_file()?,
    };

    let build_file = BuildFile::parse_file(&build_file_path)
        .await
        .map_err(|errs| {
            CliMultiError(errs.into_iter().map(|err| CliError::Koca { err }).collect())
        })?;

    let distro = if let Some(target) = &args.target {
        Distro::from_str(target).map_err(|err| CliMultiError::from(CliError::Koca { err }))?
    } else {
        Distro::detect().map_err(|err| CliMultiError::from(CliError::Koca { err }))?
    };

    // TODO: route to the iocraft TUI handler when stdin && stdout are a terminal;
    // forced to plain for now while the TUI handler is being rebuilt.
    //
    // use std::io::IsTerminal;
    // if std::io::stdin().is_terminal() && std::io::stdout().is_terminal() {
    //     return crate::handler::tui::run(&args, &build_file_path, build_file, &distro).await;
    // }
    plain::run(&args, &build_file_path, build_file, &distro).await
}
