use std::cmp::Ordering;

use libversion::version_compare2;

use crate::{KocaError, KocaResult};

/// A dependency constraint parsed from a `.koca` file's `depends` or
/// `makedepends` array.
///
/// The `name` is a **repology project name** (e.g. `"openssl"`, `"cmake"`).
/// Version operators map directly to what pacman/dpkg support.
///
/// # Examples
/// ```text
/// openssl>=3.0   →  DepConstraint { name: "openssl", op: Some(Ge), version: Some("3.0") }
/// curl           →  DepConstraint { name: "curl",    op: None,     version: None         }
/// gcc=14.1.0     →  DepConstraint { name: "gcc",     op: Some(Eq), version: Some("14.1.0") }
/// python<4       →  DepConstraint { name: "python",  op: Some(Lt), version: Some("4")    }
/// ```
#[derive(Debug, Clone)]
pub struct DepConstraint {
    /// Repology project name.
    pub name: String,
    pub op: Option<DepOp>,
    pub version: Option<String>,
}

/// Version comparison operator in a dependency constraint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DepOp {
    Ge, // >=
    Le, // <=
    Gt, // >
    Lt, // <
    Eq, // =
}

impl DepOp {
    fn as_str(self) -> &'static str {
        match self {
            DepOp::Ge => ">=",
            DepOp::Le => "<=",
            DepOp::Gt => ">",
            DepOp::Lt => "<",
            DepOp::Eq => "=",
        }
    }
}

impl std::fmt::Display for DepConstraint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.name)?;
        if let (Some(op), Some(ver)) = (&self.op, &self.version) {
            write!(f, "{}{}", op.as_str(), ver)?;
        }
        Ok(())
    }
}

impl DepConstraint {
    /// Parse a dependency string like `"openssl>=3.0"` or `"curl"`.
    ///
    /// Operators are tried longest-first (`>=` before `>`) to avoid
    /// mismatching `>=` as `>`.
    pub fn parse(s: &str) -> KocaResult<Self> {
        // Try operators longest-first to avoid ">=" being parsed as ">"
        const OPS: &[(&str, DepOp)] = &[
            (">=", DepOp::Ge),
            ("<=", DepOp::Le),
            (">", DepOp::Gt),
            ("<", DepOp::Lt),
            ("=", DepOp::Eq),
        ];

        for (op_str, op) in OPS {
            if let Some((name, ver)) = s.split_once(op_str) {
                let name = name.trim();
                let ver = ver.trim();
                if name.is_empty() {
                    return Err(KocaError::InvalidDep(s.to_string()));
                }
                return Ok(Self {
                    name: name.to_string(),
                    op: Some(*op),
                    version: Some(ver.to_string()),
                });
            }
        }

        // No operator found — bare package name
        let name = s.trim();
        if name.is_empty() {
            return Err(KocaError::InvalidDep(s.to_string()));
        }
        Ok(Self {
            name: name.to_string(),
            op: None,
            version: None,
        })
    }

    /// Check whether `installed_version` satisfies this constraint.
    ///
    /// Uses `libversion` (repology's version comparison algorithm) for
    /// cross-distro-compatible comparisons.
    ///
    /// Returns `true` when there is no version constraint (bare package name).
    pub fn satisfied_by(&self, installed_version: &str) -> bool {
        let (Some(op), Some(req_ver)) = (&self.op, &self.version) else {
            return true;
        };

        let ord = version_compare2(installed_version, req_ver);
        match op {
            DepOp::Ge => ord != Ordering::Less,
            DepOp::Le => ord != Ordering::Greater,
            DepOp::Gt => ord == Ordering::Greater,
            DepOp::Lt => ord == Ordering::Less,
            DepOp::Eq => ord == Ordering::Equal,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_bare() {
        let d = DepConstraint::parse("curl").unwrap();
        assert_eq!(d.name, "curl");
        assert!(d.op.is_none());
        assert!(d.version.is_none());
    }

    #[test]
    fn parse_ge() {
        let d = DepConstraint::parse("openssl>=3.0").unwrap();
        assert_eq!(d.name, "openssl");
        assert_eq!(d.op, Some(DepOp::Ge));
        assert_eq!(d.version.as_deref(), Some("3.0"));
    }

    #[test]
    fn parse_ge_not_gt() {
        // Make sure ">=" isn't parsed as ">" with "=3.0" as version
        let d = DepConstraint::parse("openssl>=3.0").unwrap();
        assert_eq!(d.op, Some(DepOp::Ge));
        assert_eq!(d.version.as_deref(), Some("3.0"));
    }

    #[test]
    fn parse_eq() {
        let d = DepConstraint::parse("gcc=14.1.0").unwrap();
        assert_eq!(d.op, Some(DepOp::Eq));
        assert_eq!(d.version.as_deref(), Some("14.1.0"));
    }

    #[test]
    fn parse_lt() {
        let d = DepConstraint::parse("python<4").unwrap();
        assert_eq!(d.op, Some(DepOp::Lt));
        assert_eq!(d.version.as_deref(), Some("4"));
    }

    #[test]
    fn satisfied_bare() {
        let d = DepConstraint::parse("curl").unwrap();
        assert!(d.satisfied_by("7.88.1"));
        assert!(d.satisfied_by("anything"));
    }

    #[test]
    fn satisfied_ge() {
        let d = DepConstraint::parse("openssl>=3.0").unwrap();
        assert!(d.satisfied_by("3.0"));
        assert!(d.satisfied_by("3.4.1"));
        assert!(!d.satisfied_by("2.9.9"));
    }

    #[test]
    fn satisfied_eq() {
        let d = DepConstraint::parse("gcc=14.1.0").unwrap();
        assert!(d.satisfied_by("14.1.0"));
        assert!(!d.satisfied_by("14.1.1"));
        assert!(!d.satisfied_by("13.2.0"));
    }
}
