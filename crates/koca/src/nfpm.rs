//! Utilities to interact with [`nfpm`].
use std::path::{self, Path};

use serde::Serialize;
use walkdir::WalkDir;

/// `nfpm` file info
#[derive(Serialize)]
pub struct NfpmFileInfo {
    /// The file's mode.
    mode: u32,
}

/// `nfpm` package file mappings.
#[derive(Serialize)]
pub struct NfpmFile {
    /// The file's source.
    src: String,
    /// The file's destination.
    dst: String,
    /// The file's information.
    file_info: NfpmFileInfo,
}

/// An `nfpm` config.
///
/// This intentionally doesn't do much type checking. The caller is expected to hold up most guarantees the nfpm config requires.
#[derive(Serialize)]
pub struct NfpmConfig {
    /// The package's name.
    pub name: String,
    /// The package's architecture.
    pub arch: String,
    /// The package's platform.
    pub platform: String,
    /// The package's `epoch`.
    pub epoch: Option<u32>,
    /// The package's `pkgver`.
    pub version: String,
    /// The package's `pkgrel`.
    pub release: Option<u32>,
    /// The package's maintainer.
    pub maintainer: String,
    /// The package's description.
    pub description: String,
    /// The package's license.
    pub license: String,
    /// The package's contents.
    pub contents: Vec<NfpmFile>,
}

/// Get a list of [`NfpmFile`] from the given package directory.
pub fn get_nfpm_files(pkgdir: &Path) -> Vec<NfpmFile> {
    let mut files = vec![];

    for res_entry in WalkDir::new(pkgdir) {
        let entry = res_entry.expect("pkgdir files should always be accessible");

        if entry.file_type().is_dir() {
            continue;
        }

        let src_path =
            path::absolute(entry.path()).expect("getting absolute path should always succeed");
        let dst_path = entry
            .path()
            .strip_prefix(pkgdir)
            .expect("pkgdir strip should always succeed");

        files.push(NfpmFile {
            src: src_path.display().to_string(),
            dst: dst_path.display().to_string(),
            file_info: NfpmFileInfo { mode: 0o755 },
        });
    }

    files
}
