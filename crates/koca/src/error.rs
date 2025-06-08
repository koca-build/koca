pub use brush::{Error as BrushError, ParseError as BrushParseError};
use brush_parser::ast::Assignment;
use core::error;
use std::io;
use thiserror::Error;

/// A [`Result<T, KocaError>`] type alias.
pub type KocaResult<T> = Result<T, KocaError>;

/// A [`Result<T, Vec<KocaError>>`] type alias.
pub type KocaMultiResult<T> = Result<T, Vec<KocaError>>;

/// Error that occur while parsing a Koca build file.
#[derive(Error, Debug)]
pub enum KocaParserError {
    /// An error while tokenizing the input.
    #[error("Found a syntax error")]
    Tokenizer(#[from] BrushParseError),
    /// A top-level command was provided.
    #[error("A top-level command was provided: {0}")]
    TopLevelCommand(String),
    /// An assignment was made, though it wasn't a string or indexed array.
    #[error("A variable was defined that wasn't a string or indexed array: {0}")]
    InvalidAssignment(Assignment),
    /// An assignment was made on a variable that was already defined.
    #[error("A variable was defined more than once: {0}")]
    DuplicateAssignment(Assignment),
    /// A variable that isn't allowed to perform expansion attempted to do so.
    #[error("The '{0}' variable attempted to perform expansion, but isn't allowed to do so")]
    InvalidExpansion(String),
    /// A variable was expected to be a string, but was not.
    #[error("The '{0}' variable was expected to be a string, but was not")]
    NotString(String),
    /// A variable was expected to be an array, but was not.
    #[error("The '{0}' variable was expected to be an array, but was not")]
    NotArray(String),
    /// An invalid string was specified for a version.
    #[error("'{0}' is not a valid version")]
    InvalidVersion(String),
    /// An invalid string was specified for an architecture.
    #[error("'{0}' is not a valid architecture")]
    InvalidArch(String),
    /// A required variable was not defined.
    #[error("The variable '{0}' was not defined")]
    MissingRequiredVariable(String),
    /// A required function was not defined.
    #[error("The function '{0}' was not defined")]
    MissingRequiredFunction(String),
}

/// Errors that can occur in the Koca library.
#[derive(Error, Debug)]
pub enum KocaError {
    /// An error while parsing the Koca build file.
    #[error("Failed to parse Koca build file")]
    Parser(#[from] KocaParserError),
    /// An error doing an I/O operation.
    #[error("Failed to perform I/O operation")]
    IO(#[from] io::Error),
    /// An error occurred while executing a function.
    #[error("Failed to execute function")]
    FuncExec(#[from] BrushError),
    /// A binary Koca needs to run was unable to be found.
    #[error("The binary '{0}' was not found in the PATH")]
    MissingBinary(String),
}
