use std::io::{self, Write};

use crossterm::{cursor, execute, terminal};
use koca::dep::DepConstraint;
use koca_proto::{ActionKind, DownloadEvent, Event, InstallEvent, PlannedAction, RemoveEvent};
use zolt::Colorize;

use super::{CreateUi, BUILD_GUTTER_MAX, SPINNERS};

/// Clear the current line and move cursor to column 0.
fn clear_line() -> io::Result<()> {
    execute!(
        io::stdout(),
        cursor::MoveToColumn(0),
        terminal::Clear(terminal::ClearType::CurrentLine)
    )
}

fn format_bytes(b: u64) -> String {
    if b >= 1_000_000_000 {
        format!("{:.1} GB", b as f64 / 1_000_000_000.0)
    } else if b >= 1_000_000 {
        format!("{:.1} MB", b as f64 / 1_000_000.0)
    } else if b >= 1_000 {
        format!("{:.0} KB", b as f64 / 1_000.0)
    } else {
        format!("{} B", b)
    }
}

// ── Column layout (à la `apt` Output-Version >= 30) ─────────────────────

/// Display a list of colored strings in columns, like `ls` / apt's ShowWithColumns.
/// `plain` is used for width calculations, `colored` for actual display.
/// Column-major order, 2-space indent, 2-space gap between columns.
fn show_with_columns_colored(plain: &[String], colored: &[String], indent: usize) {
    show_with_columns_inner(plain, Some(colored), indent);
}

fn show_with_columns_inner(items: &[String], colored: Option<&[String]>, indent: usize) {
    if items.is_empty() {
        return;
    }
    let screen_width = terminal::size().map(|(w, _)| w as usize).unwrap_or(80);
    let usable = screen_width.saturating_sub(indent);
    let col_gap = 2;

    // Find the max number of columns that fits.
    let mut num_cols = 1;
    for try_cols in (2..=items.len()).rev() {
        let num_rows = (items.len() + try_cols - 1) / try_cols;
        // Compute column widths (column-major).
        let mut total = 0;
        let mut fits = true;
        for col in 0..try_cols {
            let col_width = (0..num_rows)
                .filter_map(|row| items.get(col * num_rows + row).map(|s| s.len()))
                .max()
                .unwrap_or(0);
            total += col_width;
            if col + 1 < try_cols {
                total += col_gap;
            }
            if total > usable {
                fits = false;
                break;
            }
        }
        if fits {
            num_cols = try_cols;
            break;
        }
    }

    let num_rows = (items.len() + num_cols - 1) / num_cols;

    // Compute per-column widths.
    let mut col_widths = Vec::with_capacity(num_cols);
    for col in 0..num_cols {
        let w = (0..num_rows)
            .filter_map(|row| items.get(col * num_rows + row).map(|s| s.len()))
            .max()
            .unwrap_or(0);
        col_widths.push(w);
    }

    // Print column-major.
    for row in 0..num_rows {
        print!("{}", " ".repeat(indent));
        for col in 0..num_cols {
            let idx = col * num_rows + row;
            if let Some(item) = items.get(idx) {
                let display = colored
                    .and_then(|c| c.get(idx))
                    .unwrap_or(item);
                print!("{}", display);
                if col + 1 < num_cols {
                    // Pad based on plain width, not colored width.
                    let pad = col_widths[col].saturating_sub(item.len()) + col_gap;
                    print!("{}", " ".repeat(pad));
                }
            }
        }
        println!();
    }
}

/// Tracks build/package output lines for the scrolling gutter.
struct BuildState {
    lines: Vec<String>,
    /// How many gutter lines we last wrote (so we know how far to move up).
    drawn_lines: u16,
}

impl BuildState {
    fn new() -> Self {
        Self {
            lines: Vec::new(),
            drawn_lines: 0,
        }
    }

    fn push(&mut self, line: String) {
        self.lines.push(line);
    }

    /// Redraw the gutter: move up over previous output, clear, rewrite last N lines.
    fn redraw(&mut self, header: &str, tick: usize) -> io::Result<()> {
        let mut out = io::stdout();

        if self.drawn_lines > 0 {
            execute!(out, cursor::MoveUp(self.drawn_lines))?;
        }

        let spinner = SPINNERS[tick % SPINNERS.len()];
        clear_line()?;
        writeln!(out, "{} {}", spinner.blue(), header.bold())?;

        let start = self.lines.len().saturating_sub(BUILD_GUTTER_MAX);
        let visible = &self.lines[start..];
        let mut drawn: u16 = 1;

        for line in visible {
            clear_line()?;
            let width = terminal::size().map(|(w, _)| w as usize).unwrap_or(80);
            let avail = width.saturating_sub(4); // "  │ " prefix
            let truncated = if line.len() > avail { &line[..avail] } else { line.as_str() };
            writeln!(out, "  {} {}", "│".dimmed(), truncated.dimmed())?;
            drawn += 1;
        }

        self.drawn_lines = drawn;
        out.flush()
    }

