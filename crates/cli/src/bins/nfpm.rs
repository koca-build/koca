//! Utilities to interact with [`nfpm`](https://github.com/goreleaser/nfpm).
use crate::{dirs, error::CliResult, http, CliError};
use flate2::read::GzDecoder;
use koca::{PkgVersion, Version};
use regex::Regex;
use std::{
    cell::LazyCell, fs, ops::Deref, path::PathBuf, process::Command, str::FromStr, sync::LazyLock,
};
use zolt::Colorize;

/// `nfpm` string as a constant.
pub const BIN_NAME: &str = "nfpm";

/// The version of `nfpm` that should be downloaded if the system version is unavailable.
pub const VERSION: &str = "2.43.0";

/// the URL to download the `nfpm` binary from.
pub const DOWNLOAD_URL: LazyCell<String> = LazyCell::new(|| {
    format!("https://github.com/goreleaser/nfpm/releases/download/v{VERSION}/nfpm_{VERSION}_Linux_x86_64.tar.gz")
});

/// The regex to get the version out of `nfpm -v` output.
const VERSION_REGEX: LazyLock<Regex> = LazyLock::new(|| Regex::new("^GitVersion: *(.*)$").unwrap());

/// Download the `nfpm` binary if it doesn't already exist, and return the data of the downloaded binary.
pub async fn download() -> CliResult<Vec<u8>> {
    let bin_err = |err| CliError::NetworkBinary {
        bin_name: BIN_NAME.into(),
        err,
    };

    let nfpm_bin: Vec<u8> = http::CLIENT
        .get(DOWNLOAD_URL.deref())
        .send()
        .await
        .map_err(bin_err)?
        .bytes()
        .await
        .map_err(bin_err)?
        .into();

    Ok(nfpm_bin)
}

/// Check if `nfpm` is either not present, or not at the required version.
pub fn needs_install() -> bool {
    // If unable to get the cache dir, get the binary.
    let bin_path = match dirs::cache_binary_dir() {
        Ok(path) => path.join(BIN_NAME),
        Err(err) => return true,
    };

    // If unable to query nfpm for its version, get the binary.
    let version_output = match Command::new(&bin_path).arg("-v").output() {
        Ok(output) => output,
        Err(_) => return true,
    };

    if !version_output.status.success() {
        return false;
    }

    // Parse out the version from nfpm.
    let stdout = String::from_utf8(version_output.stdout).expect("output should be valid");
    let version_line = stdout
        .lines()
        .find(|line| VERSION_REGEX.is_match(line))
        .unwrap();
    let version_str = VERSION_REGEX
        .captures(version_line)
        .expect("should have captured version")
        .get(1)
        .expect("should have found version")
        .as_str();

    // If the installed version doesn't have the same major version we require, install.
    let requested_version = PkgVersion::from_str(VERSION).expect("version should be valid");
    let installed_version = PkgVersion::from_str(version_str).expect("version should be valid");

    requested_version.major != installed_version.major
}

/// Place the given binary data into the nfpm install location, returning the binary's path.
pub fn install(data: &[u8]) -> CliResult<PathBuf> {
    let bin_path = dirs::cache_binary_dir()?.join(BIN_NAME);

    // Unpack the gzip archive.
    let gz_decoder = GzDecoder::new(data);
    let mut tar_decoder = tar::Archive::new(gz_decoder);

    for res_entry in tar_decoder
        .entries()
        .expect("archive files should be valid")
    {
        let mut entry = res_entry.expect("archive entry should be valid");
        let file_name = entry
            .path()
            .expect("file name should be valid")
            .to_string_lossy()
            .to_string();

        if file_name == BIN_NAME {
            if let Err(err) = entry.unpack(&bin_path) {
                return Err(CliError::InstallBinary {
                    bin_name: BIN_NAME.into(),
                    err,
                });
            }

            return Ok(bin_path);
        }
    }

    unreachable!("{BIN_NAME} binary should have been found")
}
