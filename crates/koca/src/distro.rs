use std::{fs, str::FromStr};

use crate::{KocaError, KocaResult};

/// The detected (or user-specified) target distribution.
#[derive(Debug, Clone)]
pub struct Distro {
    pub id: String,
    pub version_id: Option<String>,
}

impl Distro {
    /// Detect the current distro by reading `/etc/os-release`.
    pub fn detect() -> KocaResult<Self> {
        let content = fs::read_to_string("/etc/os-release").map_err(KocaError::IO)?;
        Self::parse_os_release(&content)
    }

    fn parse_os_release(content: &str) -> KocaResult<Self> {
        let mut id: Option<String> = None;
        let mut version_id: Option<String> = None;

        for line in content.lines() {
            let line = line.trim();
            if let Some(val) = line.strip_prefix("ID=") {
                id = Some(unquote(val).to_string());
            } else if let Some(val) = line.strip_prefix("VERSION_ID=") {
                version_id = Some(unquote(val).to_string());
            }
        }

        let id = id.ok_or_else(|| {
            KocaError::IO(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "ID not found in /etc/os-release",
            ))
        })?;

        Ok(Self { id, version_id })
    }

    /// Returns the repology repository name for this distro.
    ///
    /// Examples: `"arch"`, `"debian_12"`, `"ubuntu_24_04"`
    pub fn repology_repo(&self) -> String {
        match self.id.as_str() {
            "arch" | "manjaro" | "endeavouros" | "garuda" => "arch".into(),
            "debian" => {
                let ver = self.version_id.as_deref().unwrap_or("").replace('.', "_");
                format!("debian_{ver}")
            }
            "ubuntu" => {
                let ver = self.version_id.as_deref().unwrap_or("").replace('.', "_");
                format!("ubuntu_{ver}")
            }
            "fedora" => {
                let ver = self.version_id.as_deref().unwrap_or("");
                format!("fedora_{ver}")
            }
            other => other.to_string(),
        }
    }

    /// Returns the backend binary name to use for this distro.
    pub fn backend_binary(&self) -> &str {
        match self.id.as_str() {
            "arch" | "manjaro" | "endeavouros" | "garuda" => "koca-backend-alpm",
            "debian" | "ubuntu" | "linuxmint" | "pop" => "koca-backend-apt",
            _ => "koca-backend-alpm",
        }
    }
}

impl FromStr for Distro {
    type Err = KocaError;

    /// Parse a `--target` override string like `"arch"` or `"debian:12"`.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if let Some((id, ver)) = s.split_once(':') {
            Ok(Self {
                id: id.to_string(),
                version_id: Some(ver.to_string()),
            })
        } else {
            Ok(Self {
                id: s.to_string(),
                version_id: None,
            })
        }
    }
}

fn unquote(s: &str) -> &str {
    let s = s.trim();
    if (s.starts_with('"') && s.ends_with('"')) || (s.starts_with('\'') && s.ends_with('\'')) {
        &s[1..s.len() - 1]
    } else {
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_arch() {
        let d = Distro::parse_os_release("ID=arch\n").unwrap();
        assert_eq!(d.id, "arch");
        assert_eq!(d.repology_repo(), "arch");
        assert_eq!(d.backend_binary(), "koca-backend-alpm");
    }

    #[test]
    fn parse_debian_quoted() {
        let d = Distro::parse_os_release("ID=debian\nVERSION_ID=\"12\"\n").unwrap();
        assert_eq!(d.repology_repo(), "debian_12");
        assert_eq!(d.backend_binary(), "koca-backend-apt");
    }

    #[test]
    fn parse_ubuntu() {
        let d = Distro::parse_os_release("ID=ubuntu\nVERSION_ID=\"24.04\"\n").unwrap();
        assert_eq!(d.repology_repo(), "ubuntu_24_04");
    }

    #[test]
    fn from_str_with_version() {
        let d: Distro = "debian:12".parse().unwrap();
        assert_eq!(d.id, "debian");
        assert_eq!(d.version_id.as_deref(), Some("12"));
    }
}
