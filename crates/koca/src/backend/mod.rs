pub mod alpm;
pub mod apt;
pub mod error;
pub mod transport;
pub mod types;

use std::collections::HashMap;

use transport::KocaListener;

use crate::handler::{DependencyHandler, ElevateCommandSpec};
use crate::init::{HELPER_VAR, SOCKET_VAR};
use crate::{KocaError, KocaResult};

pub use error::ProtoError;
pub use transport::{socket_name, BackendSession, KocaSession};
pub use types::{
    ActionKind, Command, DependencyEvent, DownloadEvent, ErrorCode, InstallEvent, InstalledStatus,
    Message, MessageBody, PackageStatus, PlannedAction, ProtocolError, RemoveEvent, Request,
    ResultPayload,
};

// ── BackendKind ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
pub enum BackendKind {
    Apt,
    Alpm,
}

// ── Backend (sudo subprocess with socket IPC) ────────────────────────────

/// A connected, privileged backend and its associated [`KocaSession`].
///
/// The library is privilege-agnostic: it never spawns `sudo` itself. A backend
/// is obtained by handing an [`ElevateCommandSpec`] to a
/// [`DependencyHandler::elevate`] implementation, which launches the helper as
/// root; the helper connects back to a socket we listen on. The re-exec'd
/// process is caught by [`crate::init`].
pub struct Backend {
    pub session: KocaSession,
    pub child: Box<dyn crate::handler::ElevatedChild>,
}

impl Backend {
    /// Obtain an elevated backend.
    ///
    /// Listens on a fresh socket, builds the helper [`ElevateCommandSpec`]
    /// (`/proc/self/exe` + `__KOCA_*`), and hands it to `handler.elevate`. The
    /// socket `accept()` is raced against the child exiting, so a failed
    /// escalation (e.g. a mistyped `sudo` password) aborts promptly instead of
    /// hanging on a connection that will never come.
    pub async fn connect_elevated(
        kind: BackendKind,
        handler: &mut impl DependencyHandler,
    ) -> KocaResult<Self> {
        let name = socket_name();
        let listener = KocaListener::listen(&name).map_err(proto_to_koca)?;

        let exe = std::env::current_exe().map_err(KocaError::IO)?;
        let helper = match kind {
            BackendKind::Apt => "backend-apt",
            BackendKind::Alpm => "backend-alpm",
        };
        let mut env = HashMap::new();
        env.insert(HELPER_VAR.to_string(), helper.to_string());
        env.insert(SOCKET_VAR.to_string(), name.clone());

        let spec = ElevateCommandSpec {
            program: exe,
            args: Vec::new(),
            env,
        };
        let mut child = handler.elevate(spec).await.map_err(KocaError::IO)?;

        let session = {
            let accept_fut = listener.accept();
            let wait_fut = child.wait();
            tokio::pin!(accept_fut, wait_fut);
            tokio::select! {
                accepted = accept_fut => accepted.map_err(proto_to_koca)?,
                status = wait_fut => {
                    let _ = status;
                    return Err(KocaError::ElevationFailed);
                }
            }
        };
        Ok(Self { session, child })
    }

    /// Send a simple request and wait for the result (no streaming events).
    pub async fn call(&mut self, cmd: Command) -> KocaResult<ResultPayload> {
        self.session.call(cmd).await.map_err(proto_to_koca)
    }

    /// Send a streaming command and forward each event to `handler`.
    pub async fn call_streaming(
        &mut self,
        cmd: Command,
        handler: &mut impl DependencyHandler,
    ) -> KocaResult<ResultPayload> {
        self.session.send(cmd).await.map_err(proto_to_koca)?;

        loop {
            match self.session.recv().await.map_err(proto_to_koca)? {
                MessageBody::Event { event } => handler.on_dep_event(&event),
                MessageBody::Result { result } => return Ok(result),
                MessageBody::Error { error } => {
                    return Err(KocaError::IO(std::io::Error::other(error.to_string())))
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

/// Run the backend command loop. Dispatched by [`crate::init`] when a process
/// is re-exec'd as a `backend-{apt,alpm}` helper.
pub async fn run_backend(socket: &str, kind: BackendKind) -> anyhow::Result<()> {
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

            Command::InstallPlan { packages } => match dispatch_install_plan(kind, &packages) {
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
            },

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

pub fn proto_to_koca(e: ProtoError) -> KocaError {
    KocaError::IO(std::io::Error::other(e.to_string()))
}
