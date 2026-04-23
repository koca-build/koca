mod render;
mod ui;
mod viewport;

use koca::dep::DepConstraint;
use koca_proto::{ActionKind, DownloadEvent, Event, InstallEvent, PlannedAction, RemoveEvent};
use std::collections::HashMap;
use std::io::{self, Write};

pub use ui::KocaCreateTui;

// ── Shared state types used by both impls ──

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Resolve,
    Confirm,
    Download,
    Install,
    Build,
    Package,
    Done,
    Failed,
}

/// Tracks download progress from backend events.
pub struct DownloadState {
    pub total_bytes: u64,
    pub done_bytes: u64,
    pub total_packages: u32,
    pub done_count: u32,
    pub active_names: Vec<String>,
    pub tick: u32,
    // Also used during install phase
    pub current_install_pkg: Option<String>,
    pub install_current: u32,
    pub install_total: u32,
    /// Per-package progress tracking for accurate byte totals.
    pkg_done: HashMap<String, u64>,
    pkg_total: HashMap<String, u64>,
}

impl DownloadState {
    pub fn new() -> Self {
        Self {
            total_bytes: 0,
            done_bytes: 0,
            total_packages: 0,
            done_count: 0,
            active_names: Vec::new(),
            tick: 0,
            current_install_pkg: None,
            install_current: 0,
            install_total: 0,
            pkg_done: HashMap::new(),
            pkg_total: HashMap::new(),
        }
    }

    /// Update per-package progress and recompute aggregate totals.
    pub fn update_progress(&mut self, package: &str, bytes_done: u64, bytes_total: u64) {
        self.pkg_done.insert(package.to_string(), bytes_done);
        if bytes_total > 0 {
            self.pkg_total.insert(package.to_string(), bytes_total);
        }
        self.done_bytes = self.pkg_done.values().sum();
        self.total_bytes = self.total_bytes.max(self.pkg_total.values().sum());
    }
}

/// Summary of completed download+install for display in later phases.
pub struct InstallSummary {
    pub total_bytes: u64,
    pub installed_count: u32,
}

/// Tracks build/package output lines for the scrolling gutter.
pub struct BuildState {
    pub lines: Vec<String>,
}

impl BuildState {
    pub fn new() -> Self {
        Self { lines: Vec::new() }
    }

    pub fn push_line(&mut self, line: String) {
        self.lines.push(line);
    }
}

// ── Trait ──

pub trait CreateUi {
    /// Show the resolve spinner while dependencies are being resolved.
    fn start_resolve(&mut self) -> io::Result<()>;

    /// Show the confirmation screen. Returns true if user confirmed.
    fn show_confirm(
        &mut self,
        actions: &[PlannedAction],
        depends: &[DepConstraint],
        noconfirm: bool,
    ) -> io::Result<bool>;

    /// Handle a backend event (download/install progress).
    fn on_event(&mut self, event: &Event) -> io::Result<()>;

    /// Called periodically (~80ms) during streaming for spinner animation.
    fn tick(&mut self) -> io::Result<()>;

    /// Mark download+install as done and transition to build phase display.
    fn finish_install(&mut self, total_bytes: u64, installed_count: u32) -> io::Result<()>;

    /// Transition to build phase.
    fn start_build(&mut self) -> io::Result<()>;

    /// Feed a line of build output.
    fn on_build_line(&mut self, line: &str) -> io::Result<()>;

    /// Mark build as successful.
    fn finish_build(&mut self, pkgname: &str, version: &str) -> io::Result<()>;

    /// Transition to package phase.
    fn start_package(&mut self) -> io::Result<()>;

    /// Feed a line of package output.
    fn on_package_line(&mut self, line: &str) -> io::Result<()>;

    /// Mark packaging as successful.
    fn finish_package(&mut self, output_file: &str) -> io::Result<()>;

    /// Show failure state.
    fn show_failure(&mut self, phase_name: &str) -> io::Result<()>;

    /// Suspend the TUI for external I/O (e.g. sudo password prompt).
    fn suspend(&mut self) -> io::Result<()>;

    /// Resume the TUI after a suspend.
    fn resume(&mut self) -> io::Result<()>;

    /// Clean up terminal state.
    fn cleanup(&mut self);
}

// ── Plain CLI fallback ──

pub struct KocaCreateCli;

impl KocaCreateCli {
    pub fn new() -> Self {
        Self
    }
}

impl CreateUi for KocaCreateCli {
    fn start_resolve(&mut self) -> io::Result<()> {
        Ok(())
    }

