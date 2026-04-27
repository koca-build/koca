use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Instant;

use koca_proto::{
    ActionKind, BackendSession, DownloadEvent as ProtoDownloadEvent, ErrorCode,
    Event as ProtoEvent, InstallEvent as ProtoInstallEvent, InstalledStatus, Message, MessageBody,
    PackageStatus, PlannedAction, ProtocolError, RemoveEvent as ProtoRemoveEvent, ResultPayload,
};
use tokio::sync::mpsc;

const LOCAL_DB: &str = "/var/lib/pacman/local";
const SYNC_DB: &str = "/var/lib/pacman/sync";
const LOG_FILE: &str = "/var/log/pacman.log";
const LOCK_FILE: &str = "/var/lib/pacman/db.lck";

// ── Local DB ─────────────────────────────────────────────────────────────

struct LocalPkg {
    version: String,
    size: u64,
    reason: u32,
}

fn read_local_pkg(name: &str) -> Option<LocalPkg> {
    let local = Path::new(LOCAL_DB);
    for entry in std::fs::read_dir(local).ok()?.flatten() {
        let desc_path = entry.path().join("desc");
        let content = std::fs::read_to_string(&desc_path).ok()?;
        let fields = parse_desc(&content);
        if fields.get("NAME").map(|n| n.as_str()) == Some(name) {
            return Some(LocalPkg {
                version: fields.get("VERSION").cloned().unwrap_or_default(),
                size: parse_u64(&fields, "SIZE"),
                reason: fields.get("REASON").and_then(|s| s.parse().ok()).unwrap_or(0),
            });
        }
    }
    None
}

// ── Sync DB ──────────────────────────────────────────────────────────────

struct SyncPkg {
    csize: u64,
    isize: u64,
}

fn read_sync_pkgs(names: &[String]) -> HashMap<String, SyncPkg> {
    let mut result = HashMap::new();
    let sync = Path::new(SYNC_DB);
    let dbs = match std::fs::read_dir(sync) {
        Ok(d) => d,
        Err(_) => return result,
    };

    for db_entry in dbs.flatten() {
        let path = db_entry.path();
        if !path.extension().is_some_and(|e| e == "db") {
            continue;
        }
        let file = match std::fs::File::open(&path) {
            Ok(f) => f,
            Err(_) => continue,
        };
        let gz = flate2::read::GzDecoder::new(file);
        let mut archive = tar::Archive::new(gz);
        let entries = match archive.entries() {
            Ok(e) => e,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let entry_path = match entry.path() {
                Ok(p) => p.to_string_lossy().to_string(),
                Err(_) => continue,
            };
            if !entry_path.ends_with("/desc") {
                continue;
            }
            let mut content = String::new();
            let mut entry = entry;
            if entry.read_to_string(&mut content).is_err() {
                continue;
            }
            let fields = parse_desc(&content);
            if let Some(name) = fields.get("NAME") {
                if names.contains(name) && !result.contains_key(name) {
                    result.insert(
                        name.clone(),
                        SyncPkg {
                            csize: parse_u64(&fields, "CSIZE"),
                            isize: parse_u64(&fields, "ISIZE"),
                        },
                    );
                }
            }
        }
    }

    result
}

// ── Desc parser ──────────────────────────────────────────────────────────

fn parse_desc(content: &str) -> HashMap<String, String> {
    let mut fields = HashMap::new();
    let mut key: Option<String> = None;
    for line in content.lines() {
        if line.starts_with('%') && line.ends_with('%') {
            key = Some(line[1..line.len() - 1].to_string());
        } else if line.is_empty() {
            key = None;
        } else if let Some(ref k) = key {
            fields.entry(k.clone()).or_insert_with(|| line.to_string());
        }
    }
    fields
}

fn parse_u64(fields: &HashMap<String, String>, key: &str) -> u64 {
    fields.get(key).and_then(|s| s.parse().ok()).unwrap_or(0)
}

