pub use brush::{Error as BrushError, ParseError as BrushParseError};
use brush_parser::ast::Assignment;
use std::io;
use thiserror::Error;

/// A [`Result<T, KocaError>`] type alias.
pub type KocaResult<T> = std::result::Result<T, KocaError>;

/// Error that occur while parsing a Koca build file.
#[derive(Error, Debug)]
pub enum KocaParserError {
    /// An error while tokenizing the input.
    #[error("Failed to parse Koca build file")]
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
}
