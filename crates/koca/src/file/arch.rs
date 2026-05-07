use std::str::FromStr;

use crate::{KocaError, KocaParserError, KocaResult};

/// A package's architecture.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum Arch {
    /// Architecture-agnostic source and binary (e.g. a Python script).
    All,
    /// Architecture-agnostic source, architecture-specific binary (e.g. compiled C).
    Any,
    /// 64-bit x86. Accepts `x64`, `x86_64`, `amd64`.
    X64,
    /// 64-bit ARM. Accepts `arm64`, `aarch64`.
    Arm64,
}

impl FromStr for Arch {
    type Err = KocaError;

    fn from_str(value: &str) -> KocaResult<Self> {
        match value {
            "all" => Ok(Arch::All),
            "any" => Ok(Arch::Any),
            "x64" | "x86_64" | "amd64" => Ok(Arch::X64),
            "arm64" | "aarch64" => Ok(Arch::Arm64),
            _ => Err(KocaParserError::InvalidArch(value.to_string()).into()),
        }
    }
}

impl Arch {
    /// Canonical display string.
    pub fn get_string(&self) -> &'static str {
        match self {
            Arch::All => "all",
            Arch::Any => "any",
            Arch::X64 => "x64",
            Arch::Arm64 => "arm64",
        }
    }

    /// Debian-style architecture string.
    pub fn get_deb_string(&self) -> &'static str {
        match self {
            Arch::All => "all",
            Arch::Any => match std::env::consts::ARCH {
                "x86_64" => "amd64",
                "aarch64" => "arm64",
                other => panic!("unsupported architecture: {other}"),
            },
            Arch::X64 => "amd64",
            Arch::Arm64 => "arm64",
        }
    }

    /// RPM-style architecture string.
    pub fn get_rpm_string(&self) -> &'static str {
        match self {
            Arch::All => "noarch",
            Arch::Any => match std::env::consts::ARCH {
                "x86_64" => "x86_64",
                "aarch64" => "aarch64",
                other => panic!("unsupported architecture: {other}"),
            },
            Arch::X64 => "x86_64",
            Arch::Arm64 => "aarch64",
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
            Arch::X64 => rfpm::Arch::Amd64,
            Arch::Arm64 => rfpm::Arch::Arm64,
        }
    }

    /// All string variants that parse to this arch. Useful for matching `source_SUFFIX` variables.
    pub fn suffixes(&self) -> &[&str] {
        match self {
            Arch::All => &["all"],
            Arch::Any => &["any"],
            Arch::X64 => &["x64", "x86_64", "amd64"],
            Arch::Arm64 => &["arm64", "aarch64"],
        }
    }
}
