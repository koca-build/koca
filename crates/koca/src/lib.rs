//! The Koca library provides programatic access to the Koca packaging format and build system.
//!
//! The entry point to the library is the [`BuildFile`].
mod error;
mod file;

pub use error::*;
pub use file::*;
