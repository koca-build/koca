use repology::RepologyClient;

use crate::{dep::DepConstraint, KocaError, KocaResult};

/// A dependency constraint resolved to one or more native package names on a
/// specific distro.
#[derive(Debug, Clone)]
pub struct ResolvedDep {
    /// The original constraint from the `.koca` file.
    pub constraint: DepConstraint,
    /// Native package name(s) for the target distro.
    /// Most projects map to a single name; some (e.g. split packages) map to
    /// multiple.
    pub native_names: Vec<String>,
}

impl ResolvedDep {
    /// The display string for the constraint, e.g. `"openssl>=3.0"`.
    pub fn display_constraint(&self) -> String {
        self.constraint.to_string()
    }
}

/// Resolve a list of repology project constraints to native package names for
/// the target distro repository.
///
/// Hard-fails if repology is unreachable or a project has no package for the
/// target repo.
pub async fn resolve_deps(deps: &[DepConstraint], repo: &str) -> KocaResult<Vec<ResolvedDep>> {
    if deps.is_empty() {
        return Ok(vec![]);
    }

    let client = RepologyClient::builder()
        .user_agent(concat!("koca/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| {
            KocaError::IO(std::io::Error::other(format!(
                "failed to build repology client: {e}"
            )))
        })?;

    let mut resolved = Vec::with_capacity(deps.len());

    for dep in deps {
        let packages = client.project(&dep.name).await.map_err(|e| {
            KocaError::IO(std::io::Error::other(format!(
                "repology lookup failed for '{}': {e}\n\
                 Ensure you have network access to repology.org.",
                dep.name
            )))
        })?;

        let matching: Vec<_> = packages.iter().filter(|p| p.repo == repo).collect();

        if matching.is_empty() {
            return Err(KocaError::IO(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!(
                    "repology project '{}' has no package for distro repo '{repo}'.\n\
                     Check the project name at https://repology.org/project/{}/versions",
                    dep.name, dep.name
                ),
            )));
        }

        // Collect all binary names. Some projects map to multiple packages
        // (e.g. split packages). We install all of them.
        let mut native_names: Vec<String> = Vec::new();
        for pkg in &matching {
            if let Some(names) = &pkg.binnames {
                native_names.extend(names.iter().cloned());
            } else if let Some(name) = &pkg.binname {
                native_names.push(name.clone());
            } else if let Some(name) = &pkg.srcname {
                native_names.push(name.clone());
            } else {
                // Fall back to the repology project name
                native_names.push(dep.name.clone());
            }
        }
        native_names.sort();
        native_names.dedup();

        resolved.push(ResolvedDep {
            constraint: dep.clone(),
            native_names,
        });
    }

    Ok(resolved)
}

/// Flatten a list of resolved deps into a plain list of native package names.
pub fn native_names(resolved: &[ResolvedDep]) -> Vec<String> {
    resolved
        .iter()
        .flat_map(|r| r.native_names.iter().cloned())
        .collect()
}
