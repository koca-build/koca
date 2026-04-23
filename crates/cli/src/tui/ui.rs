use crossterm::event::{self as ct_event, KeyCode, KeyModifiers};
use crossterm::{cursor, execute, terminal};
use koca::dep::DepConstraint;
use koca_proto::{DownloadEvent, Event, InstallEvent, PlannedAction};
use std::io::{self, Write};
use std::time::Duration;

use super::render::{self, confirm_info_lines, RenderState};
use super::viewport::DynViewport;
use super::{BuildState, CreateUi, DownloadState, InstallSummary, Phase};

// ── Ground-truth termios save/restore ────────────────────────────────────
//
// crossterm's `enable_raw_mode()` / `disable_raw_mode()` maintain a global
// `TERMINAL_MODE_PRIOR_RAW_MODE` static: the first `enable_raw_mode()` call
// snapshots the termios, and `disable_raw_mode()` restores that snapshot.
// The snapshot is cleared on restore, so a subsequent `enable_raw_mode()` will
// re-snapshot whatever the terminal looks like at *that* moment.
//
// During koca's lifecycle, the TUI suspends raw mode (for the Y/n confirm
// prompt and for the sudo password prompt) and lazily re-enters it ~80 ms
// later on the next tick.  Between the suspend and the re-enter, a privileged
// backend process (spawned via sudo) is running pacman/apt hooks that share
// the same controlling terminal.  If any hook or subprocess calls `tcsetattr`
// and clears OPOST on the shared tty device before our next
// `enable_raw_mode()` re-snapshot, crossterm bakes that broken state in as
// the "original" and faithfully restores it on exit — leaving the user's
// shell without `\n` → `\r\n` translation (indented PS1, invisible typing).
//
// Saving the termios ourselves at construction — before crossterm or any
// subprocess touches the terminal — gives us a ground-truth snapshot that is
// immune to this race.  We restore it in `cleanup()` as the very last step,
// after crossterm has done its own restore attempt.

#[cfg(unix)]
struct SavedTermios(Option<nix::sys::termios::Termios>);

#[cfg(unix)]
impl SavedTermios {
    /// Capture the current termios from stdin (fd 0), which is the same fd
    /// crossterm uses when stdin is a tty.
    fn capture() -> Self {
        use nix::sys::termios::tcgetattr;
        use std::os::fd::BorrowedFd;

        let fd = unsafe { BorrowedFd::borrow_raw(0) };
        Self(tcgetattr(fd).ok())
    }

    /// Restore the captured termios.  No-op if the capture failed or if this
    /// is a non-tty environment.
    fn restore(&self) {
        use nix::sys::termios::{tcsetattr, SetArg};
        use std::os::fd::BorrowedFd;

        if let Some(ref t) = self.0 {
            let fd = unsafe { BorrowedFd::borrow_raw(0) };
            let _ = tcsetattr(fd, SetArg::TCSANOW, t);
        }
    }
}

pub struct KocaCreateTui {
    phase: Phase,
    vp: Option<DynViewport>,
    /// Known cursor row from suspend — used to recreate the viewport without
    /// a DSR query (which flashes escape codes).
    resume_row: Option<u16>,
    info: Vec<ratatui::text::Line<'static>>,
    dl_state: DownloadState,
    install_summary: Option<InstallSummary>,
    build_state: BuildState,
    pkg_state: BuildState,
    build_summary: Option<String>,
    pkg_summary: Option<String>,
    tick: usize,
    /// Ground-truth termios captured before crossterm touches the terminal.
    /// See module-level comment for rationale.
    #[cfg(unix)]
    original_termios: SavedTermios,
}

impl KocaCreateTui {
    pub fn new() -> io::Result<Self> {
        Ok(Self {
            phase: Phase::Resolve,
            vp: None,
            resume_row: None,
            info: Vec::new(),
            dl_state: DownloadState::new(),
            install_summary: None,
            build_state: BuildState::new(),
            pkg_state: BuildState::new(),
            build_summary: None,
            pkg_summary: None,
            tick: 0,
            #[cfg(unix)]
            original_termios: SavedTermios::capture(),
        })
    }

    fn redraw(&mut self) -> io::Result<()> {
        // After confirm, the viewport is dropped and info cleared. Don't
        // create a new viewport until a real phase transition happens.
        if self.vp.is_none() && self.phase == Phase::Confirm {
            return Ok(());
        }

        let state = RenderState {
            phase: self.phase,
            info: &self.info,
            dl_state: &self.dl_state,
            install_summary: self.install_summary.as_ref(),
            build_state: &self.build_state,
            pkg_state: &self.pkg_state,
            build_summary: self.build_summary.as_deref(),
            pkg_summary: self.pkg_summary.as_deref(),
            tick: self.tick,
        };

        let height = render::calc_height(&state);

        if self.vp.is_none() {
            self.vp = Some(if let Some(row) = self.resume_row.take() {
                DynViewport::at_row(height, row)?
            } else {
                DynViewport::new(height)?
            });
        }

        self.vp.as_mut().unwrap().draw(height, |f| {
            render::render(f, &state);
        })?;
        Ok(())
    }
}

impl CreateUi for KocaCreateTui {
    fn start_resolve(&mut self) -> io::Result<()> {
        self.phase = Phase::Resolve;
        self.redraw()
    }

