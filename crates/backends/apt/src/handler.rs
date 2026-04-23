use std::cmp::Ordering;

use koca_proto::{
    ActionKind, BackendSession, DownloadEvent as ProtoDownloadEvent, ErrorCode,
    Event as ProtoEvent, InstallEvent as ProtoInstallEvent, InstalledStatus, Message, MessageBody,
    PackageStatus, PlannedAction, ProtocolError, RemoveEvent as ProtoRemoveEvent, ResultPayload,
};
use rust_apt::cache::Cache;
use rust_apt::progress::{
    AcquireProgress, DynAcquireProgress, DynInstallProgress, InstallProgress,
};
use rust_apt::raw::{AcqTextStatus, ItemDesc, PkgAcquire};
use tokio::sync::mpsc;

/// Open an APT cache.
fn open_cache() -> Result<Cache, String> {
    Cache::new::<String>(&[]).map_err(|e| format!("{e}"))
}

/// Returns `true` if the error message indicates a lock is held.
fn is_lock_error(msg: &str) -> bool {
    let lower = msg.to_lowercase();
    lower.contains("unable to lock") || lower.contains("could not open lock")
}

/// Returns `true` if the error message suggests a permission problem.
fn is_permission_error(msg: &str) -> bool {
    let lower = msg.to_lowercase();
    lower.contains("permission denied")
}

// ── check-installed ───────────────────────────────────────────────────────

pub fn check_installed(packages: &[String]) -> Result<ResultPayload, ProtocolError> {
    let cache = open_cache().map_err(|e| ProtocolError {
        code: ErrorCode::Internal,
        message: e,
    })?;

    let statuses = packages
        .iter()
        .map(|name| match cache.get(name) {
            Some(pkg) if pkg.is_installed() => {
                let version = pkg.installed().map(|v| v.version().to_string());
                PackageStatus {
                    name: name.clone(),
                    status: InstalledStatus::Installed,
                    version,
                    is_auto: Some(pkg.is_auto_installed()),
                }
            }
            _ => PackageStatus {
                name: name.clone(),
                status: InstalledStatus::Missing,
                version: None,
                is_auto: None,
            },
        })
        .collect();

    Ok(ResultPayload::CheckInstalled { packages: statuses })
}

// ── install-plan ──────────────────────────────────────────────────────────

