//! Traits the Koca library reports and escalates through, so a consumer brings its
//! own UI and privilege strategy. Methods default to no-ops; pass by `&mut impl`.

use std::collections::HashMap;
use std::io;
use std::path::PathBuf;
use std::process::ExitStatus;

use async_trait::async_trait;

use crate::backend::DependencyEvent;
use crate::file::BuildOutputLine;
use crate::source::{Source, SourceProgress};

/// A command to launch as root that connects back to the backend socket.
#[derive(Clone, Debug)]
pub struct ElevateCommandSpec {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub env: HashMap<String, String>,
}

/// A privileged child the library can wait on.
#[async_trait]
pub trait ElevatedChild: Send {
    async fn wait(&mut self) -> io::Result<ExitStatus>;
}

/// Dependency phase: resolve, install, and remove (incl. `--rm-deps`).
#[async_trait]
#[allow(unused_variables)]
pub trait DependencyHandler {
    fn on_resolve_start(&mut self) {}
    fn on_resolve_end(&mut self) {}
    /// Start of the install phase. Both counts come from the resolved [`Plan`]:
    /// `installs` is the number of package actions, `downloads` the subset that
    /// must be fetched. Reported once here — the streamed [`DependencyEvent`]s
    /// carry only per-item progress, never a re-sent total.
    ///
    /// [`Plan`]: crate::Plan
    fn on_install_start(&mut self, downloads: u32, installs: u32) {}
    fn on_remove_start(&mut self, removes: u32) {}
    fn on_dep_event(&mut self, event: &DependencyEvent) {}
    fn on_install_end(&mut self) {}
    fn on_remove_end(&mut self) {}

    /// Run `cmd` as root and hand back the child. How root is obtained (sudo,
    /// pkexec, a PTY, …) is your choice, but run it faithfully — preserve `program`,
    /// `args`, and especially `env`. Some escalation tools scrub the environment —
    /// `sudo`, for example — so pass them as explicit `VAR=value` arguments.
    async fn elevate(&mut self, cmd: ElevateCommandSpec) -> io::Result<Box<dyn ElevatedChild>>;
}

/// Source-fetching phase. Per-source callbacks keyed on the `&Source`.
#[allow(unused_variables)]
pub trait SourceHandler {
    fn on_sources_start(&mut self, sources: &[Source]) {}
    fn on_source_progress(&mut self, source: &Source, progress: &SourceProgress) {}
    fn on_source_done(&mut self, source: &Source) {}
    fn on_source_error(&mut self, source: &Source, error: &str) {}
    fn on_sources_end(&mut self) {}
}

/// `build()` (once) and `package()` (once per split package).
#[allow(unused_variables)]
pub trait BuildHandler {
    fn on_build_start(&mut self) {}
    fn on_build_line(&mut self, line: &BuildOutputLine) {}
    fn on_build_end(&mut self) {}
    fn on_build_failed(&mut self) {}
    fn on_package_start(&mut self, pkgname: &str) {}
    fn on_package_line(&mut self, pkgname: &str, line: &BuildOutputLine) {}
    fn on_package_end(&mut self, pkgname: &str, output_files: &[PathBuf]) {}
    fn on_package_failed(&mut self, pkgname: &str) {}
}