// ── Pacman CLI ───────────────────────────────────────────────────────────

fn run_pacman(args: &[&str]) -> Result<std::process::Output, ProtocolError> {
    std::process::Command::new("pacman")
        .args(args)
        .env("LC_ALL", "C")
        .output()
        .map_err(|e| ProtocolError {
            code: ErrorCode::Internal,
            message: format!("failed to run pacman: {e}"),
        })
}

/// NOTE: These error strings come from pacman source and are stable in practice
/// but not formally guaranteed. LC_ALL=C ensures English output.
fn classify_pacman_error(stderr: &str) -> ErrorCode {
    if stderr.contains("error: target not found:") {
        ErrorCode::PackageNotFound
    } else if stderr.contains("error: could not lock database") {
        ErrorCode::DatabaseLocked
    } else if stderr.contains("error: failed to commit transaction") {
        ErrorCode::TransactionFailed
    } else {
        ErrorCode::Internal
    }
}

fn check_preconditions() -> Result<(), ProtocolError> {
    if !nix::unistd::geteuid().is_root() {
        return Err(ProtocolError {
            code: ErrorCode::NeedsElevation,
            message: "must be run as root".into(),
        });
    }
    if Path::new(LOCK_FILE).exists() {
        return Err(ProtocolError {
            code: ErrorCode::DatabaseLocked,
            message: "pacman database is locked".into(),
        });
    }
    Ok(())
}

// ── Log tailer ───────────────────────────────────────────────────────────

