#![cfg_attr(docsrs, feature(doc_cfg))]
//! Koca is a modern, universal, and system-native package manager.
//!
//! This library provides a way to interact with Koca programmatically.
#[cfg(feature = "cli")]
mod cli;
#[cfg_attr(docsrs, doc(cfg(feature = "cli")))]
#[cfg(feature = "cli")]
pub use cli::Cli;
