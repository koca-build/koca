//! Configuration directories used in Koca.
use std::{
    fs,
    io::{self, ErrorKind},
    path::PathBuf,
    sync::LazyLock,
};

use directories::ProjectDirs;

use crate::error::{CliError, CliResult};

/// The [`ProjectDirs`] instance for Koca.
static PROJECT_DIRS: LazyLock<ProjectDirs> = LazyLock::new(|| {
    ProjectDirs::from("com", "Koca", "Koca").expect("Failed to get project directories")
});

/// Get the cache directory, making sure it also exists.
pub fn cache_dir() -> CliResult<PathBuf> {
    let dir = PROJECT_DIRS.cache_dir().to_owned();
    if let Err(err) = fs::create_dir_all(&dir) {
        return Err(CliError::CacheDir {
            path: dir.clone(),
            err: err,
        });
    }

    Ok(dir)
}

/// Get the binary cache directory, making sure it also exists.
pub fn cache_binary_dir() -> CliResult<PathBuf> {
    let dir = cache_dir()?.join("bin");

    if let Err(err) = fs::create_dir_all(&dir) {
        return Err(CliError::CacheBinDir {
            path: dir.clone(),
            err: err,
        });
    };

    Ok(dir)
}
