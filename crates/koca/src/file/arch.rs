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

    /// Pick the architecture to build for on a `host` (a
    /// [`std::env::consts::ARCH`] value) from a build file's `arch=()`.
    ///
    /// `all` wins, then `any`, then the entry matching the host. Errors if
    /// none apply.
    pub(crate) fn select_for_host(archs: &[Arch], host: &str) -> KocaResult<Arch> {
        if archs.contains(&Arch::All) {
            return Ok(Arch::All);
        }
        if archs.contains(&Arch::Any) {
            return Ok(Arch::Any);
        }
        if let Ok(host_arch) = Arch::from_str(host) {
            if archs.contains(&host_arch) {
                return Ok(host_arch);
            }
        }
        Err(KocaError::UnsupportedHostArch {
            host: host.to_string(),
            supported: archs
                .iter()
                .map(Arch::get_string)
                .collect::<Vec<_>>()
                .join(" "),
        })
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

#[cfg(test)]
mod tests {
    use super::Arch;
    use crate::KocaError;

    #[test]
    fn selects_matching_host_arch() {
        let archs = [Arch::X64, Arch::Arm64];
        assert_eq!(Arch::select_for_host(&archs, "x86_64").unwrap(), Arch::X64);
        assert_eq!(
            Arch::select_for_host(&archs, "aarch64").unwrap(),
            Arch::Arm64
        );
    }

    #[test]
    fn prefers_all_then_any() {
        assert_eq!(
            Arch::select_for_host(&[Arch::X64, Arch::All], "aarch64").unwrap(),
            Arch::All
        );
        assert_eq!(
            Arch::select_for_host(&[Arch::Arm64, Arch::Any, Arch::All], "aarch64").unwrap(),
            Arch::All
        );
        assert_eq!(
            Arch::select_for_host(&[Arch::X64, Arch::Any], "x86_64").unwrap(),
            Arch::Any
        );
    }

    #[test]
    fn rejects_unsupported_host_arch() {
        let err = Arch::select_for_host(&[Arch::X64], "aarch64").unwrap_err();
        assert!(matches!(err, KocaError::UnsupportedHostArch { .. }));
        assert_eq!(
            err.to_string(),
            "build file does not support host architecture aarch64; arch=(x64)"
        );

        let err = Arch::select_for_host(&[Arch::X64, Arch::Arm64], "riscv64").unwrap_err();
        assert_eq!(
            err.to_string(),
            "build file does not support host architecture riscv64; arch=(x64 arm64)"
        );
    }
}
