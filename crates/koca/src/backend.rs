use koca_proto::{
    socket_name,
    transport::{KocaListener, KocaSession},
    Command, MessageBody, ProtoError, ResultPayload,
};

use crate::{KocaError, KocaResult};

/// A running backend process and its associated [`KocaSession`].
pub struct Backend {
    pub session: KocaSession,
    pub child: tokio::process::Child,
}

impl Backend {
    /// Spawn a backend binary and wait for it to connect.
    ///
    /// Creates a local socket, spawns the binary with `--socket <name>`,
    /// then accepts the connection.
    pub async fn spawn(binary: &str, sudo: bool) -> KocaResult<Self> {
        let name = socket_name();

        // Create listener BEFORE spawning so the socket exists when the child
        // tries to connect.
        let listener = KocaListener::listen(&name).map_err(proto_to_koca)?;

        let child = spawn_process(binary, sudo, &name)?;

        let session = listener.accept().await.map_err(proto_to_koca)?;

        Ok(Self { session, child })
    }

    /// Send a simple request and wait for the result (no streaming events).
    pub async fn call(&mut self, cmd: Command) -> KocaResult<ResultPayload> {
        self.session.call(cmd).await.map_err(proto_to_koca)
    }

    /// Send a streaming command (e.g. `Confirm`) and drive the event loop.
    ///
    /// `callback` is called with `None` every ~80ms for spinner animation,
    /// and with `Some(&event)` for each progress event from the backend.
    /// Using a single callback avoids the double-borrow problem when the
    /// caller needs `&mut` access to shared state from both tick and event
    /// paths.
    pub async fn call_streaming(
        &mut self,
        cmd: Command,
        mut callback: impl FnMut(Option<&koca_proto::Event>),
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

/// Resolve a binary name to its absolute path by searching `PATH`.
fn resolve_binary(binary: &str) -> KocaResult<std::path::PathBuf> {
    std::env::var_os("PATH")
        .and_then(|paths| {
            std::env::split_paths(&paths)
                .map(|dir| dir.join(binary))
                .find(|p| p.is_file())
        })
        .ok_or_else(|| {
            KocaError::IO(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("backend binary '{binary}' not found in PATH"),
            ))
        })
}

fn spawn_process(binary: &str, sudo: bool, socket_name: &str) -> KocaResult<tokio::process::Child> {
    // Resolve to absolute path so sudo (which uses a restricted PATH) can
    // find the binary.
    let bin_path = resolve_binary(binary)?;

    let mut cmd = if sudo && !nix::unistd::geteuid().is_root() {
        let mut c = tokio::process::Command::new("sudo");
        c.arg(&bin_path);
        c
    } else {
        tokio::process::Command::new(&bin_path)
    };

    cmd.arg("--socket").arg(socket_name);

    cmd.stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());

    cmd.spawn().map_err(KocaError::IO)
}

pub fn proto_to_koca(e: ProtoError) -> KocaError {
    KocaError::IO(std::io::Error::other(e.to_string()))
}
