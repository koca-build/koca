pub mod alpm;
pub mod apt;
pub mod error;
pub mod transport;
pub mod types;

use transport::KocaListener;

use crate::{KocaError, KocaResult};

pub use error::ProtoError;
pub use transport::{socket_name, BackendSession, KocaSession};
pub use types::{
    ActionKind, Command, DownloadEvent, ErrorCode, Event, InstallEvent,
    InstalledStatus, Message, MessageBody, PackageStatus, PlannedAction, ProtocolError,
    RemoveEvent, Request, ResultPayload,
};

// ── BackendKind ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
pub enum BackendKind {
    Apt,
    Alpm,
}

// ── Backend (sudo subprocess with socket IPC) ────────────────────────────

/// A running backend subprocess and its associated [`KocaSession`].
///
/// Used only for privileged operations that require sudo.
/// Non-privileged calls (check-installed, install-plan) should use the
/// handler functions directly via [`BackendKind`].
pub struct Backend {
    pub session: KocaSession,
    pub child: tokio::process::Child,
}

impl Backend {
    /// Spawn a privileged backend subprocess and wait for it to connect.
    pub async fn spawn(kind: BackendKind, sudo: bool) -> KocaResult<Self> {
        let name = socket_name();
        let listener = KocaListener::listen(&name).map_err(proto_to_koca)?;
        let child = spawn_process(kind, sudo, &name)?;
        let session = listener.accept().await.map_err(proto_to_koca)?;
        Ok(Self { session, child })
    }

    /// Send a simple request and wait for the result (no streaming events).
    pub async fn call(&mut self, cmd: Command) -> KocaResult<ResultPayload> {
        self.session.call(cmd).await.map_err(proto_to_koca)
    }

    /// Send a streaming command and drive the event loop with a tick callback.
    pub async fn call_streaming(
        &mut self,
        cmd: Command,
        mut callback: impl FnMut(Option<&Event>),
    ) -> KocaResult<ResultPayload> {
        self.session.send(cmd).await.map_err(proto_to_koca)?;

        let mut ticker = tokio::time::interval(std::time::Duration::from_millis(80));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        loop {
            tokio::select! {
                _ = ticker.tick() => {
                    callback(None);
                }
                msg = self.session.recv() => {
                    match msg.map_err(proto_to_koca)? {
                        MessageBody::Event { event } => callback(Some(&event)),
                        MessageBody::Result { result } => return Ok(result),
                        MessageBody::Error { error } => {
                            return Err(KocaError::IO(std::io::Error::other(error.to_string())))
                        }
                    }
                }
            }
        }
    }

    /// Send `Shutdown` and wait for the backend process to exit cleanly.
    pub async fn shutdown(mut self) -> KocaResult<()> {
        self.session.shutdown().await.map_err(proto_to_koca)?;
        let _ = self.child.wait().await;
        Ok(())
    }
}

// ── Shared backend loop (for subprocess side) ────────────────────────────

/// Run the backend command loop. Called from `koca internal backend-{apt,alpm}`.
pub async fn run_backend_loop(socket: &str, kind: BackendKind) -> anyhow::Result<()> {
    let mut session = BackendSession::connect(socket).await?;
    let mut pending: Option<Vec<String>> = None;

    loop {
        let req = session.recv().await?;
        let id = req.id;

        match req.cmd {
            Command::CheckInstalled { packages } => {
                let body = match dispatch_check_installed(kind, &packages) {
                    Ok(result) => MessageBody::Result { result },
                    Err(e) => MessageBody::Error { error: e },
                };
                session.send(&Message { id, body }).await?;
            }

            Command::InstallPlan { packages } => {
                match dispatch_install_plan(kind, &packages) {
                    Ok((result, pkg_names)) => {
                        pending = Some(pkg_names);
                        session
                            .send(&Message {
                                id,
                                body: MessageBody::Result { result },
                            })
                            .await?;
                    }
                    Err(e) => {
                        session
                            .send(&Message {
                                id,
                                body: MessageBody::Error { error: e },
                            })
                            .await?;
                    }
                }
            }

            Command::Install { packages } => {
                dispatch_commit(kind, id, packages, false, &mut session).await;
            }

            Command::Confirm => {
                if let Some(pkgs) = pending.take() {
                    dispatch_commit(kind, id, pkgs, false, &mut session).await;
                } else {
                    session
                        .send(&Message {
                            id,
                            body: MessageBody::Error {
                                error: ProtocolError {
                                    code: ErrorCode::Internal,
                                    message: "no pending transaction to confirm".into(),
                                },
                            },
                        })
                        .await?;
                }
            }

            Command::Abort => {
                pending = None;
                session
                    .send(&Message {
                        id,
                        body: MessageBody::Result {
                            result: ResultPayload::Aborted,
                        },
                    })
                    .await?;
            }

            Command::Remove { packages } => {
                dispatch_commit(kind, id, packages, true, &mut session).await;
            }

            Command::Shutdown => break,
        }
    }

    Ok(())
}

// ── Dispatch helpers ─────────────────────────────────────────────────────

pub fn dispatch_check_installed(
    kind: BackendKind,
    packages: &[String],
) -> Result<ResultPayload, ProtocolError> {
    match kind {
        BackendKind::Apt => apt::check_installed(packages),
        BackendKind::Alpm => alpm::check_installed(packages),
    }
}

pub fn dispatch_install_plan(
    kind: BackendKind,
    packages: &[String],
) -> Result<(ResultPayload, Vec<String>), ProtocolError> {
    match kind {
        BackendKind::Apt => apt::install_plan(packages),
        BackendKind::Alpm => alpm::install_plan(packages),
    }
}

async fn dispatch_commit(
    kind: BackendKind,
    msg_id: u64,
    packages: Vec<String>,
    is_remove: bool,
    session: &mut BackendSession,
) {
    match kind {
        BackendKind::Apt => apt::commit_transaction(msg_id, packages, is_remove, session).await,
        BackendKind::Alpm => alpm::commit_transaction(msg_id, packages, is_remove, session).await,
    }
}

// ── Process spawning ─────────────────────────────────────────────────────

fn spawn_process(
    kind: BackendKind,
    sudo: bool,
    socket_name: &str,
) -> KocaResult<tokio::process::Child> {
    let exe = std::env::current_exe().map_err(KocaError::IO)?;

    let subcommand = match kind {
        BackendKind::Apt => "backend-apt",
        BackendKind::Alpm => "backend-alpm",
    };

    let mut cmd = if sudo && !nix::unistd::geteuid().is_root() {
        let mut c = tokio::process::Command::new("sudo");
        c.arg(&exe);
        c
    } else {
        tokio::process::Command::new(&exe)
    };

    cmd.arg("internal")
        .arg(subcommand)
        .arg("--socket")
        .arg(socket_name);

    cmd.stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());

    cmd.spawn().map_err(KocaError::IO)
}

pub fn proto_to_koca(e: ProtoError) -> KocaError {
    KocaError::IO(std::io::Error::other(e.to_string()))
}
