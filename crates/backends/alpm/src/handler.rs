use alpm::{
    Alpm, AnyDownloadEvent, AnyEvent, DownloadEvent, DownloadResult, Error as AlpmError, Event,
    PackageReason, Progress, TransFlag,
};
use koca_proto::{
    ActionKind, BackendSession, DownloadEvent as ProtoDownloadEvent, ErrorCode,
    Event as ProtoEvent, InstallEvent as ProtoInstallEvent, InstalledStatus, Message, MessageBody,
    PackageStatus, PlannedAction, ProtocolError, RemoveEvent as ProtoRemoveEvent, ResultPayload,
};
use tokio::sync::mpsc;

/// Open an ALPM handle configured from the system's pacman.conf.
fn open_handle() -> Result<Alpm, String> {
    let conf = pacmanconf::Config::new().map_err(|e| e.to_string())?;
    alpm_utils::alpm_with_conf(&conf).map_err(|e| e.to_string())
}

/// Returns `true` if the ALPM error indicates the database/handle is locked.
fn is_lock_error(e: &AlpmError) -> bool {
    matches!(e, AlpmError::HandleLock)
}

/// Returns `true` if the ALPM error indicates a permission problem.
fn is_permission_error(e: &AlpmError) -> bool {
    matches!(e, AlpmError::DbOpen)
        || e.to_string().to_lowercase().contains("permission")
        || e.to_string().to_lowercase().contains("access")
}

// ── check-installed ───────────────────────────────────────────────────────