/// Build an install plan.
///
/// Returns `(plan_payload, resolved_package_names)`.
pub fn install_plan(packages: &[String]) -> Result<(ResultPayload, Vec<String>), ProtocolError> {
    let cache = open_cache().map_err(|e| ProtocolError {
        code: if is_lock_error(&e) {
            ErrorCode::DatabaseLocked
        } else if is_permission_error(&e) {
            ErrorCode::NeedsElevation
        } else {
            ErrorCode::Internal
        },
        message: e,
    })?;

    let mut resolved_names = Vec::with_capacity(packages.len());

    for name in packages {
        let pkg = cache.get(name).ok_or_else(|| ProtocolError {
            code: ErrorCode::PackageNotFound,
            message: format!("package not found: {name}"),
        })?;

        resolved_names.push(pkg.name().to_string());

        if pkg.is_installed() {
            // Already installed — nothing to do, APT will skip it.
            continue;
        }

        // Mark for install: auto_inst=true (pull in deps), from_user=false (mark as auto)
        if !pkg.mark_install(true, false) {
            return Err(ProtocolError {
                code: ErrorCode::DependencyConflict,
                message: format!("failed to mark {name} for installation"),
            });
        }
        pkg.protect();
    }

    cache.resolve(true).map_err(|e| ProtocolError {
        code: ErrorCode::DependencyConflict,
        message: format!("{e}"),
    })?;

    let actions: Vec<PlannedAction> = cache
        .get_changes(false)
        .map(|pkg| {
            let cand = pkg.candidate();
            let installed = pkg.installed();
            let new_version = cand
                .as_ref()
                .map(|v| v.version().to_string())
                .unwrap_or_default();
            let old_version = installed.as_ref().map(|v| v.version().to_string());

            let action = match (&old_version, &cand) {
                (None, _) => ActionKind::Install,
                (Some(old), Some(new)) => match rust_apt::util::cmp_versions(old, new.version()) {
                    Ordering::Less => ActionKind::Upgrade,
                    Ordering::Greater => ActionKind::Downgrade,
                    Ordering::Equal => ActionKind::Reinstall,
                },
                _ => ActionKind::Install,
            };

            let download_size = cand.as_ref().map(|v| v.size()).unwrap_or(0);
            let install_size = cand.as_ref().map(|v| v.installed_size()).unwrap_or(0);

            PlannedAction {
                name: pkg.name().to_string(),
                version: new_version,
                old_version,
                action,
                download_size,
                install_size,
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
        resolved_names,
    ))
}

// ── commit (install or remove) ────────────────────────────────────────────

/// Run an install or remove transaction, streaming progress events to koca.
///
/// The APT cache is created on a dedicated OS thread (Cache is `!Send`).
/// Callbacks post events into a `tokio::sync::mpsc` channel which the async
/// task drains and forwards over the socket.
pub async fn commit_transaction(
    msg_id: u64,
    packages: Vec<String>,
    is_remove: bool,
    session: &mut BackendSession,
) {
    let (event_tx, mut event_rx) = mpsc::unbounded_channel::<ProtoEvent>();

    let pkgs = packages.clone();
    let tx_for_thread = event_tx.clone();

    let join_handle = std::thread::spawn(move || -> Result<Vec<String>, ProtocolError> {
        // TODO: Implement debconf relay to handle config prompts through the TUI.
        // For now, use defaults for any debconf questions (e.g. tzdata timezone).
        std::env::set_var("DEBIAN_FRONTEND", "noninteractive");

        let cache = open_cache().map_err(|e| ProtocolError {
            code: if is_permission_error(&e) {
                ErrorCode::NeedsElevation
            } else {
                ErrorCode::Internal
            },
            message: e,
        })?;

        if is_remove {
            for name in &pkgs {
                if let Some(pkg) = cache.get(name) {
                    if pkg.is_installed() {
                        if !pkg.mark_delete(false) {
                            return Err(ProtocolError {
                                code: ErrorCode::DependencyConflict,
                                message: format!("failed to mark {name} for removal"),
                            });
                        }
                    }
                    // Not installed — skip silently
                }
            }
        } else {
            for name in &pkgs {
                let pkg = cache.get(name).ok_or_else(|| ProtocolError {
                    code: ErrorCode::PackageNotFound,
                    message: format!("package not found: {name}"),
                })?;

                if pkg.is_installed() {
                    continue;
                }

                if !pkg.mark_install(true, false) {
                    return Err(ProtocolError {
                        code: ErrorCode::DependencyConflict,
                        message: format!("failed to mark {name} for installation"),
                    });
                }
                pkg.protect();
            }
        }

        cache.resolve(true).map_err(|e| ProtocolError {
            code: ErrorCode::DependencyConflict,
            message: format!("{e}"),
        })?;

        let n_pkgs = cache.get_changes(false).count() as u32;

        // Send start event
        let total_download = cache.depcache().download_size();
        if is_remove {
            let _ = tx_for_thread.send(ProtoEvent::Remove {
                inner: ProtoRemoveEvent::Start {
                    total_packages: n_pkgs,
                },
            });
        } else {
            let _ = tx_for_thread.send(ProtoEvent::Download {
                inner: ProtoDownloadEvent::Start {
                    total_bytes: total_download,
                    total_packages: n_pkgs,
                },
            });
        }

        // Build progress callbacks that send to the channel.
        // For removes, APT still runs the acquire phase but there's nothing
        // to download — suppress download events so they don't confuse the
        // TUI state machine.
        let dl_tx = tx_for_thread.clone();
        let mut acquire_progress = AcquireProgress::new(KocaAcquireProgress {
            tx: dl_tx,
            suppress: is_remove,
        });

        let inst_tx = tx_for_thread.clone();
        let mut install_progress = InstallProgress::new(KocaInstallProgress {
            tx: inst_tx,
            is_remove,
            sent_start: false,
            n_pkgs,
        });

        cache
            .commit(&mut acquire_progress, &mut install_progress)
            .map_err(|e| {
                let msg = format!("{e}");
                ProtocolError {
                    code: if is_lock_error(&msg) {
                        ErrorCode::DatabaseLocked
                    } else if is_permission_error(&msg) {
                        ErrorCode::NeedsElevation
                    } else {
                        ErrorCode::TransactionFailed
                    },
                    message: msg,
                }
            })?;

        // Send done event
        if is_remove {
            let _ = tx_for_thread.send(ProtoEvent::Remove {
                inner: ProtoRemoveEvent::Done,
            });
        } else {
            let _ = tx_for_thread.send(ProtoEvent::Install {
                inner: ProtoInstallEvent::Done,
            });
        }

        drop(tx_for_thread);
        Ok(pkgs)
    });

    // Drop our own clone so the channel closes when the thread's clones are dropped
    drop(event_tx);

    // Stream events to koca as they arrive
    while let Some(evt) = event_rx.recv().await {
        let _ = session
            .send(&Message {
                id: msg_id,
                body: MessageBody::Event { event: evt },
            })
            .await;
    }

    // Thread is done — retrieve result and send final message
    let body = match join_handle.join() {
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
                message: "backend thread panicked".into(),
            },
        },
    };

    let _ = session.send(&Message { id: msg_id, body }).await;
}

