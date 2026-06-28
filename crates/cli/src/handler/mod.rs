pub mod plain;
pub mod tui;

use std::io;
use std::process::ExitStatus;

use async_trait::async_trait;
use koca::handler::{ElevateCommandSpec, ElevatedChild};

/// A privileged child backed by a plain [`tokio::process::Child`].
pub struct TokioElevatedChild(pub tokio::process::Child);

#[async_trait]
impl ElevatedChild for TokioElevatedChild {
    async fn wait(&mut self) -> io::Result<ExitStatus> {
        self.0.wait().await
    }
}

/// Build the command that runs `spec` as root with inherited stdio.
///
/// Already root: run it directly. Otherwise wrap in `sudo env VAR=val …` —
/// `sudo` scrubs the environment, so the `__KOCA_*` vars are reapplied past it
/// through `env`, which works regardless of the sudoers env policy.
pub fn elevate_command(spec: &ElevateCommandSpec) -> tokio::process::Command {
    if nix::unistd::geteuid().is_root() {
        let mut cmd = tokio::process::Command::new(&spec.program);
        cmd.args(&spec.args);
        cmd.envs(&spec.env);
        cmd
    } else {
        let mut cmd = tokio::process::Command::new("sudo");
        cmd.arg("env");
        for (key, value) in &spec.env {
            cmd.arg(format!("{key}={value}"));
        }
        cmd.arg(&spec.program);
        cmd.args(&spec.args);
        cmd
    }
}
