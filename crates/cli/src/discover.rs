use std::path::PathBuf;

use crate::error::CliMultiError;

/// Find a single `.koca` build file in the current directory, falling back to
/// a `koca/` subdirectory.
pub fn find_build_file() -> Result<PathBuf, CliMultiError> {
    if let Some(path) = find_single_koca_in(".")? {
        return Ok(path);
    }

    if let Some(path) = find_single_koca_in("koca")? {
        return Ok(path);
    }

    Err(std::io::Error::other(
        "no .koca file found in current directory or koca/ subdirectory",
    ))?
}

/// Return the single `.koca` file in `dir`, or `None` if the directory doesn't
/// exist or contains no `.koca` files. Errors if multiple `.koca` files are found.
fn find_single_koca_in(dir: &str) -> Result<Option<PathBuf>, CliMultiError> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e)?,
    };

    let mut found: Option<PathBuf> = None;

    for entry in entries {
        let entry = entry?;
        let path = entry.path();

        if path.extension().is_some_and(|ext| ext == "koca") && path.is_file() {
            if found.is_some() {
                Err(std::io::Error::other(format!(
                    "multiple .koca files found in {dir}/ — specify one explicitly",
                )))?;
            }
            found = Some(path);
        }
    }

    Ok(found)
}
