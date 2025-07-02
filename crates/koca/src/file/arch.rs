use std::str::FromStr;

use crate::{KocaError, KocaParserError, KocaResult};

/// A package's architecture. This can be created from a string using the [`Arch::try_from`] method.
#[derive(Clone, Debug)]
pub enum Arch {
    /// The `all` architecture:
    /// The source package is architecture-agnostic, but the built package is tied to a specific architecture (i.e. a compiled C program).
    All,
    /// The `any` architecture:
    /// The source package is architecture-agnostic, as well as the built package (i.e. a Python script).
    Any,
    /// The `x86_64` architecture (or `amd64` on Debian-based system):
    /// The source package requires a specific architecture, as well as the built package (i.e. a proprietary, prebuilt-executable built outside of the Koca build file).
    X86_64,
}

impl FromStr for Arch {
    type Err = KocaError;

    /// Convert a string to an `Arch`.
    ///
    /// This also takes in Debian-style architecture strings (i.e. `x86_64` or `amd64`).
    ///
    /// Returns [`KocaParserError::InvalidArch`] if the string is not a valid architecture.
    fn from_str(value: &str) -> KocaResult<Self> {
        match value {
            "all" => Ok(Arch::All),
            "any" => Ok(Arch::Any),
            "amd64" | "x86_64" => Ok(Arch::X86_64),
            _ => Err(KocaParserError::InvalidArch(value.to_string()).into()),
        }
    }
}

impl Arch {
    /// Display the [`Arch`] as a string.
    pub fn get_string(&self) -> &'static str {
        match self {
            Arch::All => "all",
            Arch::Any => "any",
            Arch::X86_64 => "x86_64",
        }
    }

    /// Display the [`Arch`] as a Debian-based architecture string.
    pub fn get_deb_string(&self) -> &'static str {
        match self {
            Arch::All => "all",
            Arch::Any => "any",
            Arch::X86_64 => "amd64",
        }
    }
}
