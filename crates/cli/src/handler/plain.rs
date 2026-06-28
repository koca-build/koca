//! Non-TTY handlers: stream every line with a front-loaded gutter, no cursor
//! tricks, no spinner. One struct per library trait.

use std::collections::HashMap;
use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use koca::backend::{ActionKind, DependencyEvent, DownloadEvent, InstallEvent, RemoveEvent};
use koca::distro::Distro;
use koca::handler::{
    BuildHandler, DependencyHandler, ElevateCommandSpec, ElevatedChild, SourceHandler,
};
use koca::source::{format_bytes, Source, SourceProgress};
use koca::{BuildFile, BuildOutputLine, BuildOutputStream, PackageManager, Plan};
use zolt::Colorize;

use super::{elevate_command, TokioElevatedChild};
use crate::cli::CreateArgs;
use crate::error::{CliError, CliMultiError, CliMultiResult};

// ── DependencyHandler ─────────────────────────────────────────────────────

/// Holds the phase counts the library reports up front (from the resolved plan),
/// so per-item lines can show `(current/total)`. `downloaded` is the handler's
/// own counter since download `ItemDone` events carry no index.
#[derive(Default)]
pub struct PlainDependencyHandler {
    downloads: u32,
    installs: u32,
    downloaded: u32,
}

#[async_trait]
impl DependencyHandler for PlainDependencyHandler {
    fn on_resolve_start(&mut self) {
        zolt::infoln!("Resolving dependencies...");
    }

    fn on_install_start(&mut self, downloads: u32, installs: u32) {
        self.downloads = downloads;
        self.installs = installs;
        self.downloaded = 0;
        if downloads > 0 {
            zolt::infoln!("Downloading {downloads} package(s)...");
        }
        zolt::infoln!("Installing {installs} package(s)...");
    }

    fn on_remove_start(&mut self, removes: u32) {
        self.installs = removes;
        zolt::infoln!("Removing {removes} package(s)...");
    }

    fn on_dep_event(&mut self, event: &DependencyEvent) {
        match event {
            DependencyEvent::Download {
                inner: DownloadEvent::ItemDone { package },
            } => {
                self.downloaded += 1;
                println!("  downloaded {package} ({}/{})", self.downloaded, self.downloads);
            }
            DependencyEvent::Install {
                inner: InstallEvent::ItemDone { package, current },
            } => println!("  installed {package} ({current}/{})", self.installs),
            DependencyEvent::Remove {
                inner: RemoveEvent::ItemDone { package, current },
            } => println!("  removed {package} ({current}/{})", self.installs),
            _ => {}
        }
    }

    async fn elevate(&mut self, spec: ElevateCommandSpec) -> io::Result<Box<dyn ElevatedChild>> {
        let child = elevate_command(&spec)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        Ok(Box::new(TokioElevatedChild(child)))
    }
}

// ── SourceHandler ─────────────────────────────────────────────────────────

/// Per-source download progress, throttled to ~1s per source so a piped log
/// stays readable.
#[derive(Default)]
pub struct PlainSourceHandler {
    last_emit: HashMap<String, Instant>,
}

impl SourceHandler for PlainSourceHandler {
    fn on_sources_start(&mut self, sources: &[Source]) {
        zolt::infoln!("Fetching {} source(s)...", sources.len());
    }

    fn on_source_progress(&mut self, source: &Source, progress: &SourceProgress) {
        let key = source.display_url();
        let now = Instant::now();
        let due = self
            .last_emit
            .get(&key)
            .is_none_or(|t| now.duration_since(*t) >= Duration::from_secs(1));
        if !due {
            return;
        }
        self.last_emit.insert(key.clone(), now);
        match progress.fraction() {
            Some(frac) => println!("  {key}: {:.0}%", frac * 100.0),
            None => println!("  {key}: fetching..."),
        }
    }

    fn on_source_done(&mut self, source: &Source) {
        println!("  {} {}", "fetched".green(), source.display_url());
    }

    fn on_source_error(&mut self, source: &Source, error: &str) {
        zolt::errln!("  failed {}: {error}", source.display_url());
    }
}

// ── BuildHandler ──────────────────────────────────────────────────────────

#[derive(Default)]
pub struct PlainBuildHandler;

impl BuildHandler for PlainBuildHandler {
    fn on_build_start(&mut self) {
        zolt::infoln!("{}", "Building...".bold());
    }

    fn on_build_line(&mut self, line: &BuildOutputLine) {
        print_gutter_line(line);
    }

    fn on_build_failed(&mut self) {
        zolt::errln!("{}", "build failed".red());
    }

