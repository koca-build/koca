mod package;

use crate::{cli::InternalArgs, cli::InternalCommand, error::CliMultiResult};

pub async fn run(args: InternalArgs) -> CliMultiResult<()> {
    match args.command {
        InternalCommand::Package(package_args) => package::run(package_args).await,
    }
}
