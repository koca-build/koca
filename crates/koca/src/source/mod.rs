mod fetch;

pub use fetch::{fetch_source, SourceProgress, SourceProgressState};

use std::path::PathBuf;

use crate::KocaError;

/// A parsed source entry from a build file.
#[derive(Clone, Debug)]
pub struct Source {
    /// Optional rename from `filename::url` syntax.
    pub filename: Option<String>,
    /// The source kind with type-specific data.
    pub kind: SourceKind,
    /// Original raw entry string.
    pub raw: String,
}

/// What kind of source this is.
#[derive(Clone, Debug)]
pub enum SourceKind {
    /// HTTP/HTTPS/FTP download.
    Http { url: String },
    /// Git clone.
    Git {
        url: String,
        reference: Option<GitRef>,
    },
    /// Local file path.
    Local { path: PathBuf },
}

/// A git ref to check out after cloning.
#[derive(Clone, Debug)]
pub enum GitRef {
    Tag(String),
    Branch(String),
    Commit(String),
}

impl Source {
    /// Parse a source entry string.
    ///
    /// Format: `[filename::]scheme://path[#fragment]`
    ///
    /// Examples:
    /// - `https://example.com/foo.tar.gz`
    /// - `custom.tar.gz::https://example.com/foo.tar.gz`
    /// - `git+https://github.com/user/repo#tag=v1.0`
    pub fn parse(entry: &str) -> Result<Source, KocaError> {
        let raw = entry.to_string();

        // Split on first `::` for rename syntax.
        let (filename, rest) = if let Some(idx) = entry.find("::") {
            (Some(entry[..idx].to_string()), &entry[idx + 2..])
        } else {
            (None, entry)
        };

        // Check for git+ prefix.
        if let Some(after_git) = rest.strip_prefix("git+") {
            // Split off #fragment.
            let (url_part, fragment) = if let Some(hash_idx) = after_git.find('#') {
                (&after_git[..hash_idx], Some(&after_git[hash_idx + 1..]))
            } else {
                (after_git, None)
            };

            let reference = fragment.map(parse_git_fragment).transpose()?;

            return Ok(Source {
                filename,
                kind: SourceKind::Git {
                    url: url_part.to_string(),
                    reference,
                },
                raw,
            });
        }

        // Check for URL (contains ://).
        if rest.contains("://") {
            return Ok(Source {
                filename,
                kind: SourceKind::Http {
                    url: rest.to_string(),
                },
                raw,
            });
        }

        // Otherwise it's a local file.
        Ok(Source {
            filename,
            kind: SourceKind::Local {
                path: PathBuf::from(rest),
            },
            raw,
        })
    }

    /// Clean URL for UI display.
    ///
    /// Strips `git+` prefix and `#fragment`, appends ref in parens.
    /// - `git+https://github.com/user/repo#tag=v1.0` → `https://github.com/user/repo (tag=v1.0)`
    /// - `https://example.com/file.tar.gz` → `https://example.com/file.tar.gz`
    /// - `custom.tar.gz::https://example.com/f.tar.gz` → `https://example.com/f.tar.gz`
    pub fn display_url(&self) -> String {
        match &self.kind {
            SourceKind::Http { url } => url.clone(),
            SourceKind::Git { url, reference } => match reference {
                Some(git_ref) => {
                    let ref_str = match git_ref {
                        GitRef::Tag(t) => format!("tag={t}"),
                        GitRef::Branch(b) => format!("branch={b}"),
                        GitRef::Commit(c) => format!("commit={c}"),
                    };
                    format!("{url} ({ref_str})")
                }
                None => url.clone(),
            },
            SourceKind::Local { path } => path.display().to_string(),
        }
    }

    /// Destination filename/dirname in srcdir.
    ///
    /// Uses the explicit filename if set, otherwise the last URL path segment.
    pub fn dest_name(&self) -> String {
        if let Some(name) = &self.filename {
            return name.clone();
        }
        match &self.kind {
            SourceKind::Http { url } => url_last_segment(url),
            SourceKind::Git { url, .. } => {
                let seg = url_last_segment(url);
                // Strip .git suffix if present.
                seg.strip_suffix(".git").unwrap_or(&seg).to_string()
            }
            SourceKind::Local { path } => path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| "source".to_string()),
        }
    }
}

/// Parse a git fragment like `tag=v1.0`, `branch=main`, or `commit=abc123`.
fn parse_git_fragment(fragment: &str) -> Result<GitRef, KocaError> {
    if let Some((key, value)) = fragment.split_once('=') {
        match key {
            "tag" => Ok(GitRef::Tag(value.to_string())),
            "branch" => Ok(GitRef::Branch(value.to_string())),
            "commit" => Ok(GitRef::Commit(value.to_string())),
            _ => Err(KocaError::InvalidSource(format!(
                "unknown git fragment key: {key}"
            ))),
        }
    } else {
        Err(KocaError::InvalidSource(format!(
            "invalid git fragment (expected key=value): {fragment}"
        )))
    }
}

