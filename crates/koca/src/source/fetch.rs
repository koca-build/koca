use std::path::Path;
use std::sync::{Arc, Mutex};

use super::{GitRef, Source, SourceKind};

/// Per-item progress state, updated by the fetcher, read by the UI.
pub struct SourceProgress {
    /// Progress bar fraction (0.0..1.0), None if total unknown.
    pub fraction: Option<f64>,
    /// Bytes transferred.
    pub bytes: u64,
    /// Total bytes expected (from Content-Length), None if unknown.
    pub total_bytes: Option<u64>,
    /// Human-readable detail (e.g. "15.2 MB/38.1 MB" or "1204/3500 objects (2.1 MB)").
    pub detail: String,
    /// Whether the fetch is complete.
    pub done: bool,
    /// Error message if the fetch failed.
    pub error: Option<String>,
}

impl Default for SourceProgress {
    fn default() -> Self {
        Self::new()
    }
}

impl SourceProgress {
    pub fn new() -> Self {
        Self {
            fraction: None,
            bytes: 0,
            total_bytes: None,
            detail: String::new(),
            done: false,
            error: None,
        }
    }
}

/// Shared state for all source fetches.
pub type SourceProgressState = Arc<Mutex<Vec<SourceProgress>>>;

pub fn format_bytes(b: u64) -> String {
    if b >= 1_000_000_000 {
        format!("{:.1} GB", b as f64 / 1_000_000_000.0)
    } else if b >= 1_000_000 {
        format!("{:.1} MB", b as f64 / 1_000_000.0)
    } else if b >= 1_000 {
        format!("{:.0} KB", b as f64 / 1_000.0)
    } else {
        format!("{} B", b)
    }
}

/// Fetch a single source to `dest_dir`, updating `progress[index]`.
pub async fn fetch_source(
    source: &Source,
    dest_dir: &Path,
    index: usize,
    progress: &SourceProgressState,
) -> Result<(), String> {
    match &source.kind {
        SourceKind::Http { url } => fetch_http(url, source, dest_dir, index, progress).await,
        SourceKind::Git { url, reference } => {
            fetch_git(url, reference.as_ref(), source, dest_dir, index, progress).await
        }
        SourceKind::Local { path } => fetch_local(path, source, dest_dir, index, progress).await,
    }
}

fn mark_cached(dest: &Path, index: usize, progress: &SourceProgressState) -> Result<(), String> {
    let size = std::fs::metadata(dest).map(|m| m.len()).unwrap_or(0);
    let mut s = progress.lock().unwrap();
    s[index].bytes = size;
    s[index].total_bytes = Some(size);
    s[index].fraction = Some(1.0);
    s[index].detail = format!("{} (cached)", format_bytes(size));
    s[index].done = true;
    Ok(())
}

async fn fetch_http(
    url: &str,
    source: &Source,
    dest_dir: &Path,
    index: usize,
    progress: &SourceProgressState,
) -> Result<(), String> {
    let dest = dest_dir.join(source.dest_name());
    if dest.exists() {
        return mark_cached(&dest, index, progress);
    }

    let client = reqwest::Client::builder()
        .user_agent("koca")
        .build()
        .map_err(|e| e.to_string())?;

    let mut response = client.get(url).send().await.map_err(|e| e.to_string())?;

    if !response.status().is_success() {
        let msg = format!("HTTP {}", response.status());
        let mut s = progress.lock().unwrap();
        s[index].error = Some(msg.clone());
        s[index].done = true;
        return Err(msg);
    }

    let total = response.content_length().filter(|&l| l > 0);
    if let Some(t) = total {
        let mut s = progress.lock().unwrap();
        s[index].total_bytes = Some(t);
        s[index].detail = format!("0 B/{}", format_bytes(t));
    }

    let mut file = tokio::fs::File::create(&dest)
        .await
        .map_err(|e| e.to_string())?;

    let mut downloaded: u64 = 0;
    while let Some(chunk) = response.chunk().await.map_err(|e| e.to_string())? {
        tokio::io::AsyncWriteExt::write_all(&mut file, &chunk)
            .await
            .map_err(|e| e.to_string())?;
        downloaded += chunk.len() as u64;

        let (fraction, detail) = if let Some(t) = total {
            (
                Some((downloaded as f64 / t as f64).min(1.0)),
                format!("{}/{}", format_bytes(downloaded), format_bytes(t)),
            )
        } else {
            (None, format_bytes(downloaded))
        };

        let mut s = progress.lock().unwrap();
        s[index].bytes = downloaded;
        s[index].fraction = fraction;
        s[index].detail = detail;
    }

    let mut s = progress.lock().unwrap();
    s[index].done = true;
    Ok(())
}

async fn fetch_git(
    url: &str,
    reference: Option<&GitRef>,
    source: &Source,
    dest_dir: &Path,
    index: usize,
    progress: &SourceProgressState,
) -> Result<(), String> {
    let dest = dest_dir.join(source.dest_name());
    if dest.exists() {
        return mark_cached(&dest, index, progress);
    }
    let url = url.to_string();
    let reference = reference.cloned();
    let progress = Arc::clone(progress);

    tokio::task::spawn_blocking(move || {
        let mut cb = git2::RemoteCallbacks::new();
        let progress_clone = Arc::clone(&progress);

        cb.transfer_progress(move |stats| {
            let received = stats.received_objects() as u64;
            let total = stats.total_objects() as u64;
            let bytes = stats.received_bytes() as u64;

            let mut s = progress_clone.lock().unwrap();
            s[index].bytes = bytes;
            if total > 0 {
                s[index].fraction = Some(received as f64 / total as f64);
                s[index].detail =
                    format!("{}/{} objects ({})", received, total, format_bytes(bytes));
            }
            true
        });

        let mut fo = git2::FetchOptions::new();
        fo.remote_callbacks(cb);
        fo.depth(1);

        let mut builder = git2::build::RepoBuilder::new();
        builder.fetch_options(fo);

        if let Some(ref git_ref) = reference {
            match git_ref {
                GitRef::Branch(b) => {
                    builder.branch(b);
                }
                GitRef::Tag(t) => {
                    builder.branch(t);
                }
                GitRef::Commit(_) => {
                    // Clone first, checkout commit after.
                }
            }
        }

        let repo = builder
            .clone(&url, &dest)
            .map_err(|e| e.message().to_string())?;

        // For commit refs, checkout after clone.
        if let Some(GitRef::Commit(ref hash)) = reference {
            let oid = git2::Oid::from_str(hash).map_err(|e| e.message().to_string())?;
            let commit = repo.find_commit(oid).map_err(|e| e.message().to_string())?;
            repo.checkout_tree(commit.as_object(), None)
                .map_err(|e| e.message().to_string())?;
            repo.set_head_detached(oid)
                .map_err(|e| e.message().to_string())?;
        }

        progress.lock().unwrap()[index].done = true;
        Ok(())
    })
    .await
    .map_err(|e| e.to_string())?
}

async fn fetch_local(
    path: &Path,
    source: &Source,
    dest_dir: &Path,
    index: usize,
    progress: &SourceProgressState,
) -> Result<(), String> {
    let dest = dest_dir.join(source.dest_name());
    tokio::fs::copy(path, &dest)
        .await
        .map_err(|e| e.to_string())?;

    let meta = tokio::fs::metadata(&dest)
        .await
        .map_err(|e| e.to_string())?;
    let size = meta.len();

    let mut s = progress.lock().unwrap();
    s[index].bytes = size;
    s[index].fraction = Some(1.0);
    s[index].detail = format_bytes(size);
    s[index].done = true;
    Ok(())
}
