//! The Koca library provides programmatic access to the Koca packaging format and build system.
//!
//! The entry point to the library is the [`BuildFile`].
#![allow(clippy::result_large_err)]

mod error;
mod file;
mod nfpm;
pub use error::*;
pub use file::*;