    fn finish(&mut self, summary: &str) -> io::Result<()> {
        let mut out = io::stdout();
        if self.drawn_lines > 0 {
            execute!(out, cursor::MoveUp(self.drawn_lines))?;
            for _ in 0..self.drawn_lines {
                clear_line()?;
                writeln!(out)?;
            }
            execute!(out, cursor::MoveUp(self.drawn_lines))?;
        }
        clear_line()?;
        writeln!(out, "{}", summary)?;
        self.drawn_lines = 0;
        out.flush()
    }

    /// On failure: clear the gutter, then dump all captured output.
    fn finish_with_output(&mut self, header: &str) -> io::Result<()> {
        let mut out = io::stdout();
        if self.drawn_lines > 0 {
            execute!(out, cursor::MoveUp(self.drawn_lines))?;
            for _ in 0..self.drawn_lines {
                clear_line()?;
                writeln!(out)?;
            }
            execute!(out, cursor::MoveUp(self.drawn_lines))?;
        }
        clear_line()?;
        writeln!(out, "{}", header)?;
        for line in &self.lines {
            writeln!(out, "  {} {}", "│".dimmed(), line)?;
        }
        self.drawn_lines = 0;
        out.flush()
    }
}

pub struct KocaCreateUi {
    tick: usize,
    build_state: BuildState,
    pkg_state: BuildState,
    dl_bytes_done: u64,
    dl_total_bytes: u64,
    dl_percent: Option<u32>,
    dl_total_pkgs: u32,
    dl_done_pkgs: u32,
    dl_active: Vec<String>,
    dl_lines_drawn: u16,
    /// Resolve spinner: true while resolving deps.
    resolving: bool,
    resolve_lines_drawn: u16,
    /// Install/remove inline progress (collapsed on Done, like downloads).
    inst_total: u32,
    /// Number of fully completed packages (got ItemDone).
    inst_done: u32,
    /// Number of packages that have started but aren't done yet.
    inst_in_progress: u32,
    inst_active: Vec<String>,
    inst_is_remove: bool,
    inst_lines_drawn: u16,
}

impl KocaCreateUi {
    pub fn new() -> io::Result<Self> {
        Ok(Self {
            tick: 0,
            build_state: BuildState::new(),
            pkg_state: BuildState::new(),
            dl_bytes_done: 0,
            dl_total_bytes: 0,
            dl_percent: None,
            dl_total_pkgs: 0,
            dl_done_pkgs: 0,
            dl_active: Vec::new(),
            dl_lines_drawn: 0,
            resolving: false,
            resolve_lines_drawn: 0,
            inst_total: 0,
            inst_done: 0,
            inst_in_progress: 0,
            inst_active: Vec::new(),
            inst_is_remove: false,
            inst_lines_drawn: 0,
        })
    }

    fn redraw_download(&mut self) -> io::Result<()> {
        let total = self.dl_total_bytes;
        let pct = if total > 0 {
            (self.dl_bytes_done as f64 / total as f64 * 100.0) as u32
        } else {
            self.dl_percent.unwrap_or(0)
        };
        let bar_width = 30usize;
        let filled = (bar_width as f64 * pct as f64 / 100.0) as usize;
        let spinner = SPINNERS[self.tick % SPINNERS.len()];
        let mut out = io::stdout();

        if self.dl_lines_drawn > 0 {
            execute!(out, cursor::MoveUp(self.dl_lines_drawn))?;
        }

        clear_line()?;
        if total > 0 {
            writeln!(
                out,
                "Downloading {}/{} packages {}{} {}% ({}/{})",
                self.dl_done_pkgs,
                self.dl_total_pkgs,
                "█".repeat(filled).green(),
                "░".repeat(bar_width.saturating_sub(filled)).dimmed(),
                pct,
                format_bytes(self.dl_bytes_done),
                format_bytes(total),
            )?;
        } else {
            writeln!(
                out,
                "Downloading {}{} {}%",
                "█".repeat(filled).green(),
                "░".repeat(bar_width.saturating_sub(filled)).dimmed(),
                pct,
            )?;
        }

        clear_line()?;
        if !self.dl_active.is_empty() {
            let label = if self.dl_active.len() == 1 { "download" } else { "downloads" };
            writeln!(
                out,
                "{} active ({} {}): {}",
                spinner.blue(),
                self.dl_active.len(),
                label,
                self.dl_active.join(", ").dimmed()
            )?;
        } else {
            writeln!(out, "{} waiting for package downloads...", spinner.blue())?;
        }

        self.dl_lines_drawn = 2;
        out.flush()
    }

