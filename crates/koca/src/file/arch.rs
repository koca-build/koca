use std::str::FromStr;

use crate::{KocaError, KocaParserError, KocaResult};

/// A package's architecture. This can be created from a string using the [`Arch::try_from`] method.
#[derive(Clone, Debug)]
pub enum Arch {
    /// The `all` architecture:
    /// The source package is architecture-agnostic, as well as the built package (i.e. a Python script).
    All,
    /// The `any` architecture:
    /// The source package is architecture-agnostic, but the built package is tied to a specific architecture (i.e. a compiled C program).
    Any,
    /// The `x86_64` architecture (or `amd64` on Debian-based system):
    /// The source package requires a specific architecture, as well as the built package (i.e. a proprietary, prebuilt-executable built outside of the Koca build file).
    X86_64,
    /// The `aarch64` architecture (or `arm64` on Debian-based systems):
    /// The source package requires a specific architecture, as well as the built package.
    Aarch64,
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
            "arm64" | "aarch64" => Ok(Arch::Aarch64),
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
            Arch::Aarch64 => "aarch64",
        }
    }

    /// Display the [`Arch`] as a Debian-based architecture string.
    pub fn get_deb_string(&self) -> &'static str {
        match self {
            Arch::All => "all",
            Arch::Any => match std::env::consts::ARCH {
                "x86_64" => "amd64",
                "aarch64" => "arm64",
                other => panic!("unsupported architecture: {other}"),
            },
            Arch::X86_64 => "amd64",
            Arch::Aarch64 => "arm64",
        }
    }

    /// Display the [`Arch`] as an RPM-based architecture string.
    pub fn get_rpm_string(&self) -> &'static str {
        match self {
            Arch::All => "noarch",
            Arch::Any => match std::env::consts::ARCH {
                "x86_64" => "x86_64",
                "aarch64" => "aarch64",
                other => panic!("unsupported architecture: {other}"),
            },
            Arch::X86_64 => "x86_64",
            Arch::Aarch64 => "aarch64",
        }
    }

    /// Convert to the rfpm [`Arch`](rfpm::Arch) type.
    pub fn to_rfpm(&self) -> rfpm::Arch {
        match self {
            Arch::All => rfpm::Arch::All,
            Arch::Any => match std::env::consts::ARCH {
                "x86_64" => rfpm::Arch::Amd64,
                "aarch64" => rfpm::Arch::Arm64,
                other => panic!("unsupported architecture: {other}"),
            },
            Arch::X86_64 => rfpm::Arch::Amd64,
            Arch::Aarch64 => rfpm::Arch::Arm64,
        }
    }
}
