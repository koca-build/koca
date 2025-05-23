pub use brush::{Error as BrushError, ParseError as BrushParseError};
use std::io;
use thiserror::Error;

/// A [`Result<T, KocaError>`] type alias.
pub type KocaResult<T> = std::result::Result<T, KocaError>;

/// Errors that can occur in the Koca library.
#[derive(Error, Debug)]
pub enum KocaError {
    /// An error from the Koca parser.
    #[error("Failed to parse koca build file")]
    Parser(#[from] BrushParseError),
    /// An error from the Koca shell handler.
    #[error("Failed to handle koca build file")]
    Shell(#[from] BrushError),
    /// An error doing an I/O operation.
    #[error("Failed to perform I/O operation")]
    IO(#[from] io::Error),
}
