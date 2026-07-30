//! Runs an elevated command on a pseudo-terminal so `sudo` can authenticate by
//! any PAM method (password, fingerprint, …) while the caller keeps owning the
//! real terminal. The caller renders the PTY's output and forwards keystrokes
//! through the returned [`SudoPty`].

use std::io::{Read, Write};
use std::os::unix::process::ExitStatusExt;
use std::process::ExitStatus;

use async_trait::async_trait;
use koca::handler::{ElevateCommandSpec, ElevatedChild};
use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtySize};
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};

/// The terminal bridge to a running `sudo`: line snapshots of its output and a
/// sink for keystrokes to forward to it.
pub struct SudoPty {
    pub lines: UnboundedReceiver<Vec<String>>,
    pub keys: UnboundedSender<Vec<u8>>,
}

/// Spawn `spec` under `sudo` on a PTY of `cols`×`rows`. `sudo`'s `use_pty` puts
/// the *PTY* into raw mode rather than the real terminal, so the caller's
/// rendering is undisturbed. `sudo` scrubs the environment, so `spec.env` is
/// reapplied through `env`.
pub fn spawn(
    spec: ElevateCommandSpec,
    cols: u16,
    rows: u16,
) -> std::io::Result<(SudoPty, Box<dyn ElevatedChild>)> {
    let pty = native_pty_system()
        .openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| std::io::Error::other(e.to_string()))?;

    let mut cmd = CommandBuilder::new("sudo");
    cmd.arg("env");
    for (key, value) in &spec.env {
        cmd.arg(format!("{key}={value}"));
    }
    cmd.arg(&spec.program);
    cmd.args(&spec.args);
    if let Ok(cwd) = std::env::current_dir() {
        cmd.cwd(cwd);
    }

    let child = pty
        .slave
        .spawn_command(cmd)
        .map_err(|e| std::io::Error::other(e.to_string()))?;
    drop(pty.slave);

    let reader = pty
        .master
        .try_clone_reader()
        .map_err(|e| std::io::Error::other(e.to_string()))?;
    let writer = pty
        .master
        .take_writer()
        .map_err(|e| std::io::Error::other(e.to_string()))?;

    let (lines_tx, lines_rx) = unbounded_channel();
    let (keys_tx, keys_rx) = unbounded_channel();
    std::thread::spawn(move || read_lines(reader, lines_tx));
    std::thread::spawn(move || write_keys(writer, keys_rx));

    let bridge = SudoPty {
        lines: lines_rx,
        keys: keys_tx,
    };
    let child = Box::new(PtyChild {
        master: pty.master,
        child: Some(child),
    });
    Ok((bridge, child))
}

struct PtyChild {
    /// Held only to keep the backend's controlling terminal open until the child
    /// is reaped; never read directly.
    #[allow(dead_code)]
    master: Box<dyn MasterPty + Send>,
    child: Option<Box<dyn portable_pty::Child + Send + Sync>>,
}

#[async_trait]
impl ElevatedChild for PtyChild {
    async fn wait(&mut self) -> std::io::Result<ExitStatus> {
        let Some(mut child) = self.child.take() else {
            return Err(std::io::Error::other("already waited"));
        };
        let status = tokio::task::spawn_blocking(move || child.wait())
            .await
            .map_err(|e| std::io::Error::other(e.to_string()))?
            .map_err(|e| std::io::Error::other(e.to_string()))?;
        Ok(ExitStatus::from_raw(
            ((status.exit_code() & 0xff) as i32) << 8,
        ))
    }
}

/// Read the PTY in a loop, maintaining the current line set (`\r` rewrites the
/// current line, `\n` commits it) and emitting the whole set whenever it changes.
fn read_lines(mut reader: Box<dyn Read + Send>, tx: UnboundedSender<Vec<String>>) {
    let mut lines: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut buf = [0u8; 1024];
    loop {
        match reader.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                // The PTY translates `\n` to `\r\n` on output; collapse it so a
                // line ending commits its content instead of a lone `\r` clearing it.
                let chunk = String::from_utf8_lossy(&buf[..n]).replace("\r\n", "\n");
                for ch in chunk.chars() {
                    match ch {
                        '\n' => lines.push(std::mem::take(&mut current)),
                        '\r' => current.clear(),
                        c if c.is_control() => {}
                        c => current.push(c),
                    }
                }
                let mut snapshot = lines.clone();
                if !current.is_empty() {
                    snapshot.push(current.clone());
                }
                if tx.send(snapshot).is_err() {
                    break;
                }
            }
        }
    }
}

/// Forward keystrokes from the caller to the PTY.
fn write_keys(mut writer: Box<dyn Write + Send>, mut keys: UnboundedReceiver<Vec<u8>>) {
    while let Some(bytes) = keys.blocking_recv() {
        if writer.write_all(&bytes).is_err() {
            break;
        }
        let _ = writer.flush();
    }
}
