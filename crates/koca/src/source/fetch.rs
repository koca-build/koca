use std::path::Path;

use super::{GitRef, Source, SourceKind};
use crate::handler::SourceHandler;
use tempfile::NamedTempFile;
use tokio::sync::mpsc;

/// Live progress for a single source fetch.
pub enum SourceProgress {
    /// HTTP/FTP/local download, by bytes.
    Download {
        bytes: u64,
        total_bytes: Option<u64>,
    },
    /// Git clone/fetch, by object count (plus bytes received).
    Git {
        received_objects: usize,
        total_objects: usize,
        bytes: u64,
    },
}

impl SourceProgress {
    /// Completion fraction in `0.0..=1.0`, or `None` when the total is unknown.
    pub fn fraction(&self) -> Option<f64> {
        match self {
            SourceProgress::Download { bytes, total_bytes } => total_bytes.map(|t| {
                if t == 0 {
                    0.0
                } else {
                    (*bytes as f64 / t as f64).min(1.0)
                }
            }),
            SourceProgress::Git {
                received_objects,
                total_objects,
                ..
            } => (*total_objects > 0).then(|| *received_objects as f64 / *total_objects as f64),
        }
    }
}

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

/// A per-source message from a fetcher task to the drain loop.
enum SourceMsg {
    Progress {
        index: usize,
        progress: SourceProgress,
    },
    Done {
        index: usize,
    },
    Error {
        index: usize,
        error: String,
    },
}

/// Fetch every source in `sources` into `dest_dir` concurrently, reporting through
/// `handler`. The parallel fetchers funnel through one channel so the handler is
/// called sequentially. Returns a per-source result in `sources` order.
pub async fn fetch_sources(
    sources: &[Source],
    dest_dir: &Path,
    handler: &mut impl SourceHandler,
) -> Vec<Result<(), String>> {
    handler.on_sources_start(sources);

    // Ensure the destination exists; per-source fetches surface any real error.
    let _ = std::fs::create_dir_all(dest_dir);

    let (tx, mut rx) = mpsc::unbounded_channel::<SourceMsg>();
    let mut tasks = Vec::with_capacity(sources.len());
    for (index, source) in sources.iter().enumerate() {
        let source = source.clone();
        let dest_dir = dest_dir.to_path_buf();
        let tx = tx.clone();
        tasks.push(tokio::spawn(async move {
            fetch_one(&source, &dest_dir, index, &tx).await
        }));
    }
    // Drop our sender so the channel closes once every fetcher finishes.
    drop(tx);

    while let Some(msg) = rx.recv().await {
        match msg {
            SourceMsg::Progress { index, progress } => {
                handler.on_source_progress(&sources[index], &progress)
            }
            SourceMsg::Done { index } => handler.on_source_done(&sources[index]),
            SourceMsg::Error { index, error } => handler.on_source_error(&sources[index], &error),
        }
    }

    let mut results = Vec::with_capacity(tasks.len());
    for task in tasks {
        results.push(task.await.unwrap_or_else(|e| Err(e.to_string())));
    }

    handler.on_sources_end();
    results
}

/// Fetch one source and emit its terminal `Done`/`Error` message.
async fn fetch_one(
    source: &Source,
    dest_dir: &Path,
    index: usize,
    tx: &mpsc::UnboundedSender<SourceMsg>,
) -> Result<(), String> {
    let result = match &source.kind {
        SourceKind::Http { url } => fetch_http(url, source, dest_dir, index, tx).await,
        SourceKind::Git { url, reference } => {
            fetch_git(url, reference.as_ref(), source, dest_dir, index, tx).await
        }
        SourceKind::Local { path } => fetch_local(path, source, dest_dir, index, tx).await,
    };
    let _ = match &result {
        Ok(()) => tx.send(SourceMsg::Done { index }),
        Err(error) => tx.send(SourceMsg::Error {
            index,
            error: error.clone(),
        }),
    };
    result
}

async fn fetch_http(
    url: &str,
    source: &Source,
    dest_dir: &Path,
    index: usize,
    tx: &mpsc::UnboundedSender<SourceMsg>,
) -> Result<(), String> {
    let dest = dest_dir.join(source.dest_name());
    if dest.exists() {
        let size = std::fs::metadata(&dest).map(|m| m.len()).unwrap_or(0);
        let _ = tx.send(SourceMsg::Progress {
            index,
            progress: SourceProgress::Download {
                bytes: size,
                total_bytes: Some(size),
            },
        });
        return Ok(());
    }

    let client = reqwest::Client::builder()
        .user_agent("koca")
        .build()
        .map_err(|e| e.to_string())?;

    let mut response = client.get(url).send().await.map_err(|e| e.to_string())?;
    if !response.status().is_success() {
        return Err(format!("HTTP {}", response.status()));
    }

    let total = response.content_length().filter(|&l| l > 0);
    let tmp = NamedTempFile::new_in(dest_dir).map_err(|e| e.to_string())?;
    let tmp_path = tmp.into_temp_path();
    let mut file = tokio::fs::File::create(&tmp_path)
        .await
        .map_err(|e| e.to_string())?;

    let mut downloaded: u64 = 0;
    let _ = tx.send(SourceMsg::Progress {
        index,
        progress: SourceProgress::Download {
            bytes: 0,
            total_bytes: total,
        },
    });
    while let Some(chunk) = response.chunk().await.map_err(|e| e.to_string())? {
        tokio::io::AsyncWriteExt::write_all(&mut file, &chunk)
            .await
            .map_err(|e| e.to_string())?;
        downloaded += chunk.len() as u64;
        let _ = tx.send(SourceMsg::Progress {
            index,
            progress: SourceProgress::Download {
                bytes: downloaded,
                total_bytes: total,
            },
        });
    }

    drop(file);
    tmp_path.persist(&dest).map_err(|e| e.to_string())?;
    Ok(())
}