    fn on_package_start(&mut self, pkgname: &str) {
        zolt::infoln!("{}", format!("Packaging {pkgname}...").bold());
    }

    fn on_package_line(&mut self, _pkgname: &str, line: &BuildOutputLine) {
        print_gutter_line(line);
    }

    fn on_package_end(&mut self, _pkgname: &str, output_files: &[PathBuf]) {
        for file in output_files {
            println!("{} {}", "Package created:".green(), file.display());
        }
    }

    fn on_package_failed(&mut self, pkgname: &str) {
        zolt::errln!("{} {}", "package failed:".red(), pkgname);
    }
}

fn print_gutter_line(line: &BuildOutputLine) {
    let prefix = "│".dimmed();
    match line.stream {
        BuildOutputStream::Stdout => println!("  {prefix} {}", line.line),
        BuildOutputStream::Stderr => eprintln!("  {prefix} {}", line.line),
    }
}

// ── Confirmation (consumer decision, not a library trait) ──────────────────

/// Print the resolved plan and decide whether to proceed.
///
/// `--noconfirm` proceeds unconditionally. Otherwise prompt if stdin is a
/// terminal; in a non-interactive session without `--noconfirm`, abort rather
/// than silently install.
fn confirm(plan: &Plan, noconfirm: bool) -> bool {
    show_plan(plan);

    if noconfirm {
        return true;
    }
    if !io::stdin().is_terminal() {
        zolt::errln!("Refusing to install without --noconfirm in a non-interactive session.");
        return false;
    }

    print!("{} ", "Continue? [Y/n]".bold());
    let _ = io::stdout().flush();
    let mut input = String::new();
    if io::stdin().read_line(&mut input).is_err() {
        return false;
    }
    let input = input.trim().to_lowercase();
    input != "n" && input != "no"
}

fn show_plan(plan: &Plan) {
    let group = |label: &str, kind: ActionKind| {
        let names: Vec<&str> = plan
            .actions
            .iter()
            .filter(|a| a.action == kind)
            .map(|a| a.name.as_str())
            .collect();
        if !names.is_empty() {
            println!("{}: {}", label.bold(), names.join(" "));
        }
    };
    group("Installing", ActionKind::Install);
    group("Upgrading", ActionKind::Upgrade);
    group("Downgrading", ActionKind::Downgrade);
    group("Reinstalling", ActionKind::Reinstall);
    group("Removing", ActionKind::Remove);

    if plan.total_download > 0 {
        println!("  Download size: {}", format_bytes(plan.total_download).bold());
    }
    if plan.total_install > 0 {
        println!("  Install size:  {}", format_bytes(plan.total_install).bold());
    }
}

// ── Driver ─────────────────────────────────────────────────────────────────

/// Run a `create` end-to-end with plain (non-TTY) handlers.
pub async fn run(
    args: &CreateArgs,
    build_file_path: &Path,
    mut build_file: BuildFile,
    distro: &Distro,
) -> CliMultiResult<()> {
    let ke = |e: koca::KocaError| -> CliMultiError { CliError::Koca { err: e }.into() };

    let mut pm = PackageManager::for_distro(distro);
    let mut deps = PlainDependencyHandler::default();

    // Resolve → (decision) confirm → install.
    let plan = pm.resolve(build_file.all_deps(), &mut deps).await.map_err(ke)?;
    if !plan.is_empty() {
        if !confirm(&plan, args.noconfirm) {
            return Ok(());
        }
        pm.install(&plan, &mut deps).await.map_err(ke)?;
    }

    // Sources.
    let arch = build_file.arch()[0].clone();
    let srcdir = Path::new("koca-build/src");
    let mut sources = PlainSourceHandler::default();
    let results = build_file.fetch_sources(&arch, srcdir, &mut sources).await;
    let failures = results.iter().filter(|r| r.is_err()).count();
    if failures > 0 {
        return Err(ke(koca::KocaError::InvalidSource(format!(
            "{failures} source(s) failed to fetch"
        ))));
    }

    // Build + package.
    let mut build = PlainBuildHandler;
    if build_file.has_build() {
        build_file.run_build(&mut build).await.map_err(ke)?;
    }

    let formats = args.output_type.bundle_formats();
    let out_dir = Path::new("koca-out");
    let pkgnames = build_file.pkgnames().to_vec();
    for pkg in &pkgnames {
        build_file
            .run_package_for(build_file_path, pkg, &formats, out_dir, &mut build)
            .await
            .map_err(ke)?;
    }

    // Optional cleanup of installed build deps.
    if args.rm_deps {
        pm.remove_installed(&mut deps).await.map_err(ke)?;
    }

    Ok(())
}
