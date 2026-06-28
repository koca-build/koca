//! The Koca library provides programmatic access to the Koca packaging format and build system.
//!
//! The entry point to the library is the [`BuildFile`].
#![allow(clippy::result_large_err)]

pub mod backend;
pub mod distro;
mod error;
mod file;
pub mod handler;
mod init;
pub mod pm;
pub mod source;
pub use error::*;
pub use file::*;
pub use init::init;
pub use pm::{PackageManager, Plan};
pub use rfpm;