    fn start_download_ui(&mut self, total_packages: u32, total_bytes: u64) -> io::Result<()> {
        self.dl_bytes_done = 0;
        self.dl_total_bytes = total_bytes;
        self.dl_percent = Some(0);
        self.dl_total_pkgs = total_packages;
        self.dl_done_pkgs = 0;
        self.dl_active.clear();
        self.redraw_download()
    }

    fn redraw_resolve(&mut self) -> io::Result<()> {
        let mut out = io::stdout();
        if self.resolve_lines_drawn > 0 {
            execute!(out, cursor::MoveUp(self.resolve_lines_drawn))?;
        }
        let spinner = SPINNERS[self.tick % SPINNERS.len()];
        clear_line()?;
        writeln!(out, "{} {}", spinner.blue(), "Resolving dependencies...".bold())?;
        self.resolve_lines_drawn = 1;
        out.flush()
    }

    fn redraw_install(&mut self) -> io::Result<()> {
        let mut out = io::stdout();
        if self.inst_lines_drawn > 0 {
            execute!(out, cursor::MoveUp(self.inst_lines_drawn))?;
        }

        let label = if self.inst_is_remove { "Removing" } else { "Installing" };
        let spinner = SPINNERS[self.tick % SPINNERS.len()];
        let width = terminal::size().map(|(w, _)| w as usize).unwrap_or(80);

        // Progress: in_progress counts as half done (started but not finished).
        // total_steps = total * 2, done_steps = done * 2 + in_progress * 1.
        let total_steps = self.inst_total as u64 * 2;
        let done_steps = self.inst_done as u64 * 2 + self.inst_in_progress as u64;
        let pct = if total_steps > 0 {
            ((done_steps as f64 / total_steps as f64 * 100.0) as u32).min(100)
        } else {
            0
        };
        let bar_width = 30usize;
        let filled = ((bar_width as f64 * pct as f64 / 100.0) as usize).min(bar_width);

        let done_display = self.inst_done.min(self.inst_total);

        clear_line()?;
        writeln!(
            out,
            "{} {}/{} packages {}{} {}%",
            label,
            done_display,
            self.inst_total,
            "█".repeat(filled).green(),
            "░".repeat(bar_width.saturating_sub(filled)).dimmed(),
            pct,
        )?;

        clear_line()?;
        if !self.inst_active.is_empty() {
            let prefix = format!("{} ", spinner);
            let avail = width.saturating_sub(prefix.len());
            let pkgs = self.inst_active.join(", ");
            let display = if pkgs.len() > avail && avail > 3 {
                format!("{}...", &pkgs[..avail - 3])
            } else {
                pkgs
            };
            writeln!(out, "{}{}", prefix.blue(), display.dimmed())?;
        } else {
            writeln!(out)?;
        }
        self.inst_lines_drawn = 2;
        out.flush()
    }

    fn finish_install_ui(&mut self) -> io::Result<()> {
        let mut out = io::stdout();
        if self.inst_lines_drawn > 0 {
            execute!(out, cursor::MoveUp(self.inst_lines_drawn))?;
            for _ in 0..self.inst_lines_drawn {
                clear_line()?;
                writeln!(out)?;
            }
            execute!(out, cursor::MoveUp(self.inst_lines_drawn))?;
        }
        clear_line()?;
        let label = if self.inst_is_remove { "Removed" } else { "Installed" };
        let count = if self.inst_done > 0 { self.inst_done } else { self.inst_total };
        println!("{} {} package(s)", label.green(), count);
        self.inst_lines_drawn = 0;
        Ok(())
    }
}

impl CreateUi for KocaCreateUi {
    fn start_resolve(&mut self) -> io::Result<()> {
        self.resolving = true;
        self.redraw_resolve()
    }

    fn finish_resolve(&mut self) -> io::Result<()> {
        self.resolving = false;
        if self.resolve_lines_drawn > 0 {
            let mut out = io::stdout();
            execute!(out, cursor::MoveUp(self.resolve_lines_drawn))?;
            for _ in 0..self.resolve_lines_drawn {
                clear_line()?;
                writeln!(out)?;
            }
            execute!(out, cursor::MoveUp(self.resolve_lines_drawn))?;
            self.resolve_lines_drawn = 0;
        }
        Ok(())
    }