fn tail_pacman_log(
    start_pos: u64,
    n_pkgs: u32,
    is_remove: bool,
    tx: &mpsc::UnboundedSender<ProtoEvent>,
) {
    let mut current: u32 = 0;
    let mut sent_start = false;
    let mut pos = start_pos;

    for _ in 0..600 {
        let file = match std::fs::File::open(LOG_FILE) {
            Ok(f) => f,
            Err(_) => {
                std::thread::sleep(std::time::Duration::from_millis(100));
                continue;
            }
        };
        let mut reader = BufReader::new(file);
        let _ = reader.seek(SeekFrom::Start(pos));

        for line in reader.lines().map_while(Result::ok) {
            pos += line.len() as u64 + 1; // +1 for newline
            let Some(msg) = extract_alpm_msg(&line) else {
                continue;
            };

            if msg == "transaction started" && !sent_start {
                sent_start = true;
            } else if msg == "transaction completed" {
                let _ = tx.send(if is_remove {
                    ProtoEvent::Remove {
                        inner: ProtoRemoveEvent::Done,
                    }
                } else {
                    ProtoEvent::Install {
                        inner: ProtoInstallEvent::Done,
                    }
                });
                return;
            } else if let Some(hook) = msg.strip_prefix("running '") {
                let hook = hook.trim_end_matches("'...");
                if !is_remove {
                    let _ = tx.send(ProtoEvent::Install {
                        inner: ProtoInstallEvent::Hook {
                            name: hook.to_string(),
                            current: 0,
                            total: 0,
                        },
                    });
                }
            } else if let Some(pkg) = parse_pkg_action(msg) {
                current += 1;
                let _ = tx.send(if is_remove {
                    ProtoEvent::Remove {
                        inner: ProtoRemoveEvent::ItemDone {
                            package: pkg,
                            current,
                            total: n_pkgs,
                        },
                    }
                } else {
                    ProtoEvent::Install {
                        inner: ProtoInstallEvent::ItemDone {
                            package: pkg,
                            current,
                            total: n_pkgs,
                        },
                    }
                });
            }
        }

        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

/// `[timestamp] [ALPM] message` → `message`
fn extract_alpm_msg(line: &str) -> Option<&str> {
    let rest = line.strip_prefix('[')?.split_once("] ")?.1;
    rest.strip_prefix("[ALPM] ")
}

/// `"installed nano (9.0-1)"` → `Some("nano")`
fn parse_pkg_action(msg: &str) -> Option<String> {
    for prefix in ["installed ", "upgraded ", "downgraded ", "reinstalled ", "removed "] {
        if let Some(rest) = msg.strip_prefix(prefix) {
            return Some(rest.split_once(' ')?.0.to_string());
        }
    }
    None
}

// ── Protocol handlers ────────────────────────────────────────────────────

pub fn check_installed(packages: &[String]) -> Result<ResultPayload, ProtocolError> {
    let statuses = packages
        .iter()
        .map(|name| match read_local_pkg(name) {
            Some(pkg) => PackageStatus {
                name: name.clone(),
                status: InstalledStatus::Installed,
                version: Some(pkg.version),
                is_auto: Some(pkg.reason == 1),
            },
            None => PackageStatus {
                name: name.clone(),
                status: InstalledStatus::Missing,
                version: None,
                is_auto: None,
            },
        })
        .collect();

    Ok(ResultPayload::CheckInstalled { packages: statuses })
}

pub fn install_plan(packages: &[String]) -> Result<(ResultPayload, Vec<String>), ProtocolError> {
    let output = run_pacman(
        &[&["-S", "--print", "--print-format", "%n|%v|%r"], packages.iter().map(|s| s.as_str()).collect::<Vec<_>>().as_slice()].concat(),
    )?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(ProtocolError {
            code: classify_pacman_error(&stderr),
            message: stderr.trim().to_string(),
        });
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let targets: Vec<(String, String, String)> = stdout
        .lines()
        .filter_map(|line| {
            let mut parts = line.splitn(3, '|');
            Some((
                parts.next()?.to_string(),
                parts.next()?.to_string(),
                parts.next()?.to_string(),
            ))
        })
        .collect();

    let names: Vec<String> = targets.iter().map(|(n, _, _)| n.clone()).collect();
    let sync_pkgs = read_sync_pkgs(&names);

    let actions: Vec<PlannedAction> = targets
        .iter()
        .map(|(name, version, _)| {
            let sync = sync_pkgs.get(name);
            let local = read_local_pkg(name);
            let (action, old_version) = match local {
                None => (ActionKind::Install, None),
                Some(ref l) if l.version == *version => {
                    (ActionKind::Reinstall, Some(l.version.clone()))
                }
                Some(ref l) if l.version.as_str() < version.as_str() => {
                    (ActionKind::Upgrade, Some(l.version.clone()))
                }
                Some(l) => (ActionKind::Downgrade, Some(l.version)),
            };

            PlannedAction {
                name: name.clone(),
                version: version.clone(),
                old_version,
                action,
                download_size: sync.map(|s| s.csize).unwrap_or(0),
                install_size: sync.map(|s| s.isize).unwrap_or(0),
            }
        })
        .collect();

    let total_download: u64 = actions.iter().map(|a| a.download_size).sum();
    let total_install: u64 = actions.iter().map(|a| a.install_size).sum();

    Ok((
        ResultPayload::InstallPlan {
            actions,
            total_download,
            total_install,
        },
        names,
    ))
}

// ── Download helpers ─────────────────────────────────────────────────────

const CACHE_DIR: &str = "/var/cache/pacman/pkg";

struct DownloadItem {
    package: String,
    url: String,
    filename: String,
    size: u64,
}

/// Extract a package name from a pacman archive filename.
/// e.g. `gcc-15.2.1+r604+g0b99615a8aef-1-x86_64.pkg.tar.zst` → `gcc`
///
/// Pacman filenames use `{name}-{version}-{pkgrel}-{arch}.pkg.tar.{ext}`.
/// The version can contain hyphens (e.g. `15.2.1+r604-1`), so we strip the
/// `.pkg.tar.*` suffix, then pop the last three `-`-separated segments
/// (arch, pkgrel, version) to get the name.
fn package_name_from_filename(filename: &str) -> String {
    let base = filename
        .strip_suffix(".pkg.tar.zst")
        .or_else(|| filename.strip_suffix(".pkg.tar.xz"))
        .or_else(|| filename.strip_suffix(".pkg.tar.gz"))
        .unwrap_or(filename);
    // Pop arch, pkgrel, version (3 segments from the right).
    let mut rest = base;
    for _ in 0..3 {
        if let Some(pos) = rest.rfind('-') {
            rest = &rest[..pos];
        } else {
            return filename.to_string();
        }
    }
    rest.to_string()
}

/// Get download URLs via `pacman -Sp`, with sizes from the sync DB.
fn get_download_urls(packages: &[String]) -> Result<Vec<DownloadItem>, ProtocolError> {
    let mut args: Vec<&str> = vec!["-Sp"];
    for p in packages {
        args.push(p);
    }
    let output = run_pacman(&args)?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(ProtocolError {
            code: classify_pacman_error(&stderr),
            message: stderr.trim().to_string(),
        });
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut items = Vec::new();
    let mut all_names = Vec::new();
    for line in stdout.lines() {
        let url = line.trim().to_string();
        if url.is_empty() {
            continue;
        }
        let filename = url.rsplit('/').next().unwrap_or("").to_string();
        let package = package_name_from_filename(&filename);
        all_names.push(package.clone());
        items.push(DownloadItem {
            package,
            url,
            filename,
            size: 0,
        });
    }

    // Fill in sizes from sync DB.
    let sync_pkgs = read_sync_pkgs(&all_names);
    for item in &mut items {
        if let Some(sync) = sync_pkgs.get(&item.package) {
            item.size = sync.csize;
        }
    }

    Ok(items)
}

/// Download packages to the pacman cache dir with parallel downloads, emitting progress events.
async fn download_packages(
    items: &[DownloadItem],
    tx: &mpsc::UnboundedSender<ProtoEvent>,
) -> Result<(), ProtocolError> {
    use std::sync::{Arc, Mutex};
    use tokio::sync::Semaphore;

    let client = reqwest::Client::new();
    let n_pkgs = items.len() as u32;

    // Filter out cached items.
    let mut to_download: Vec<(String, String, String, u64)> = Vec::new();
    for item in items {
        if item.url.starts_with("file://") || PathBuf::from(CACHE_DIR).join(&item.filename).exists() {
            let _ = tx.send(ProtoEvent::Download {
                inner: ProtoDownloadEvent::ItemDone {
                    package: item.package.clone(),
                },
            });
            continue;
        }
        to_download.push((
            item.package.clone(),
            item.url.clone(),
            item.filename.clone(),
            item.size,
        ));
    }

    let total_bytes: u64 = to_download.iter().map(|(_, _, _, s)| s).sum();
    let _ = tx.send(ProtoEvent::Download {
        inner: ProtoDownloadEvent::Start {
            total_bytes,
            total_packages: n_pkgs,
        },
    });

    if to_download.is_empty() {
        let _ = tx.send(ProtoEvent::Download { inner: ProtoDownloadEvent::Done });
        return Ok(());
    }

    // Shared progress state for parallel downloads.
    let state = Arc::new(Mutex::new((0u64, Vec::<String>::new()))); // (done_bytes, active_names)
    let sem = Arc::new(Semaphore::new(4));
    let mut handles = Vec::new();

    for (package, url, filename, _size) in to_download {
        let client = client.clone();
        let tx = tx.clone();
        let state = state.clone();
        let sem = sem.clone();
        let total = total_bytes;

        handles.push(tokio::spawn(async move {
            let _permit = sem.acquire().await.unwrap();
            state.lock().unwrap().1.push(package.clone());
            {
                let s = state.lock().unwrap();
                let _ = tx.send(ProtoEvent::Download {
                    inner: ProtoDownloadEvent::Progress {
                        bytes_done: s.0,
                        bytes_total: total,
                        percent: None,
                        active: s.1.clone(),
                    },
                });
            }

            let mut resp = client.get(&url).send().await.map_err(|e| ProtocolError {
                code: ErrorCode::Internal,
                message: format!("download failed for {filename}: {e}"),
            })?;
            if !resp.status().is_success() {
                return Err(ProtocolError {
                    code: ErrorCode::Internal,
                    message: format!("HTTP {} for {url}", resp.status()),
                });
            }

            let dest = PathBuf::from(CACHE_DIR).join(&filename);
            let mut file = std::fs::File::create(&dest).map_err(|e| ProtocolError {
                code: ErrorCode::Internal,
                message: format!("create {}: {e}", dest.display()),
            })?;

            let mut last_emit = Instant::now();
            while let Some(chunk) = resp.chunk().await.map_err(|e| ProtocolError {
                code: ErrorCode::Internal,
                message: format!("download {filename}: {e}"),
            })? {
                std::io::Write::write_all(&mut file, &chunk).map_err(|e| ProtocolError {
                    code: ErrorCode::Internal,
                    message: format!("write: {e}"),
                })?;
                let mut s = state.lock().unwrap();
                s.0 += chunk.len() as u64;
                if last_emit.elapsed() >= std::time::Duration::from_millis(80) {
                    let _ = tx.send(ProtoEvent::Download {
                        inner: ProtoDownloadEvent::Progress {
                            bytes_done: s.0,
                            bytes_total: total,
                            percent: None,
                            active: s.1.clone(),
                        },
                    });
                    last_emit = Instant::now();
                }
            }

            state.lock().unwrap().1.retain(|n| n != &package);
            {
                let s = state.lock().unwrap();
                let _ = tx.send(ProtoEvent::Download {
                    inner: ProtoDownloadEvent::Progress {
                        bytes_done: s.0,
                        bytes_total: total,
                        percent: None,
                        active: s.1.clone(),
                    },
                });
            }
            let _ = tx.send(ProtoEvent::Download {
                inner: ProtoDownloadEvent::ItemDone { package },
            });
            Ok::<(), ProtocolError>(())
        }));
    }

    for h in handles {
        h.await.unwrap_or_else(|_| Err(ProtocolError {
            code: ErrorCode::Internal, message: "download task panicked".into(),
        }))?;
    }

    let _ = tx.send(ProtoEvent::Download { inner: ProtoDownloadEvent::Done });
    Ok(())
}

// ── commit (install or remove) ───────────────────────────────────────────

pub async fn commit_transaction(
    msg_id: u64,
    packages: Vec<String>,
    is_remove: bool,
    session: &mut BackendSession,
) {
    if let Err(e) = check_preconditions() {
        let _ = session
            .send(&Message {
                id: msg_id,
                body: MessageBody::Error { error: e },
            })
            .await;
        return;
    }

    let (event_tx, mut event_rx) = mpsc::unbounded_channel::<ProtoEvent>();
    let pkgs = packages.clone();
    let n_pkgs = pkgs.len() as u32;

    let join_handle = tokio::spawn(async move {
        if is_remove {
            // Remove: just run pacman -R directly.
            let _ = event_tx.send(ProtoEvent::Remove {
                inner: ProtoRemoveEvent::Start {
                    total_packages: n_pkgs,
                },
            });
            return tokio::task::spawn_blocking(move || {
                let log_pos = std::fs::metadata(LOG_FILE).map(|m| m.len()).unwrap_or(0);
                let mut args = vec!["-R", "--noconfirm"];
                for p in &pkgs {
                    args.push(p);
                }
                let child = std::process::Command::new("pacman")
                    .args(&args)
                    .env("LC_ALL", "C")
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .spawn()
                    .map_err(|e| ProtocolError {
                        code: ErrorCode::Internal,
                        message: format!("failed to spawn pacman: {e}"),
                    })?;

                let log_tx = event_tx.clone();
                let log_handle = std::thread::spawn(move || {
                    tail_pacman_log(log_pos, n_pkgs, true, &log_tx);
                });

                let output = child.wait_with_output().map_err(|e| ProtocolError {
                    code: ErrorCode::Internal,
                    message: format!("failed to wait for pacman: {e}"),
                })?;
                let _ = log_handle.join();
                drop(event_tx);

                if !output.status.success() {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    return Err(ProtocolError {
                        code: classify_pacman_error(&stderr),
                        message: stderr.trim().to_string(),
                    });
                }
                Ok(pkgs)
            }).await.unwrap_or_else(|_| Err(ProtocolError {
                code: ErrorCode::Internal,
                message: "task panicked".into(),
            }));
        }

        // Install: download first, then install from cache.
        let items = match tokio::task::spawn_blocking({
            let pkgs = pkgs.clone();
            move || get_download_urls(&pkgs)
        }).await {
            Ok(Ok(i)) => i,
            Ok(Err(e)) => {
                drop(event_tx);
                return Err(e);
            }
            Err(_) => {
                drop(event_tx);
                return Err(ProtocolError {
                    code: ErrorCode::Internal,
                    message: "download URL task panicked".into(),
                });
            }
        };

        let resolved_count = items.len() as u32;

        if let Err(e) = download_packages(&items, &event_tx).await {
            drop(event_tx);
            return Err(e);
        }

        // Install via normal pacman -S — it finds cached files automatically.
        let _ = event_tx.send(ProtoEvent::Install {
            inner: ProtoInstallEvent::Start {
                total_packages: resolved_count,
            },
        });
        let tx_for_install = event_tx.clone();
        tokio::task::spawn_blocking(move || {
            let log_pos = std::fs::metadata(LOG_FILE).map(|m| m.len()).unwrap_or(0);
            let mut args = vec!["-S", "--noconfirm"];
            for p in &pkgs {
                args.push(p);
            }

            let child = std::process::Command::new("pacman")
                .args(&args)
                .env("LC_ALL", "C")
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .map_err(|e| ProtocolError {
                    code: ErrorCode::Internal,
                    message: format!("failed to spawn pacman: {e}"),
                })?;

            let log_tx = tx_for_install.clone();
            let log_handle = std::thread::spawn(move || {
                tail_pacman_log(log_pos, resolved_count, false, &log_tx);
            });

            let output = child.wait_with_output().map_err(|e| ProtocolError {
                code: ErrorCode::Internal,
                message: format!("failed to wait for pacman: {e}"),
            })?;
            let _ = log_handle.join();
            drop(tx_for_install);

            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                return Err(ProtocolError {
                    code: classify_pacman_error(&stderr),
                    message: stderr.trim().to_string(),
                });
            }
            // Mark all installed packages as deps so they can be cleaned up.
            let mut mark_args: Vec<&str> = vec!["-D", "--asdeps"];
            for p in &pkgs {
                mark_args.push(p);
            }
            let _ = run_pacman(&mark_args);
            Ok(pkgs)
        }).await.unwrap_or_else(|_| Err(ProtocolError {
            code: ErrorCode::Internal,
            message: "task panicked".into(),
        }))
    });

    // Drop our sender so channel closes when task finishes.
    // (already moved into the task)

    while let Some(evt) = event_rx.recv().await {
        let _ = session
            .send(&Message {
                id: msg_id,
                body: MessageBody::Event { event: evt },
            })
            .await;
    }

    let body = match join_handle.await {
        Ok(Ok(names)) => MessageBody::Result {
            result: if is_remove {
                ResultPayload::Remove {
                    success: true,
                    removed: names,
                }
            } else {
                ResultPayload::Install {
                    success: true,
                    installed: names,
                }
            },
        },
        Ok(Err(e)) => MessageBody::Error { error: e },
        Err(_) => MessageBody::Error {
            error: ProtocolError {
                code: ErrorCode::Internal,
                message: "backend task panicked".into(),
            },
        },
    };

    let _ = session.send(&Message { id: msg_id, body }).await;
}
