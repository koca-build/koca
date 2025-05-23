use crate::KocaResult;
use brush::{CreateOptions, Shell};
use std::{fs, path::Path};

/// A package's architecture.
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

/// A package's version.
pub struct Version {
    /// The version's package version segment (`1.0.0` in `1.0.0-2`).
    pub pkgver: String,
    /// The version's package releationship segment (`2` in `1.0.0-2`).
    pub pkgrel: u32,
    /// The version's epoch segment (`3` in `1.0.0-3`).
    pub epoch: Option<u32>,
}

/// A package's Koca build file.
pub struct BuildFile {
    /// The package's name.
    pub pkgname: String,
    /// The package's version.
    pub version: Version,
    /// The package's architecture.
    pub arch: Arch,
}

impl BuildFile {
    /// Get the [`CreateOptions`].
    fn create_options() -> CreateOptions {
        CreateOptions {
            no_profile: true,
            no_rc: true,
            do_not_inherit_env: true,
            ..Default::default()
        }
    }

    /// Read a Koca build script from the input bytes.
    ///
    /// Returns a [`KocaError::Parser`] error if the input is an invalid script.
    pub async fn from_bytes<B: Into<Vec<u8>>>(bytes: B) -> KocaResult<Self> {
        let create_options = Self::create_options();
        let shell = Shell::new(&create_options).await.unwrap();
        shell.parse_bytes(bytes)?;
        todo!("Fully process the build file boi");
    }

    /// Read a Koca build script from the input string.
    ///
    /// Returns a [`KocaError::Parser`] error if the input is an invalid script.
    pub async fn from_str<S: Into<String>>(string: S) -> KocaResult<Self> {
        Self::from_bytes(string.into()).await
    }

    /// Read a Koca build script from the input file.
    ///
    /// Returns a:
    /// - [`KocaError::Parser`] error if the input is an invalid script.
    /// - [`KocaError::IO`] error if the input file can't be read.
    pub async fn from_file<P: AsRef<Path>>(path: P) -> KocaResult<Self> {
        let file_bytes = fs::read(path)?;
        Self::from_bytes(file_bytes).await
    }
}