/// Extract the last non-empty path segment from a URL.
fn url_last_segment(url: &str) -> String {
    // Strip query string and fragment.
    let clean = url.split('?').next().unwrap_or(url);
    let clean = clean.split('#').next().unwrap_or(clean);
    clean
        .rsplit('/')
        .find(|s| !s.is_empty())
        .unwrap_or("download")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_http() {
        let s = Source::parse("https://example.com/foo-1.0.tar.gz").unwrap();
        assert!(s.filename.is_none());
        assert!(matches!(s.kind, SourceKind::Http { ref url } if url == "https://example.com/foo-1.0.tar.gz"));
    }

    #[test]
    fn parse_http_rename() {
        let s = Source::parse("custom.tar.gz::https://example.com/foo.tar.gz").unwrap();
        assert_eq!(s.filename.as_deref(), Some("custom.tar.gz"));
        assert!(matches!(s.kind, SourceKind::Http { ref url } if url == "https://example.com/foo.tar.gz"));
    }

    #[test]
    fn parse_git_bare() {
        let s = Source::parse("git+https://github.com/user/repo").unwrap();
        assert!(matches!(s.kind, SourceKind::Git { ref url, ref reference } if url == "https://github.com/user/repo" && reference.is_none()));
    }

    #[test]
    fn parse_git_tag() {
        let s = Source::parse("git+https://github.com/user/repo#tag=v1.0").unwrap();
        assert!(matches!(s.kind, SourceKind::Git { ref reference, .. } if matches!(reference, Some(GitRef::Tag(t)) if t == "v1.0")));
    }

    #[test]
    fn parse_git_branch() {
        let s = Source::parse("git+https://github.com/user/repo#branch=main").unwrap();
        assert!(matches!(s.kind, SourceKind::Git { ref reference, .. } if matches!(reference, Some(GitRef::Branch(b)) if b == "main")));
    }

    #[test]
    fn parse_git_commit() {
        let s = Source::parse("git+https://github.com/user/repo#commit=abc123").unwrap();
        assert!(matches!(s.kind, SourceKind::Git { ref reference, .. } if matches!(reference, Some(GitRef::Commit(c)) if c == "abc123")));
    }

    #[test]
    fn parse_git_rename() {
        let s = Source::parse("myrepo::git+https://github.com/user/repo#tag=v2").unwrap();
        assert_eq!(s.filename.as_deref(), Some("myrepo"));
        assert!(matches!(s.kind, SourceKind::Git { .. }));
    }

    #[test]
    fn parse_git_bad_fragment() {
        assert!(Source::parse("git+https://github.com/user/repo#wat=nope").is_err());
        assert!(Source::parse("git+https://github.com/user/repo#noequalssign").is_err());
    }

    #[test]
    fn parse_local() {
        let s = Source::parse("/path/to/file.tar.gz").unwrap();
        assert!(matches!(s.kind, SourceKind::Local { ref path } if path == &PathBuf::from("/path/to/file.tar.gz")));
    }

    #[test]
    fn parse_local_relative() {
        let s = Source::parse("patches/fix.patch").unwrap();
        assert!(matches!(s.kind, SourceKind::Local { .. }));
    }

    #[test]
    fn display_url_http() {
        assert_eq!(Source::parse("https://example.com/f.tar.gz").unwrap().display_url(), "https://example.com/f.tar.gz");
    }

    #[test]
    fn display_url_http_rename() {
        assert_eq!(Source::parse("x::https://example.com/f.tar.gz").unwrap().display_url(), "https://example.com/f.tar.gz");
    }

    #[test]
    fn display_url_git_tag() {
        assert_eq!(Source::parse("git+https://github.com/u/r#tag=v1.0").unwrap().display_url(), "https://github.com/u/r (tag=v1.0)");
    }

    #[test]
    fn display_url_git_bare() {
        assert_eq!(Source::parse("git+https://github.com/u/r").unwrap().display_url(), "https://github.com/u/r");
    }

    #[test]
    fn dest_name_http() {
        assert_eq!(Source::parse("https://example.com/foo-1.0.tar.gz").unwrap().dest_name(), "foo-1.0.tar.gz");
    }

    #[test]
    fn dest_name_rename() {
        assert_eq!(Source::parse("custom.tar.gz::https://example.com/foo.tar.gz").unwrap().dest_name(), "custom.tar.gz");
    }

    #[test]
    fn dest_name_git_strips_dotgit() {
        assert_eq!(Source::parse("git+https://github.com/u/repo.git#tag=v1").unwrap().dest_name(), "repo");
    }

    #[test]
    fn dest_name_git_no_dotgit() {
        assert_eq!(Source::parse("git+https://github.com/u/repo#tag=v1").unwrap().dest_name(), "repo");
    }

    #[test]
    fn dest_name_local() {
        assert_eq!(Source::parse("/path/to/file.patch").unwrap().dest_name(), "file.patch");
    }
}
