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
}

/// A list of [`CliError`] instances.
pub struct CliMultiError(pub Vec<CliError>);

impl From<CliError> for CliMultiError {
    fn from(value: CliError) -> Self {
        Self(vec![value])
    }
}