pub fn check_installed(packages: &[String]) -> Result<ResultPayload, ProtocolError> {
    let handle = open_handle().map_err(|e| ProtocolError {
        code: ErrorCode::Internal,
        message: e,
    })?;

    let local = handle.localdb();
    let statuses = packages
        .iter()
        .map(|name| match local.pkg(name.as_str()) {
            Ok(pkg) => PackageStatus {
                name: name.clone(),
                status: InstalledStatus::Installed,
                version: Some(pkg.version().to_string()),
                is_auto: Some(pkg.reason() == PackageReason::Depend),
            },
            Err(_) => PackageStatus {
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
/// The resolved names may differ from the input if `find_satisfier` returned a
/// different package (e.g. a virtual provider).
pub fn install_plan(packages: &[String]) -> Result<(ResultPayload, Vec<String>), ProtocolError> {
    let mut handle = open_handle().map_err(|e| ProtocolError {
        code: ErrorCode::Internal,
        message: e,
    })?;

    handle
        .trans_init(TransFlag::NEEDED | TransFlag::NO_LOCK)
        .map_err(|e| {
            let code = if is_lock_error(&e) {
                ErrorCode::DatabaseLocked
            } else if is_permission_error(&e) {
                ErrorCode::NeedsElevation
            } else {
                ErrorCode::Internal
            };
            ProtocolError {
                code,
                message: e.to_string(),
            }
        })?;

    let local = handle.localdb();
    let mut resolved_names = Vec::with_capacity(packages.len());

    for name in packages {
        // First check if already installed via local DB
        if let Ok(pkg) = local.pkg(name.as_str()) {
            // Already installed at a version that satisfies NEEDED flag —
            // ALPM will skip it during trans_prepare. We still note its name.
            resolved_names.push(pkg.name().to_string());
            continue;
        }

        // Look up in sync DBs
        let pkg = handle
            .syncdbs()
            .find_satisfier(name.as_str())
            .ok_or_else(|| ProtocolError {
                code: ErrorCode::PackageNotFound,
                message: format!("package not found: {name}"),
            })?;

        resolved_names.push(pkg.name().to_string());
        handle.trans_add_pkg(pkg).map_err(|e| ProtocolError {
            code: ErrorCode::DependencyConflict,
            message: e.to_string(),
        })?;
    }

    handle.trans_prepare().map_err(|e| ProtocolError {
        code: ErrorCode::DependencyConflict,
        message: e.to_string(),
    })?;

    let local = handle.localdb();
    let actions: Vec<PlannedAction> = handle
        .trans_add()
        .iter()
        .map(|pkg| {
            let old_version = local.pkg(pkg.name()).ok().map(|p| p.version().to_string());
            let action = match &old_version {
                None => ActionKind::Install,
                Some(old) => {
                    // Compare versions to determine upgrade vs downgrade
                    use std::cmp::Ordering;
                    let cmp = alpm::vercmp(old.as_str(), pkg.version().as_str());
                    match cmp {
                        Ordering::Less => ActionKind::Upgrade,
                        Ordering::Greater => ActionKind::Downgrade,
                        Ordering::Equal => ActionKind::Reinstall,
                    }
                }
            };
            PlannedAction {
                name: pkg.name().to_string(),
                version: pkg.version().to_string(),
                old_version,
                action,
                download_size: pkg.size().max(0) as u64,
                install_size: pkg.isize().max(0) as u64,
            }
        })
        .collect();

    let total_download: u64 = actions.iter().map(|a| a.download_size).sum();
    let total_install: u64 = actions.iter().map(|a| a.install_size).sum();

    let _ = handle.trans_release();

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
/// The ALPM handle is created on a dedicated OS thread (Alpm is `!Send`).
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
        let mut handle = open_handle().map_err(|e| ProtocolError {
            code: ErrorCode::Internal,
            message: e,
        })?;

        // ── download callback ──────────────────────────────────────────────
        let dl_tx = tx_for_thread.clone();
        handle.set_dl_cb(dl_tx, |filename, dl_event: AnyDownloadEvent, tx| {
            let evt = match dl_event.event() {
                DownloadEvent::Init(_) => Some(ProtoEvent::Download {
                    inner: ProtoDownloadEvent::Progress {
                        package: strip_pkg_ext(filename),
                        bytes_done: 0,
                        bytes_total: 0,
                    },
                }),
                DownloadEvent::Progress(p) => Some(ProtoEvent::Download {
                    inner: ProtoDownloadEvent::Progress {
                        package: strip_pkg_ext(filename),
                        bytes_done: p.downloaded.max(0) as u64,
                        bytes_total: p.total.max(0) as u64,
                    },
                }),
                DownloadEvent::Completed(c) => {
                    if c.result != DownloadResult::Failed {
                        Some(ProtoEvent::Download {
                            inner: ProtoDownloadEvent::ItemDone {
                                package: strip_pkg_ext(filename),
                            },
                        })
                    } else {
                        None
                    }
                }
                DownloadEvent::Retry(_) => None,
            };
            if let Some(e) = evt {
                let _ = tx.send(e);
            }
        });

        // ── event callback ─────────────────────────────────────────────────
        let ev_tx = tx_for_thread.clone();
        let n_pkgs = pkgs.len() as u32;
        handle.set_event_cb(ev_tx, move |any_event: AnyEvent, tx| {
            let proto_evt = match any_event.event() {
                Event::RetrieveStart => Some(ProtoEvent::Download {
                    inner: ProtoDownloadEvent::Start {
                        total_bytes: 0,
                        total_packages: n_pkgs,
                    },
                }),
                Event::RetrieveDone => Some(ProtoEvent::Download {
                    inner: ProtoDownloadEvent::Done,
                }),
                Event::PkgRetrieveDone(_) => None,
                Event::TransactionStart => Some(if is_remove {
                    ProtoEvent::Remove {
                        inner: ProtoRemoveEvent::Start {
                            total_packages: n_pkgs,
                        },
                    }
                } else {
                    ProtoEvent::Install {
                        inner: ProtoInstallEvent::Start {
                            total_packages: n_pkgs,
                        },
                    }
                }),
                Event::TransactionDone => Some(if is_remove {
                    ProtoEvent::Remove {
                        inner: ProtoRemoveEvent::Done,
                    }
                } else {
                    ProtoEvent::Install {
                        inner: ProtoInstallEvent::Done,
                    }
                }),
                Event::HookRunStart(h) => {
                    if is_remove {
                        None
                    } else {
                        Some(ProtoEvent::Install {
                            inner: ProtoInstallEvent::Hook {
                                name: h.name().to_string(),
                                current: h.position() as u32,
                                total: h.total() as u32,
                            },
                        })
                    }
                }
                _ => None,
            };
            if let Some(e) = proto_evt {
                let _ = tx.send(e);
            }
        });

        // ── progress callback ──────────────────────────────────────────────
        let prog_tx = tx_for_thread.clone();
        handle.set_progress_cb(
            prog_tx,
            move |progress: Progress,
                  pkgname: &str,
                  percent: i32,
                  howmany: usize,
                  current: usize,
                  tx| {
                let action_str = match progress {
                    Progress::AddStart => "installing",
                    Progress::UpgradeStart => "upgrading",
                    Progress::DowngradeStart => "downgrading",
                    Progress::ReinstallStart => "reinstalling",
                    Progress::RemoveStart => "removing",
                    _ => "processing",
                };
                let evt = if is_remove {
                    ProtoEvent::Remove {
                        inner: ProtoRemoveEvent::Action {
                            package: pkgname.to_string(),
                            action: action_str.to_string(),
                            current: current as u32,
                            total: howmany as u32,
                            percent: Some(percent.max(0) as u32),
                        },
                    }
                } else {
                    ProtoEvent::Install {
                        inner: ProtoInstallEvent::Action {
                            package: pkgname.to_string(),
                            action: action_str.to_string(),
                            current: current as u32,
                            total: howmany as u32,
                            percent: Some(percent.max(0) as u32),
                        },
                    }
                };
                let _ = tx.send(evt);
            },
        );

        // ── transaction ────────────────────────────────────────────────────
        handle.trans_init(TransFlag::NEEDED).map_err(|e| {
            let code = if is_lock_error(&e) {
                ErrorCode::DatabaseLocked
            } else if is_permission_error(&e) {
                ErrorCode::NeedsElevation
            } else {
                ErrorCode::Internal
            };
            ProtocolError {
                code,
                message: e.to_string(),
            }
        })?;

        if is_remove {
            let local = handle.localdb();
            for name in &pkgs {
                match local.pkg(name.as_str()) {
                    Ok(pkg) => handle.trans_remove_pkg(pkg).map_err(|e| ProtocolError {
                        code: ErrorCode::DependencyConflict,
                        message: e.to_string(),
                    })?,
                    Err(_) => {
                        // Package not installed; skip silently
                    }
                }
            }
        } else {
            for name in &pkgs {
                let pkg = handle
                    .syncdbs()
                    .find_satisfier(name.as_str())
                    .ok_or_else(|| ProtocolError {
                        code: ErrorCode::PackageNotFound,
                        message: format!("package not found: {name}"),
                    })?;
                handle.trans_add_pkg(pkg).map_err(|e| ProtocolError {
                    code: ErrorCode::DependencyConflict,
                    message: e.to_string(),
                })?;
            }
        }

        handle.trans_prepare().map_err(|e| ProtocolError {
            code: ErrorCode::DependencyConflict,
            message: e.to_string(),
        })?;

        handle.trans_commit().map_err(|e| ProtocolError {
            code: ErrorCode::TransactionFailed,
            message: e.to_string(),
        })?;

        // Mark newly installed packages as auto (dependency reason)
        if !is_remove {
            let local = handle.localdb();
            for name in &pkgs {
                if let Ok(pkg) = local.pkg(name.as_str()) {
                    let _ = pkg.set_reason(PackageReason::Depend);
                }
            }
        }

        // Dropping handle closes the alpm handle, dropping tx_for_thread closes the channel
        drop(handle);
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

/// Strip the `.pkg.tar.zst` (and similar) extension from a downloaded filename.
fn strip_pkg_ext(filename: &str) -> String {
    // Remove trailing compression extension, then .pkg.tar
    let s = filename
        .strip_suffix(".zst")
        .or_else(|| filename.strip_suffix(".xz"))
        .or_else(|| filename.strip_suffix(".gz"))
        .unwrap_or(filename);
    let s = s.strip_suffix(".pkg.tar").unwrap_or(s);
    s.to_string()
}
