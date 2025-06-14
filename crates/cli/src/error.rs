use koca::KocaError;
use std::{io, path::PathBuf};
use zolt::Colorize;

use thiserror::Error;

pub type CliResult<T> = Result<T, CliError>;
pub type CliMultiResult<T> = Result<T, CliMultiError>;

#[derive(Error, Debug)]
pub enum CliError {
    /// An error from Koca.
    #[error("Received an error from Koca")]
    Koca {
        #[source]
        err: KocaError,
    },
    /// An error occurred while downloading a needed binary.
    #[error("Failed to download {}", .bin_name.bold().red())]
    NetworkBinary {
        bin_name: String,
        #[source]
        err: reqwest::Error,
    },
    /// An error occurred while installing a needed binary.
    #[error("Failed to place {} binary", .bin_name.bold().red())]
    InstallBinary {
        bin_name: String,
        #[source]
        err: io::Error,
    },
    /// An error creating Koca's cache directory.
    #[error("Failed to create Koca's cache directory at {}", .path.display().to_string().red().bold())]
    CacheDir {
        path: PathBuf,
        #[source]
        err: io::Error,
    },
    /// An error creating Koca's binary cache directory.
    #[error("Failed to create Koca's binary cache directory at {}", .path.display().to_string().red().bold())]
    CacheBinDir {
        path: PathBuf,
        #[source]
        err: io::Error,
    },
}

/// A list of [`CliError`] instances.
pub struct CliMultiError(pub Vec<CliError>);

impl From<CliError> for CliMultiError {
    fn from(value: CliError) -> Self {
        Self(vec![value])
    }
}