    fn show_confirm(
        &mut self,
        actions: &[PlannedAction],
        _depends: &[DepConstraint],
        noconfirm: bool,
    ) -> io::Result<bool> {
        // Group by action type.
        let installs: Vec<&PlannedAction> = actions.iter().filter(|a| a.action == ActionKind::Install).collect();
        let upgrades: Vec<&PlannedAction> = actions.iter().filter(|a| a.action == ActionKind::Upgrade).collect();
        let downgrades: Vec<&PlannedAction> = actions.iter().filter(|a| a.action == ActionKind::Downgrade).collect();
        let reinstalls: Vec<&PlannedAction> = actions.iter().filter(|a| a.action == ActionKind::Reinstall).collect();
        let removes: Vec<&PlannedAction> = actions.iter().filter(|a| a.action == ActionKind::Remove).collect();

        let show_group = |label: &str, pkgs: &[&PlannedAction]| {
            if pkgs.is_empty() {
                return;
            }
            println!("{}:", label.bold());
            let names: Vec<String> = pkgs.iter().map(|a| a.name.clone()).collect();
            show_with_columns_inner(&names, None, 2);
            println!();
        };

        show_group("Installing", &installs);
        show_group("Upgrading", &upgrades);
        show_group("Downgrading", &downgrades);
        show_group("Reinstalling", &reinstalls);
        show_group("Removing", &removes);

        // Summary.
        let mut summary = Vec::new();
        if !installs.is_empty() { summary.push(format!("{} to install", installs.len())); }
        if !upgrades.is_empty() { summary.push(format!("{} to upgrade", upgrades.len())); }
        if !downgrades.is_empty() { summary.push(format!("{} to downgrade", downgrades.len())); }
        if !reinstalls.is_empty() { summary.push(format!("{} to reinstall", reinstalls.len())); }
        if !removes.is_empty() { summary.push(format!("{} to remove", removes.len())); }
        if !summary.is_empty() {
            println!("{}: {}", "Summary".bold(), summary.join(", "));
        }

        // Sizes.
        let total_dl: u64 = actions.iter().map(|a| a.download_size).sum();
        let total_inst: u64 = actions.iter().map(|a| a.install_size).sum();
        if total_dl > 0 {
            println!("  Download size: {}", format_bytes(total_dl).bold());
        }
        if total_inst > 0 {
            println!("  Install size:  {}", format_bytes(total_inst).bold());
        }
        println!();

        if noconfirm {
            return Ok(true);
        }

        print!("{} ", "Continue? [Y/n]".bold());
        io::stdout().flush()?;
        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        let input = input.trim().to_lowercase();
        let accepted = input != "n" && input != "no";

        execute!(io::stdout(), cursor::MoveUp(1))?;
        clear_line()?;
        if accepted {
            let total_packages = actions
                .iter()
                .filter(|a| a.action != ActionKind::Remove)
                .count() as u32;
            self.start_download_ui(total_packages, total_dl)?;
        } else {
            println!("{}", "Cancelled.".dimmed());
        }

        Ok(accepted)
    }