// ── Progress adapters ─────────────────────────────────────────────────────

/// Acquire (download) progress adapter that sends proto events over a channel.
/// When `suppress` is true (remove transactions), all callbacks are no-ops —
/// APT still runs the acquire phase during removes but there's nothing to
/// download, and emitting download events would confuse the TUI state machine.
struct KocaAcquireProgress {
    tx: mpsc::UnboundedSender<ProtoEvent>,
    suppress: bool,
}

impl DynAcquireProgress for KocaAcquireProgress {
    fn pulse_interval(&self) -> usize {
        500_000
    }

    fn hit(&mut self, _item: &ItemDesc) {}

    fn fetch(&mut self, _item: &ItemDesc) {}

    fn fail(&mut self, _item: &ItemDesc) {}

    fn pulse(&mut self, status: &AcqTextStatus, owner: &PkgAcquire) {
        if self.suppress {
            return;
        }
        let active: Vec<String> = owner
            .workers()
            .iter()
            .filter_map(|w| w.item().ok().map(|i| i.short_desc()))
            .collect();

        let _ = self.tx.send(ProtoEvent::Download {
            inner: ProtoDownloadEvent::Progress {
                bytes_done: status.current_bytes(),
                bytes_total: status.total_bytes(),
                active,
            },
        });
    }

    fn done(&mut self, item: &ItemDesc) {
        if self.suppress {
            return;
        }
        let _ = self.tx.send(ProtoEvent::Download {
            inner: ProtoDownloadEvent::ItemDone {
                package: item.short_desc(),
            },
        });
    }

    fn start(&mut self) {}

    fn stop(&mut self, _status: &AcqTextStatus) {
        if self.suppress {
            return;
        }
        let _ = self.tx.send(ProtoEvent::Download {
            inner: ProtoDownloadEvent::Done,
        });
    }
}

/// Install progress adapter that sends proto events over a channel.
struct KocaInstallProgress {
    tx: mpsc::UnboundedSender<ProtoEvent>,
    is_remove: bool,
    sent_start: bool,
    n_pkgs: u32,
}

impl KocaInstallProgress {
    /// Emit the phase-start event once, on the first progress callback.
    fn ensure_start_sent(&mut self) {
        if self.sent_start {
            return;
        }
        self.sent_start = true;
        let evt = if self.is_remove {
            ProtoEvent::Remove {
                inner: ProtoRemoveEvent::Start {
                    total_packages: self.n_pkgs,
                },
            }
        } else {
            ProtoEvent::Install {
                inner: ProtoInstallEvent::Start {
                    total_packages: self.n_pkgs,
                },
            }
        };
        let _ = self.tx.send(evt);
    }
}

impl DynInstallProgress for KocaInstallProgress {
    fn status_changed(
        &mut self,
        pkgname: String,
        steps_done: u64,
        total_steps: u64,
        action: String,
    ) {
        self.ensure_start_sent();

        let percent = if total_steps > 0 {
            Some(((steps_done as f64 / total_steps as f64) * 100.0) as u32)
        } else {
            None
        };

        let evt = if self.is_remove {
            ProtoEvent::Remove {
                inner: ProtoRemoveEvent::Action {
                    package: pkgname.clone(),
                    action: action.to_lowercase(),
                    current: steps_done as u32,
                    total: total_steps as u32,
                    percent,
                },
            }
        } else {
            ProtoEvent::Install {
                inner: ProtoInstallEvent::Action {
                    package: pkgname.clone(),
                    action: action.to_lowercase(),
                    current: steps_done as u32,
                    total: total_steps as u32,
                    percent,
                },
            }
        };
        let _ = self.tx.send(evt);
    }

    fn error(&mut self, pkgname: String, steps_done: u64, total_steps: u64, error: String) {
        self.ensure_start_sent();
        let evt = if self.is_remove {
            ProtoEvent::Remove {
                inner: ProtoRemoveEvent::Action {
                    package: pkgname,
                    action: format!("error: {error}"),
                    current: steps_done as u32,
                    total: total_steps as u32,
                    percent: None,
                },
            }
        } else {
            ProtoEvent::Install {
                inner: ProtoInstallEvent::Action {
                    package: pkgname,
                    action: format!("error: {error}"),
                    current: steps_done as u32,
                    total: total_steps as u32,
                    percent: None,
                },
            }
        };
        let _ = self.tx.send(evt);
    }
}
