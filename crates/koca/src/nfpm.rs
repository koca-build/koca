//! Utilities to interact with [`nfpm`].
use std::os::unix::fs::MetadataExt;
use std::path::{self, Path};

use nix::unistd::{Gid, Group, Uid, User};
use serde::Serialize;
use walkdir::WalkDir;

/// File ownership and permissions to embed in an [`NfpmFile`].
#[derive(Serialize)]
pub struct NfpmFileInfo {
    pub owner: String,
    pub group: String,
    pub mode: u32,
}

/// `nfpm` package file mappings.
#[derive(Serialize)]
pub struct NfpmFile {
    /// The file's source.
    src: String,
    /// The file's destination.
    dst: String,
    /// The file's ownership/permissions, resolved via libc (fakeroot-aware).
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
    /// The package's runtime dependencies.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub depends: Vec<String>,
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

        // Stat the file here in Rust (via libc) rather than in Go (via raw syscall).
        // fakeroot intercepts libc calls, so `metadata()` returns the fakeroot-tracked
        // uid/gid set by `install -o`/`chown` in package(). Go bypasses libc entirely
        // with raw syscalls, so nfpm's own os.Stat() would see the real kernel owner
        // and ignore whatever fakeroot tracked.
        //
        // Use entry.metadata() — WalkDir already called stat() during traversal, so
        // this reuses the cached result instead of issuing a second syscall per file.
        let metadata = entry
            .metadata()
            .expect("file metadata should always be readable");
        let uid = metadata.uid();
        let gid = metadata.gid();
        let mode = metadata.mode() & 0o7777;

        let owner = User::from_uid(Uid::from_raw(uid))
            .ok()
            .flatten()
            .map(|u| u.name)
            .unwrap_or_else(|| uid.to_string());

        let group = Group::from_gid(Gid::from_raw(gid))
            .ok()
            .flatten()
            .map(|g| g.name)
            .unwrap_or_else(|| gid.to_string());

        files.push(NfpmFile {
            src: src_path.display().to_string(),
            dst: dst_path.display().to_string(),
            file_info: NfpmFileInfo { owner, group, mode },
        });
    }

    files
}