    fn on_event(&mut self, event: &Event) -> io::Result<()> {
        match event {
            Event::Download { inner } => match inner {
                DownloadEvent::Start {
                    total_packages,
                    total_bytes,
                } => {
                    self.start_download_ui(*total_packages, *total_bytes)?;
                }
                DownloadEvent::Progress {
                    bytes_done,
                    bytes_total,
                    percent,
                    active,
                } => {
                    self.dl_bytes_done = *bytes_done;
                    if *bytes_total > 0 {
                        self.dl_total_bytes = *bytes_total;
                    }
                    self.dl_percent = *percent;
                    self.dl_active = active.clone();
                    self.redraw_download()?;
                }
                DownloadEvent::ItemDone { package: _ } => {
                    self.dl_done_pkgs += 1;
                    self.redraw_download()?;
                }
                DownloadEvent::Done => {
                    if self.dl_lines_drawn > 0 {
                        let mut out = io::stdout();
                        execute!(out, cursor::MoveUp(self.dl_lines_drawn))?;
                        for _ in 0..self.dl_lines_drawn {
                            clear_line()?;
                            writeln!(out)?;
                        }
                        execute!(out, cursor::MoveUp(self.dl_lines_drawn))?;
                    }
                    clear_line()?;
                    if self.dl_total_bytes > 0 {
                        println!(
                            "Downloaded {} packages ({})",
                            self.dl_total_pkgs,
                            format_bytes(self.dl_total_bytes).bold()
                        );
                    }
                    self.dl_lines_drawn = 0;
                    self.dl_percent = None;
                }
            },
            Event::Install { inner } => match inner {
                InstallEvent::Start { total_packages } => {
                    self.inst_total = *total_packages;
                    self.inst_done = 0;
                    self.inst_in_progress = 0;
                    self.inst_active.clear();
                    self.inst_is_remove = false;
                    self.inst_lines_drawn = 0;
                    self.redraw_install()?;
                }
                InstallEvent::Action {
                    package,
                    ..
                } => {
                    if !self.inst_active.iter().any(|n| n == package) {
                        self.inst_active.push(package.clone());
                        self.inst_in_progress += 1;
                    }
                    self.redraw_install()?;
                }
                InstallEvent::ItemDone { package, .. } => {
                    if self.inst_active.iter().any(|n| n == package) {
                        self.inst_active.retain(|n| n != package);
                        self.inst_in_progress = self.inst_in_progress.saturating_sub(1);
                    }
                    self.inst_done += 1;
                    self.redraw_install()?;
                }
                InstallEvent::Hook { name, .. } => {
                    if !self.inst_active.iter().any(|n| n == name) {
                        self.inst_active.push(name.clone());
                    }
                    self.redraw_install()?;
                }
                InstallEvent::Done => {
                    self.finish_install_ui()?;
                }
            },
            Event::Remove { inner } => match inner {
                RemoveEvent::Start { total_packages } => {
                    self.inst_total = *total_packages;
                    self.inst_done = 0;
                    self.inst_in_progress = 0;
                    self.inst_active.clear();
                    self.inst_is_remove = true;
                    self.inst_lines_drawn = 0;
                    self.redraw_install()?;
                }
                RemoveEvent::Action {
                    package,
                    ..
                } => {
                    if !self.inst_active.iter().any(|n| n == package) {
                        self.inst_active.push(package.clone());
                        self.inst_in_progress += 1;
                    }
                    self.redraw_install()?;
                }
                RemoveEvent::ItemDone { package, .. } => {
                    if self.inst_active.iter().any(|n| n == package) {
                        self.inst_active.retain(|n| n != package);
                        self.inst_in_progress = self.inst_in_progress.saturating_sub(1);
                    }
                    self.inst_done += 1;
                    self.redraw_install()?;
                }
                RemoveEvent::Done => {
                    self.finish_install_ui()?;
                }
            },
        }
        Ok(())
    }

    fn tick(&mut self) -> io::Result<()> {
        self.tick += 1;
        if self.resolving {
            self.redraw_resolve()?;
        }
        if self.dl_lines_drawn > 0 {
            self.redraw_download()?;
        }
        if self.inst_lines_drawn > 0 {
            self.redraw_install()?;
        }
        if self.build_state.drawn_lines > 0 {
            self.build_state.redraw("Building...", self.tick)?;
        }
        if self.pkg_state.drawn_lines > 0 {
            self.pkg_state.redraw("Packaging...", self.tick)?;
        }
        Ok(())
    }

    fn finish_install(&mut self, _total_bytes: u64, _installed_count: u32) -> io::Result<()> {
        if self.inst_lines_drawn > 0 {
            self.finish_install_ui()?;
        }
        println!();
        Ok(())
    }

    fn start_build(&mut self) -> io::Result<()> {
        self.build_state = BuildState::new();
        self.build_state.redraw("Building...", self.tick)
    }

    fn on_build_line(&mut self, line: &str) -> io::Result<()> {
        self.build_state.push(line.to_string());
        self.build_state.redraw("Building...", self.tick)
    }

    fn finish_build(&mut self, pkgname: &str, version: &str) -> io::Result<()> {
        self.build_state
            .finish(&format!("{} {} {}", "Built".green(), pkgname.bold(), version.dimmed()))
    }

    fn start_package(&mut self) -> io::Result<()> {
        self.pkg_state = BuildState::new();
        self.pkg_state.redraw("Packaging...", self.tick)
    }

    fn on_package_line(&mut self, line: &str) -> io::Result<()> {
        self.pkg_state.push(line.to_string());
        self.pkg_state.redraw("Packaging...", self.tick)
    }

    fn finish_package(&mut self, output_file: &str) -> io::Result<()> {
        self.pkg_state
            .finish(&format!("{} {}", "Package created:".green(), output_file.bold()))
    }

    fn show_failure(&mut self, phase_name: &str) -> io::Result<()> {
        if self.build_state.drawn_lines > 0 {
            self.build_state.finish_with_output(&format!("{} {}", phase_name.red(), "failed".red()))?;
        }
        if self.pkg_state.drawn_lines > 0 {
            self.pkg_state.finish_with_output(&format!("{} {}", phase_name.red(), "failed".red()))?;
        }
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
