use koca::KocaError;

use thiserror::Error;

// pub type CliResult<T> = Result<T, CliError>;
pub type CliMultiResult<T> = Result<T, CliMultiError>;

#[derive(Error, Debug)]
pub enum CliError {
    /// An error from Koca.
    #[error("Received an error from Koca")]
    Koca {
        #[source]
        err: KocaError,
    },
    /// An IO error.
    #[error("IO error")]
    Io {
        #[source]
        err: std::io::Error,
    },
    /// The fakeroot package phase failed.
    #[error("Package phase failed")]
    PackageFailed,
    /// fakeroot is not installed.
    #[error("fakeroot is not installed or not in PATH")]
    FakerootNotFound,
}

/// A list of [`CliError`] instances.
pub struct CliMultiError(pub Vec<CliError>);

impl From<CliError> for CliMultiError {
    fn from(value: CliError) -> Self {
        Self(vec![value])
    }
}

impl From<std::io::Error> for CliMultiError {
    fn from(err: std::io::Error) -> Self {
        CliError::Io { err }.into()
    }
}