/// Init an empty repo at `dest` and shallow-fetch `refspec` from `url` into it.
fn init_and_fetch(
    dest: &Path,
    url: &str,
    refspec: &str,
    fo: &mut git2::FetchOptions,
) -> Result<git2::Repository, String> {
    let repo = git2::Repository::init(dest).map_err(|e| e.message().to_string())?;
    repo.remote("origin", url)
        .map_err(|e| e.message().to_string())?
        .fetch(&[refspec], Some(fo), None)
        .map_err(|e| e.message().to_string())?;
    Ok(repo)
}

/// Detached-checkout `oid` in `repo`, surfacing git errors as strings.
fn checkout_oid(repo: &git2::Repository, oid: git2::Oid) -> Result<(), String> {
    let commit = repo.find_commit(oid).map_err(|e| e.message().to_string())?;
    repo.checkout_tree(commit.as_object(), None)
        .map_err(|e| e.message().to_string())?;
    repo.set_head_detached(oid)
        .map_err(|e| e.message().to_string())?;
    Ok(())
}

async fn fetch_git(
    url: &str,
    reference: Option<&GitRef>,
    source: &Source,
    dest_dir: &Path,
    index: usize,
    tx: &mpsc::UnboundedSender<SourceMsg>,
) -> Result<(), String> {
    let dest = dest_dir.join(source.dest_name());
    if dest.exists() {
        return Ok(());
    }
    let url = url.to_string();
    let reference = reference.cloned();
    let tx = tx.clone();

    tokio::task::spawn_blocking(move || {
        let mut cb = git2::RemoteCallbacks::new();
        let tx_cb = tx.clone();
        cb.transfer_progress(move |stats| {
            let _ = tx_cb.send(SourceMsg::Progress {
                index,
                progress: SourceProgress::Git {
                    received_objects: stats.received_objects(),
                    total_objects: stats.total_objects(),
                    bytes: stats.received_bytes() as u64,
                },
            });
            true
        });

        let mut fo = git2::FetchOptions::new();
        fo.remote_callbacks(cb);
        fo.depth(1);

        let result = (|| -> Result<(), String> {
            match &reference {
                // A bare commit hash: init an empty repo and fetch exactly that
                // object, then detach onto it.
                Some(GitRef::Commit(hash)) => {
                    let repo = init_and_fetch(&dest, &url, hash, &mut fo)?;
                    let oid = git2::Oid::from_str(hash).map_err(|e| e.message().to_string())?;
                    checkout_oid(&repo, oid)
                }
                // A tag: RepoBuilder won't materialise a tag under a shallow
                // (depth=1) fetch -- its default refspec only follows
                // refs/heads/*, so the tag ref is never written and the
                // checkout fails. Fetch the tag ref explicitly instead, then
                // peel it (tags may be annotated) to the commit and detach.
                Some(GitRef::Tag(tag)) => {
                    let tag_ref = format!("refs/tags/{tag}");
                    let repo =
                        init_and_fetch(&dest, &url, &format!("+{tag_ref}:{tag_ref}"), &mut fo)?;
                    let oid = repo
                        .find_reference(&tag_ref)
                        .and_then(|r| r.peel_to_commit())
                        .map_err(|e| e.message().to_string())?
                        .id();
                    checkout_oid(&repo, oid)
                }
                // A branch or the default branch: RepoBuilder checks out a real
                // branch correctly, so let it drive the clone.
                other => {
                    let mut builder = git2::build::RepoBuilder::new();
                    builder.fetch_options(fo);
                    if let Some(GitRef::Branch(b)) = other {
                        builder.branch(b);
                    }
                    builder
                        .clone(&url, &dest)
                        .map_err(|e| e.message().to_string())?;
                    Ok(())
                }
            }
        })();

        if result.is_err() {
            let _ = std::fs::remove_dir_all(&dest);
        }
        result
    })
    .await
    .map_err(|e| e.to_string())?
}

async fn fetch_local(
    path: &Path,
    source: &Source,
    dest_dir: &Path,
    index: usize,
    tx: &mpsc::UnboundedSender<SourceMsg>,
) -> Result<(), String> {
    let dest = dest_dir.join(source.dest_name());
    tokio::fs::copy(path, &dest)
        .await
        .map_err(|e| e.to_string())?;

    let size = tokio::fs::metadata(&dest)
        .await
        .map(|m| m.len())
        .unwrap_or(0);
    let _ = tx.send(SourceMsg::Progress {
        index,
        progress: SourceProgress::Download {
            bytes: size,
            total_bytes: Some(size),
        },
    });
    Ok(())
}
