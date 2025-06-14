use std::{fmt, str::FromStr};

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
    /// The `x86_64` architecture:
    /// The source package requires a specific architecture, as well as the built package (i.e. a proprietary, prebuilt-executable built outside of the Koca build file).
    X86_64,
}

impl FromStr for Arch {
    type Err = KocaError;

    /// Convert a string to an `Arch`.
    ///
    /// Returns [`KocaParserError::InvalidArch`] if the string is not a valid architecture.
    fn from_str(value: &str) -> KocaResult<Self> {
        match value {
            "all" => Ok(Arch::All),
            "any" => Ok(Arch::Any),
            "x86_64" => Ok(Arch::X86_64),
            _ => Err(KocaParserError::InvalidArch(value.to_string()).into()),
        }
    }
}

impl fmt::Display for Arch {
    /// Format the [`Arch`] as a string.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Arch::All => write!(f, "all"),
            Arch::Any => write!(f, "any"),
            Arch::X86_64 => write!(f, "x86_64"),
        }
    }
}