    fn show_confirm(
        &mut self,
        actions: &[PlannedAction],
        depends: &[DepConstraint],
        noconfirm: bool,
    ) -> io::Result<bool> {
        self.info = confirm_info_lines(actions, depends);
        self.phase = Phase::Confirm;
        self.redraw()?;

        if noconfirm {
            return Ok(true);
        }

        // Suspend TUI so stdin echoes and Ctrl+C works for Y/n input.
        // Keep cursor where ratatui left it (after "Continue? [Y/n] ").
        self.vp.as_mut().unwrap().suspend(true)?;
        io::stdout().flush()?;

        let mut input = String::new();
        io::stdin().read_line(&mut input)?;

        // The confirm info is now visible on the terminal. Clear it from state
        // so it isn't duplicated when a new viewport is created for later phases.
        self.info.clear();

        let input = input.trim().to_lowercase();
        Ok(input != "n" && input != "no")
    }

    fn on_event(&mut self, event: &Event) -> io::Result<()> {
        match event {
            Event::Download { inner } => match inner {
                DownloadEvent::Start {
                    total_bytes,
                    total_packages,
                } => {
                    self.phase = Phase::Download;
                    self.dl_state.total_bytes = *total_bytes;
                    self.dl_state.total_packages = *total_packages;
                }
                DownloadEvent::Progress {
                    bytes_done,
                    bytes_total,
                    active,
                } => {
                    self.dl_state.done_bytes = *bytes_done;
                    self.dl_state.total_bytes = self.dl_state.total_bytes.max(*bytes_total);
                    self.dl_state.active_names = active.clone();
                }
                DownloadEvent::ItemDone { package } => {
                    self.dl_state.done_count += 1;
                    self.dl_state.active_names.retain(|n| n != package);
                }
                DownloadEvent::Done => {
                    self.dl_state.done_bytes = self.dl_state.total_bytes;
                    self.dl_state.done_count = self.dl_state.total_packages;
                    self.dl_state.active_names.clear();
                }
            },
            Event::Install { inner } => match inner {
                InstallEvent::Start { total_packages } => {
                    self.phase = Phase::Install;
                    self.dl_state.install_total = *total_packages;
                }
                InstallEvent::Action {
                    package,
                    current,
                    total,
                    ..
                } => {
                    self.dl_state.current_install_pkg = Some(package.clone());
                    self.dl_state.install_current = *current;
                    self.dl_state.install_total = *total;
                }
                InstallEvent::ItemDone { .. } => {}
                InstallEvent::Hook { .. } => {}
                InstallEvent::Done => {}
            },
            Event::Remove { .. } => {}
        }
        self.redraw()
    }

    fn tick(&mut self) -> io::Result<()> {
        // In raw mode SIGINT is suppressed, so check for Ctrl+C as a key event.
        if ct_event::poll(Duration::ZERO)? {
            if let ct_event::Event::Key(key) = ct_event::read()? {
                if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
                    self.cleanup();
                    std::process::exit(130);
                }
            }
        }
        self.tick += 1;
        self.dl_state.tick += 1;
        self.redraw()
    }

    fn finish_install(&mut self, total_bytes: u64, installed_count: u32) -> io::Result<()> {
        self.install_summary = Some(InstallSummary {
            total_bytes,
            installed_count,
        });
        Ok(())
    }

    fn start_build(&mut self) -> io::Result<()> {
        self.phase = Phase::Build;
        self.build_state = BuildState::new();
        self.redraw()
    }

    fn on_build_line(&mut self, line: &str) -> io::Result<()> {
        self.build_state.push_line(line.to_string());
        self.tick += 1;
        self.redraw()
    }

    fn finish_build(&mut self, pkgname: &str, version: &str) -> io::Result<()> {
        self.build_summary = Some(format!("{} {}", pkgname, version));
        Ok(())
    }

    fn start_package(&mut self) -> io::Result<()> {
        self.phase = Phase::Package;
        self.pkg_state = BuildState::new();
        self.redraw()
    }

    fn on_package_line(&mut self, line: &str) -> io::Result<()> {
        self.pkg_state.push_line(line.to_string());
        self.tick += 1;
        self.redraw()
    }

    fn finish_package(&mut self, output_file: &str) -> io::Result<()> {
        self.pkg_summary = Some(output_file.to_string());
        self.phase = Phase::Done;
        self.redraw()
    }

    fn show_failure(&mut self, _phase_name: &str) -> io::Result<()> {
        self.phase = Phase::Failed;
        self.redraw()
    }

    fn suspend(&mut self) -> io::Result<()> {
        if let Some(vp) = self.vp.as_mut() {
            // Save the cursor row before suspending — suspend(false) moves
            // the cursor to top_y + current_vh.
            self.resume_row = Some(vp.cursor_row_after_suspend());
            vp.suspend(false)?;
        }
        Ok(())
    }

    fn resume(&mut self) -> io::Result<()> {
        // Drop the old viewport — its position is stale after external I/O
        // (sudo, user input) may have printed lines. The next redraw() will
        // lazily create a fresh viewport at the saved cursor row.
        self.vp = None;
        Ok(())
    }

    fn cleanup(&mut self) {
        if let Some(mut vp) = self.vp.take() {
            vp.cleanup().ok();
        }
        let _ = terminal::disable_raw_mode();
        let _ = execute!(io::stdout(), cursor::Show);
        // Restore the ground-truth termios we captured at construction time.
        // This is the authoritative restore — see the module-level comment on
        // SavedTermios for why crossterm's own restore is insufficient.
        #[cfg(unix)]
        self.original_termios.restore();
    }
}