    fn show_confirm(
        &mut self,
        actions: &[PlannedAction],
        _depends: &[DepConstraint],
        noconfirm: bool,
    ) -> io::Result<bool> {
        use zolt::Colorize;

        println!();
        println!(
            "{} {}",
            "Missing makedepends:".bold(),
            actions
                .iter()
                .map(|a| a.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
                .yellow()
        );
        println!();

        for action in actions {
            let (icon, color) = match action.action {
                ActionKind::Install => ("+", "green"),
                ActionKind::Upgrade => ("^", "yellow"),
                ActionKind::Downgrade => ("v", "yellow"),
                ActionKind::Reinstall => ("=", "cyan"),
                ActionKind::Remove => ("-", "red"),
            };
            let line = format!("  {} {} {}", icon, action.name, action.version.dimmed());
            match color {
                "green" => println!("{}", line.green()),
                "yellow" => println!("{}", line.yellow()),
                "cyan" => println!("{}", line.cyan()),
                _ => println!("{}", line),
            }
        }

        println!();

        if noconfirm {
            return Ok(true);
        }

        print!("{} ", "Install missing makedepends? [Y/n]".bold());
        io::stdout().flush()?;
        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        let input = input.trim().to_lowercase();
        Ok(input != "n" && input != "no")
    }

    fn on_event(&mut self, event: &Event) -> io::Result<()> {
        use zolt::Colorize;

        match event {
            Event::Download { inner } => match inner {
                DownloadEvent::Start { total_packages, .. } => {
                    println!(
                        "{} {} package(s)...",
                        "Downloading".bold().blue(),
                        total_packages
                    );
                }
                DownloadEvent::Progress {
                    package,
                    bytes_done,
                    bytes_total,
                } => {
                    if *bytes_total > 0 {
                        let pct = bytes_done * 100 / bytes_total;
                        print!("\r  {} {}%  ", package.dimmed(), pct);
                        io::stdout().flush()?;
                    }
                }
                DownloadEvent::ItemDone { package } => {
                    println!("\r  {} {:<50}", "Downloaded".green(), package);
                }
                DownloadEvent::Done => {
                    println!("{}", "Download complete.".green());
                }
            },
            Event::Install { inner } => match inner {
                InstallEvent::Start { total_packages } => {
                    println!(
                        "{} {} package(s)...",
                        "Installing".bold().blue(),
                        total_packages
                    );
                }
                InstallEvent::Action {
                    package,
                    action,
                    current,
                    total,
                    ..
                } => {
                    print!(
                        "\r  [{}/{}] {} {}  ",
                        current,
                        total,
                        action.bold(),
                        package
                    );
                    io::stdout().flush()?;
                }
                InstallEvent::ItemDone { package, .. } => {
                    println!("\r  {} {:<50}", "Installed".green(), package);
                }
                InstallEvent::Hook { name, .. } => {
                    print!("\r  {} {}  ", "Running hook".dimmed(), name.dimmed());
                    io::stdout().flush()?;
                }
                InstallEvent::Done => {
                    println!();
                }
            },
            Event::Remove { inner } => match inner {
                RemoveEvent::Start { total_packages } => {
                    println!(
                        "{} {} package(s)...",
                        "Removing".bold().blue(),
                        total_packages
                    );
                }
                RemoveEvent::Action {
                    package,
                    action,
                    current,
                    total,
                    ..
                } => {
                    print!(
                        "\r  [{}/{}] {} {}  ",
                        current,
                        total,
                        action.bold(),
                        package
                    );
                    io::stdout().flush()?;
                }
                RemoveEvent::ItemDone { package, .. } => {
                    println!("\r  {} {:<50}", "Removed".green(), package);
                }
                RemoveEvent::Done => {
                    println!();
                }
            },
        }
        Ok(())
    }

    fn tick(&mut self) -> io::Result<()> {
        Ok(())
    }

    fn finish_install(&mut self, _total_bytes: u64, _installed_count: u32) -> io::Result<()> {
        Ok(())
    }

    fn start_build(&mut self) -> io::Result<()> {
        use zolt::Colorize;
        println!();
        zolt::infoln!("Running {} stage...", koca::funcs::BUILD.bold().blue());
        Ok(())
    }

    fn on_build_line(&mut self, line: &str) -> io::Result<()> {
        println!("{}", line);
        Ok(())
    }

    fn finish_build(&mut self, _pkgname: &str, _version: &str) -> io::Result<()> {
        Ok(())
    }

    fn start_package(&mut self) -> io::Result<()> {
        Ok(())
    }

    fn on_package_line(&mut self, line: &str) -> io::Result<()> {
        println!("{}", line);
        Ok(())
    }

    fn finish_package(&mut self, output_file: &str) -> io::Result<()> {
        use zolt::Colorize;
        zolt::infoln!("Package created: {}", output_file.bold());
        Ok(())
    }

    fn show_failure(&mut self, _phase_name: &str) -> io::Result<()> {
        Ok(())
    }

    fn suspend(&mut self) -> io::Result<()> {
        Ok(())
    }

    fn resume(&mut self) -> io::Result<()> {
        Ok(())
    }

    fn cleanup(&mut self) {}
}
