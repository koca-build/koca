mod package;

use koca::backend::{self, BackendKind};

use crate::{cli::InternalArgs, cli::InternalCommand, error::CliMultiResult};

pub async fn run(args: InternalArgs) -> CliMultiResult<()> {
    match args.command {
        InternalCommand::Package(package_args) => package::run(package_args).await,
        InternalCommand::BackendApt(args) => {
            backend::run_backend_loop(&args.socket, BackendKind::Apt)
                .await
                .map_err(|e| crate::error::CliError::Io { err: std::io::Error::other(e.to_string()) })?;
            Ok(())
        }
        InternalCommand::BackendAlpm(args) => {
            backend::run_backend_loop(&args.socket, BackendKind::Alpm)
                .await
                .map_err(|e| crate::error::CliError::Io { err: std::io::Error::other(e.to_string()) })?;
            Ok(())
        }
    }
}
