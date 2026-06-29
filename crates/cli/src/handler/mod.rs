pub mod plain;
pub mod tui;

use std::io::{self, Write};
use std::process::ExitStatus;

use async_trait::async_trait;
use koca::handler::ElevatedChild;
use tokio::io::AsyncReadExt;

/// A privileged child backed by a [`tokio::process::Child`].
///
/// Spawn it with `stderr` piped: it's captured in the background and replayed to
/// our stderr when the child finishes, so a `sudo` auth error or backend panic
/// surfaces instead of being swallowed. The backend writes only to stderr (its
/// stdout is unused; apt output is captured internally and progress flows over
/// the socket), so a single stream is all we need — and ordering is trivially
/// preserved.
pub struct TokioElevatedChild {
    child: tokio::process::Child,
    stderr: Option<tokio::task::JoinHandle<Vec<u8>>>,
}

impl TokioElevatedChild {
    pub fn new(mut child: tokio::process::Child) -> Self {
        let stderr = child.stderr.take().map(|mut reader| {
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let _ = reader.read_to_end(&mut buf).await;
                buf
            })
        });
        Self { child, stderr }
    }
}

#[async_trait]
impl ElevatedChild for TokioElevatedChild {
    async fn wait(&mut self) -> io::Result<ExitStatus> {
        let status = self.child.wait().await?;
        if let Some(task) = self.stderr.take() {
            if let Ok(buf) = task.await {
                let mut err = io::stderr();
                let _ = err.write_all(&buf);
                let _ = err.flush();
            }
        }
        Ok(status)
    }
}
