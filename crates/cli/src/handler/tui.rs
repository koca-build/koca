//! Interactive iocraft TUI handlers for the `create` workflow.

use std::future::Future;
use std::path::Path;
use std::sync::Mutex;

use async_trait::async_trait;
use iocraft::prelude::*;
use koca::backend::{DependencyEvent, DownloadEvent, InstallEvent, RemoveEvent};
use koca::distro::Distro;
use koca::handler::{
    BuildHandler, DependencyHandler, ElevateCommandSpec, ElevatedChild, SourceHandler,
};
use koca::source::{format_bytes, Source, SourceProgress};
use koca::{BuildFile, BuildOutputLine, PackageManager};
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};
use zolt::Colorize;

use super::sudo::{self, SudoPty};
use crate::cli::CreateArgs;
use crate::components::{OutputGutter, ProgressBar, Spinner, GUTTER_WIDTH};
use crate::error::{CliError, CliMultiError, CliMultiResult};

/// Format a "done/total" byte pair, or just the done bytes when the total is
/// unknown.
fn size_pair(done: u64, total: Option<u64>) -> String {
    match total {
        Some(t) => format!("{}/{}", format_bytes(done), format_bytes(t)),
        None => format_bytes(done),
    }
}

/// Spawn `element`'s render loop, run `body`, then send `finish` so the section
/// component drains it, exits, and the loop tears down — leaving its final frame
/// in scrollback. Permanent summary lines are printed by the caller afterwards.
async fn run_section<T, M: Send + 'static>(
    mut element: impl ElementExt + Send + 'static,
    finish_tx: UnboundedSender<M>,
    finish: M,
    body: impl Future<Output = T>,
) -> T {
    let render = tokio::spawn(async move {
        let _ = element.render_loop().await;
    });
    let result = body.await;
    let _ = finish_tx.send(finish);
    let _ = render.await;
    result
}

/// Drive a section component: drain `rx` through `apply` until a finishing
/// message (or the channel closes), then exit the render loop. Returns whether
/// the section has finished, in which case the caller renders an empty view so
/// the live widget is cleared before teardown.
fn use_section<M, A, F>(
    hooks: &mut Hooks,
    rx: Option<UnboundedReceiver<M>>,
    mut apply: A,
    is_finish: F,
) -> bool
where
    M: Send + 'static,
    A: FnMut(M) + Send + 'static,
    F: Fn(&M) -> bool + Send + 'static,
{
    let mut done = hooks.use_state(|| false);
    hooks.use_future(async move {
        if let Some(mut rx) = rx {
            while let Some(msg) = rx.recv().await {
                if is_finish(&msg) {
                    break;
                }
                apply(msg);
            }
        }
        done.set(true);
    });
    let mut system = hooks.use_context_mut::<SystemContext>();
    if done.get() {
        system.exit();
    }
    done.get()
}

/// No-op dependency handler for the in-process, unprivileged resolve step.
struct NoopDep;

#[async_trait]
impl DependencyHandler for NoopDep {
    async fn elevate(
        &mut self,
        _spec: ElevateCommandSpec,
    ) -> std::io::Result<Box<dyn ElevatedChild>> {
        Err(std::io::Error::other("resolve never elevates"))
    }
}

#[derive(Default, Props)]
struct ResolveSectionProps {
    stop: Option<Mutex<UnboundedReceiver<()>>>,
}

#[component]
fn ResolveSection(
    mut hooks: Hooks,
    props: &mut ResolveSectionProps,
) -> impl Into<AnyElement<'static>> {
    let rx = props.stop.take().map(|m| m.into_inner().unwrap());
    if use_section(&mut hooks, rx, |_| {}, |_| true) {
        return element!(View).into_any();
    }
    element! {
        View {
            Spinner
            Text(content: " Resolving dependencies...", weight: Weight::Bold)
        }
    }
    .into_any()
}

const BAR_WIDTH: u16 = 30;
const SRC_BAR_WIDTH: u16 = 20;
/// Lines kept (and rendered) by the build/package gutter.
const GUTTER_TAIL: usize = 5;

/// One source's live fetch state, mirrored from the handler into the view.
#[derive(Clone, Default)]
struct SourceItem {
    url: String,
    fraction: Option<f64>,
    detail: String,
    bytes: u64,
    total_bytes: Option<u64>,
    done: bool,
    failed: bool,
}

#[derive(Default, Props)]
struct DownloadViewProps {
    done_pkgs: u32,
    total_pkgs: u32,
    bytes_done: u64,
    total_bytes: u64,
    /// Used only when `total_bytes` is unknown.
    percent: Option<u32>,
    active: Vec<String>,
}

#[component]
fn DownloadView(props: &DownloadViewProps) -> impl Into<AnyElement<'static>> {
    let pct = if props.total_bytes > 0 {
        ((props.bytes_done as f64 / props.total_bytes as f64) * 100.0) as u32
    } else {
        props.percent.unwrap_or(0)
    }
    .min(100);

    let header = if props.total_bytes > 0 {
        format!(
            " {}% ({})",
            pct,
            size_pair(props.bytes_done, Some(props.total_bytes))
        )
    } else {
        format!(" {pct}%")
    };

    let active_line = if props.active.is_empty() {
        element!(Text(color: Color::DarkGrey, content: " waiting for package downloads..."))
            .into_any()
    } else {
        let label = if props.active.len() == 1 {
            "download"
        } else {
            "downloads"
        };
        element! {
            View {
                Text(content: format!(" active ({} {}): ", props.active.len(), label))
                Text(color: Color::DarkGrey, content: props.active.join(", "))
            }
        }
        .into_any()
    };

    element! {
        View(flex_direction: FlexDirection::Column) {
            View {
                Text(content: format!("Downloading {}/{} packages ", props.done_pkgs, props.total_pkgs))
                ProgressBar(fraction: pct as f64 / 100.0, width: BAR_WIDTH)
                Text(content: header)
            }
            View {
                Spinner
                #(active_line)
            }
        }
    }
}

#[derive(Default, Props)]
struct InstallViewProps {
    is_remove: bool,
    done: u32,
    total: u32,
    /// Started but not yet finished; each counts as half a step.
    in_progress: u32,
    active: Vec<String>,
}

#[component]
fn InstallView(props: &InstallViewProps) -> impl Into<AnyElement<'static>> {
    let label = if props.is_remove {
        "Removing"
    } else {
        "Installing"
    };
    let total_steps = props.total as u64 * 2;
    let done_steps = props.done as u64 * 2 + props.in_progress as u64;
    let pct = if total_steps > 0 {
        ((done_steps as f64 / total_steps as f64) * 100.0) as u32
    } else {
        0
    }
    .min(100);
    let done_display = props.done.min(props.total);

    let active_line = if props.active.is_empty() {
        element!(Text(content: "")).into_any()
    } else {
        element!(Text(color: Color::DarkGrey, content: props.active.join(", "))).into_any()
    };

    element! {
        View(flex_direction: FlexDirection::Column) {
            View {
                Text(content: format!("{label} {done_display}/{} packages ", props.total))
                ProgressBar(fraction: pct as f64 / 100.0, width: BAR_WIDTH)
                Text(content: format!(" {pct}%"))
            }
            View {
                Spinner
                Text(content: " ")
                #(active_line)
            }
        }
    }
}

#[derive(Default, Props)]
struct SourcesViewProps {
    items: Vec<SourceItem>,
}

#[component]
fn SourcesView(props: &SourcesViewProps) -> impl Into<AnyElement<'static>> {
    let total = props.items.len();
    let completed = props.items.iter().filter(|i| i.done && !i.failed).count();
    let failed = props.items.iter().filter(|i| i.failed).count();
    let bytes_done: u64 = props.items.iter().map(|i| i.bytes).sum();
    let bytes_total: Option<u64> = {
        let known: Vec<u64> = props.items.iter().filter_map(|i| i.total_bytes).collect();
        (known.len() == total).then(|| known.iter().sum())
    };
    let size_str = size_pair(bytes_done, bytes_total.filter(|&t| t > 0));

    let header = if failed > 0 {
        element! {
            View {
                Spinner
                Text(content: " Fetching sources ", weight: Weight::Bold)
                Text(color: Color::Green, content: format!("{completed} completed"))
                Text(content: ", ")
                Text(color: Color::Red, content: format!("{failed} failed"))
                Text(content: format!(" ({size_str})"))
            }
        }
        .into_any()
    } else {
        element! {
            View {
                Spinner
                Text(content: " Fetching sources ", weight: Weight::Bold)
                Text(content: format!("{completed}/{total} ({size_str})"))
            }
        }
        .into_any()
    };

    let rows: Vec<AnyElement<'static>> = props
        .items
        .iter()
        .filter(|i| !i.done && !i.failed)
        .map(|i| match i.fraction {
            Some(frac) => element! {
                View {
                    Text(content: format!("  {} ", i.url))
                    ProgressBar(fraction: frac, width: SRC_BAR_WIDTH)
                    Text(color: Color::DarkGrey, content: format!(" {}", i.detail))
                }
            }
            .into_any(),
            None if i.detail.is_empty() => {
                element!(Text(content: format!("  {}", i.url))).into_any()
            }
            None => element! {
                View {
                    Text(content: format!("  {} ", i.url))
                    Text(color: Color::DarkGrey, content: i.detail.clone())
                }
            }
            .into_any(),
        })
        .collect();

    element! {
        View(flex_direction: FlexDirection::Column) {
            #(header)
            #(rows.into_iter())
        }
    }
}

#[derive(Default, Props)]
struct SudoAuthProps {
    io: Option<Mutex<SudoPty>>,
}

/// Renders sudo's live PTY output and forwards the user's keystrokes to it.
#[component]
fn SudoAuth(mut hooks: Hooks, props: &mut SudoAuthProps) -> impl Into<AnyElement<'static>> {
    let mut lines = hooks.use_state(Vec::<String>::new);
    // The keys sender must outlive `props.io` (which is `Some` only on the first
    // render), so stash it in persistent state for the event handler to read.
    let mut keys = hooks.use_state(|| None::<UnboundedSender<Vec<u8>>>);

    let io = props.io.take().map(|m| m.into_inner().unwrap());
    if let Some(io) = &io {
        if keys.read().is_none() {
            keys.set(Some(io.keys.clone()));
        }
    }

    hooks.use_future(async move {
        let Some(mut io) = io else { return };
        while let Some(snapshot) = io.lines.recv().await {
            lines.set(snapshot);
        }
    });

    hooks.use_terminal_events(move |event| {
        if let TerminalEvent::Key(KeyEvent {
            code,
            modifiers,
            kind,
            ..
        }) = event
        {
            if kind == KeyEventKind::Release {
                return;
            }
            if let Some(bytes) = encode_key(code, modifiers, kind) {
                if let Some(tx) = keys.read().as_ref() {
                    let _ = tx.send(bytes);
                }
            }
        }
    });

    element! {
        OutputGutter(
            header: "Authenticating (sudo)".to_string(),
            lines: lines.read().clone(),
            max_lines: None,
        )
    }
}

/// Encode a key into the byte sequence a real terminal would emit, for writing
/// straight to the PTY.
fn encode_key(code: KeyCode, modifiers: KeyModifiers, kind: KeyEventKind) -> Option<Vec<u8>> {
    let event = terminput_crossterm::to_terminput_key(crossterm::event::KeyEvent::new_with_kind(
        code, modifiers, kind,
    ))
    .ok()?;
    let mut buf = [0u8; 16];
    let n = terminput::Event::Key(event)
        .encode(&mut buf, terminput::Encoding::Xterm)
        .ok()?;
    Some(buf[..n].to_vec())
}

// ── Install section ───────────────────────────────────────────────────────

/// Which widget the install section is showing.
#[derive(Default, Clone, Copy, PartialEq)]
enum InstView {
    #[default]
    Idle,
    Auth,
    Download,
    Install,
}

enum InstallMsg {
    Start {
        downloads: u32,
        installs: u32,
    },
    RemoveStart {
        removes: u32,
    },
    DlStart {
        total_bytes: u64,
    },
    DlProgress {
        bytes_done: u64,
        bytes_total: u64,
        percent: Option<u32>,
        active: Vec<String>,
    },
    DlItemDone,
    DlDone,
    InstStart,
    Action {
        package: String,
    },
    ItemDone {
        package: String,
        current: u32,
    },
    Hook {
        name: String,
    },
    AuthStart {
        io: SudoPty,
    },
    Finish,
}

#[derive(Default)]
struct InstModel {
    view: InstView,
    dl_total_pkgs: u32,
    dl_done_pkgs: u32,
    dl_bytes_done: u64,
    dl_total_bytes: u64,
    dl_percent: Option<u32>,
    dl_active: Vec<String>,
    inst_total: u32,
    inst_done: u32,
    inst_in_progress: u32,
    inst_active: Vec<String>,
    inst_is_remove: bool,
    auth: Option<Mutex<SudoPty>>,
}

#[derive(Default, Props)]
struct InstallSectionProps {
    rx: Option<Mutex<UnboundedReceiver<InstallMsg>>>,
}

#[component]
fn InstallSection(
    mut hooks: Hooks,
    props: &mut InstallSectionProps,
) -> impl Into<AnyElement<'static>> {
    let mut model = hooks.use_state(InstModel::default);
    let rx = props.rx.take().map(|m| m.into_inner().unwrap());
    let finished = use_section(
        &mut hooks,
        rx,
        move |msg| apply_install(&mut model, msg),
        |msg| matches!(msg, InstallMsg::Finish),
    );
    if finished {
        return element!(View).into_any();
    }

    // Move the auth handle into `SudoAuth` once; guard the write so a dirty mark
    // doesn't busy-loop the render task.
    if model.read().view == InstView::Auth {
        let io = if model.read().auth.is_some() {
            model.write().auth.take()
        } else {
            None
        };
        return element!(SudoAuth(io: io)).into_any();
    }

    let m = model.read();
    match m.view {
        InstView::Idle | InstView::Auth => element!(View).into_any(),
        InstView::Download => element! {
            DownloadView(
                done_pkgs: m.dl_done_pkgs,
                total_pkgs: m.dl_total_pkgs,
                bytes_done: m.dl_bytes_done,
                total_bytes: m.dl_total_bytes,
                percent: m.dl_percent,
                active: m.dl_active.clone(),
            )
        }
        .into_any(),
        InstView::Install => element! {
            InstallView(
                is_remove: m.inst_is_remove,
                done: m.inst_done,
                total: m.inst_total,
                in_progress: m.inst_in_progress,
                active: m.inst_active.clone(),
            )
        }
        .into_any(),
    }
}

fn apply_install(model: &mut State<InstModel>, msg: InstallMsg) {
    let mut m = model.write();
    match msg {
        InstallMsg::Start {
            downloads,
            installs,
        } => {
            m.dl_total_pkgs = downloads;
            m.dl_done_pkgs = 0;
            m.inst_total = installs;
            m.inst_is_remove = false;
        }
        InstallMsg::RemoveStart { removes } => {
            m.inst_total = removes;
            m.inst_is_remove = true;
        }
        InstallMsg::DlStart { total_bytes } => {
            m.dl_total_bytes = total_bytes;
            m.dl_bytes_done = 0;
            m.view = InstView::Download;
        }
        InstallMsg::DlProgress {
            bytes_done,
            bytes_total,
            percent,
            active,
        } => {
            m.dl_bytes_done = bytes_done;
            if bytes_total > 0 {
                m.dl_total_bytes = bytes_total;
            }
            m.dl_percent = percent;
            m.dl_active = active;
        }
        InstallMsg::DlItemDone => m.dl_done_pkgs += 1,
        InstallMsg::DlDone => m.view = InstView::Idle,
        InstallMsg::InstStart => {
            m.inst_done = 0;
            m.inst_in_progress = 0;
            m.inst_active.clear();
            m.view = InstView::Install;
        }
        InstallMsg::Action { package } => {
            if !m.inst_active.contains(&package) {
                m.inst_active.push(package);
                m.inst_in_progress += 1;
            }
        }
        InstallMsg::ItemDone { package, current } => {
            if m.inst_active.contains(&package) {
                m.inst_active.retain(|n| *n != package);
                m.inst_in_progress = m.inst_in_progress.saturating_sub(1);
            }
            m.inst_done = current;
        }
        InstallMsg::Hook { name } => {
            m.inst_active.clear();
            m.inst_active.push(name);
        }
        InstallMsg::AuthStart { io } => {
            m.auth = Some(Mutex::new(io));
            m.view = InstView::Auth;
        }
        InstallMsg::Finish => {}
    }
}

/// Dependency handler feeding the install section, translating backend events to
/// [`InstallMsg`] and elevating the backend over a PTY.
struct InstallHandler {
    tx: UnboundedSender<InstallMsg>,
}

#[async_trait]
impl DependencyHandler for InstallHandler {
    fn on_install_start(&mut self, downloads: u32, installs: u32) {
        let _ = self.tx.send(InstallMsg::Start {
            downloads,
            installs,
        });
    }

    fn on_remove_start(&mut self, removes: u32) {
        let _ = self.tx.send(InstallMsg::RemoveStart { removes });
    }

    fn on_dep_event(&mut self, event: &DependencyEvent) {
        let msg = match event {
            DependencyEvent::Download { inner } => match inner {
                DownloadEvent::Start { total_bytes } => InstallMsg::DlStart {
                    total_bytes: *total_bytes,
                },
                DownloadEvent::Progress {
                    bytes_done,
                    bytes_total,
                    percent,
                    active,
                } => InstallMsg::DlProgress {
                    bytes_done: *bytes_done,
                    bytes_total: *bytes_total,
                    percent: *percent,
                    active: active.clone(),
                },
                DownloadEvent::ItemDone { .. } => InstallMsg::DlItemDone,
                DownloadEvent::Done => InstallMsg::DlDone,
            },
            DependencyEvent::Install { inner } => match inner {
                InstallEvent::Start => InstallMsg::InstStart,
                InstallEvent::Action { package, .. } => InstallMsg::Action {
                    package: package.clone(),
                },
                InstallEvent::ItemDone { package, current } => InstallMsg::ItemDone {
                    package: package.clone(),
                    current: *current,
                },
                InstallEvent::Hook { name, .. } => InstallMsg::Hook { name: name.clone() },
                InstallEvent::Done => return,
            },
            DependencyEvent::Remove { inner } => match inner {
                RemoveEvent::Start => InstallMsg::InstStart,
                RemoveEvent::Action { package, .. } => InstallMsg::Action {
                    package: package.clone(),
                },
                RemoveEvent::ItemDone { package, current } => InstallMsg::ItemDone {
                    package: package.clone(),
                    current: *current,
                },
                RemoveEvent::Done => return,
            },
        };
        let _ = self.tx.send(msg);
    }

    async fn elevate(
        &mut self,
        spec: ElevateCommandSpec,
    ) -> std::io::Result<Box<dyn ElevatedChild>> {
        if nix::unistd::geteuid().is_root() {
            return super::spawn_root_direct(&spec).await;
        }

        // Size the PTY to the width sudo's output will actually have inside the
        // auth gutter, so its prompts wrap where the gutter does.
        let (cols, rows) = crossterm::terminal::size().unwrap_or((80, 24));
        let (bridge, child) = sudo::spawn(spec, cols.saturating_sub(GUTTER_WIDTH), rows)?;
        let _ = self.tx.send(InstallMsg::AuthStart { io: bridge });
        Ok(child)
    }
}

// ── Sources section ─────────────────────────────────────────────────────────

enum SourcesMsg {
    Start {
        urls: Vec<String>,
    },
    Progress {
        url: String,
        fraction: Option<f64>,
        detail: String,
        bytes: u64,
        total_bytes: Option<u64>,
    },
    Done {
        url: String,
    },
    Failed {
        url: String,
        error: String,
    },
    Finish,
}

#[derive(Default, Props)]
struct SourcesSectionProps {
    rx: Option<Mutex<UnboundedReceiver<SourcesMsg>>>,
}

#[component]
fn SourcesSection(
    mut hooks: Hooks,
    props: &mut SourcesSectionProps,
) -> impl Into<AnyElement<'static>> {
    let mut items = hooks.use_state(Vec::<SourceItem>::new);
    let rx = props.rx.take().map(|m| m.into_inner().unwrap());
    let finished = use_section(
        &mut hooks,
        rx,
        move |msg| apply_sources(&mut items, msg),
        |msg| matches!(msg, SourcesMsg::Finish),
    );
    if finished {
        return element!(View).into_any();
    }
    element!(SourcesView(items: items.read().clone())).into_any()
}

fn apply_sources(state: &mut State<Vec<SourceItem>>, msg: SourcesMsg) {
    let mut items = state.write();
    match msg {
        SourcesMsg::Start { urls } => {
            *items = urls
                .into_iter()
                .map(|url| SourceItem {
                    url,
                    ..Default::default()
                })
                .collect();
        }
        SourcesMsg::Progress {
            url,
            fraction,
            detail,
            bytes,
            total_bytes,
        } => {
            if let Some(item) = items.iter_mut().find(|i| i.url == url) {
                item.fraction = fraction;
                item.detail = detail;
                item.bytes = bytes;
                item.total_bytes = total_bytes;
            }
        }
        SourcesMsg::Done { url } => {
            if let Some(item) = items.iter_mut().find(|i| i.url == url) {
                item.done = true;
            }
        }
        SourcesMsg::Failed { url, error } => {
            if let Some(item) = items.iter_mut().find(|i| i.url == url) {
                item.failed = true;
                item.detail = error;
            }
        }
        SourcesMsg::Finish => {}
    }
}

struct SourceSectionHandler {
    tx: UnboundedSender<SourcesMsg>,
}

impl SourceHandler for SourceSectionHandler {
    fn on_sources_start(&mut self, sources: &[Source]) {
        let urls = sources.iter().map(|s| s.display_url()).collect();
        let _ = self.tx.send(SourcesMsg::Start { urls });
    }

    fn on_source_progress(&mut self, source: &Source, progress: &SourceProgress) {
        let (detail, bytes, total_bytes) = match progress {
            SourceProgress::Download { bytes, total_bytes } => {
                (size_pair(*bytes, *total_bytes), *bytes, *total_bytes)
            }
            SourceProgress::Git {
                received_objects,
                total_objects,
                bytes,
            } => (
                format!("{received_objects}/{total_objects} objects"),
                *bytes,
                None,
            ),
        };
        let _ = self.tx.send(SourcesMsg::Progress {
            url: source.display_url(),
            fraction: progress.fraction(),
            detail,
            bytes,
            total_bytes,
        });
    }

    fn on_source_done(&mut self, source: &Source) {
        let _ = self.tx.send(SourcesMsg::Done {
            url: source.display_url(),
        });
    }

    fn on_source_error(&mut self, source: &Source, error: &str) {
        let _ = self.tx.send(SourcesMsg::Failed {
            url: source.display_url(),
            error: error.to_string(),
        });
    }
}

// ── Build / package section ─────────────────────────────────────────────────

enum GutterMsg {
    Line(String),
    Finish,
}

#[derive(Default, Props)]
struct GutterSectionProps {
    header: String,
    rx: Option<Mutex<UnboundedReceiver<GutterMsg>>>,
}

#[component]
fn GutterSection(
    mut hooks: Hooks,
    props: &mut GutterSectionProps,
) -> impl Into<AnyElement<'static>> {
    let mut lines = hooks.use_state(Vec::<String>::new);
    let header = props.header.clone();
    let rx = props.rx.take().map(|m| m.into_inner().unwrap());
    let finished = use_section(
        &mut hooks,
        rx,
        move |msg| {
            if let GutterMsg::Line(line) = msg {
                // Only the tail is ever shown; keep the rendered Vec bounded so a
                // long build doesn't re-clone thousands of lines each frame. The
                // full history lives in `GutterHandler.lines` for the failure dump.
                let mut lines = lines.write();
                lines.push(line);
                let overflow = lines.len().saturating_sub(GUTTER_TAIL);
                if overflow > 0 {
                    lines.drain(0..overflow);
                }
            }
        },
        |msg| matches!(msg, GutterMsg::Finish),
    );
    if finished {
        return element!(View).into_any();
    }
    element!(OutputGutter(header: header, lines: lines.read().clone(), max_lines: Some(GUTTER_TAIL)))
        .into_any()
}

/// Build handler feeding a gutter section; retains all lines so the caller can
/// dump them on failure.
struct GutterHandler {
    tx: UnboundedSender<GutterMsg>,
    lines: Vec<String>,
}

impl GutterHandler {
    fn new(tx: UnboundedSender<GutterMsg>) -> Self {
        Self {
            tx,
            lines: Vec::new(),
        }
    }

    fn push(&mut self, line: &BuildOutputLine) {
        self.lines.push(line.line.clone());
        let _ = self.tx.send(GutterMsg::Line(line.line.clone()));
    }
}

impl BuildHandler for GutterHandler {
    fn on_build_line(&mut self, line: &BuildOutputLine) {
        self.push(line);
    }

    fn on_package_line(&mut self, _pkgname: &str, line: &BuildOutputLine) {
        self.push(line);
    }
}

// ── Orchestration ───────────────────────────────────────────────────────────

/// Run a `create` end-to-end, driving one section render loop per phase.
pub async fn run(
    args: &CreateArgs,
    build_file_path: &Path,
    mut build_file: BuildFile,
    distro: &Distro,
) -> CliMultiResult<()> {
    let ke = |e: koca::KocaError| -> CliMultiError { CliError::Koca { err: e }.into() };
    let mut pm = PackageManager::for_distro(distro);

    // Resolve.
    let mut noop = NoopDep;
    let plan = {
        let (stop_tx, stop_rx) = unbounded_channel::<()>();
        let element = element!(ResolveSection(stop: Some(Mutex::new(stop_rx))));
        run_section(
            element,
            stop_tx,
            (),
            pm.resolve(build_file.all_deps(), &mut noop),
        )
        .await
        .map_err(ke)?
    };

    // Confirm (plain prompt — the terminal is free between sections).
    if !plan.is_empty() {
        if !super::plain::confirm(&plan, args.noconfirm) {
            return Ok(());
        }
        let (tx, rx) = unbounded_channel::<InstallMsg>();
        let element = element!(InstallSection(rx: Some(Mutex::new(rx))));
        let mut handler = InstallHandler { tx: tx.clone() };
        run_section(
            element,
            tx,
            InstallMsg::Finish,
            pm.install(&plan, &mut handler),
        )
        .await
        .map_err(ke)?;

        let downloads = plan.download_count();
        if downloads > 0 {
            println!("{} {} package(s)", "Downloaded".green(), downloads);
        }
        println!("{} {} package(s)", "Installed".green(), plan.actions.len());
    }

    // Sources.
    let arch = build_file.arch()[0].clone();
    let srcdir = Path::new("koca-build/src");
    let results = {
        let (tx, rx) = unbounded_channel::<SourcesMsg>();
        let element = element!(SourcesSection(rx: Some(Mutex::new(rx))));
        let mut handler = SourceSectionHandler { tx: tx.clone() };
        run_section(
            element,
            tx,
            SourcesMsg::Finish,
            build_file.fetch_sources(&arch, srcdir, &mut handler),
        )
        .await
    };
    let failures = results.iter().filter(|r| r.is_err()).count();
    if failures > 0 {
        return Err(ke(koca::KocaError::InvalidSource(format!(
            "{failures} source(s) failed to fetch"
        ))));
    }
    println!("{} {} source(s)", "Fetched".green(), results.len());

    // Build.
    if build_file.has_build() {
        let (tx, rx) = unbounded_channel::<GutterMsg>();
        let element =
            element!(GutterSection(header: "Building...".to_string(), rx: Some(Mutex::new(rx))));
        let mut handler = GutterHandler::new(tx.clone());
        let result = run_section(
            element,
            tx,
            GutterMsg::Finish,
            build_file.run_build(&mut handler),
        )
        .await;
        if let Err(err) = result {
            dump_failure("build failed", &handler.lines);
            return Err(ke(err));
        }
        println!(
            "{} {} {}",
            "Built".green(),
            build_file.pkgnames()[0].clone().bold(),
            build_file.version().to_string().dimmed()
        );
    }

    // Package, once per split package.
    let formats = args.output_type.bundle_formats();
    let out_dir = Path::new("koca-out");
    for pkg in build_file.pkgnames().to_vec() {
        let (tx, rx) = unbounded_channel::<GutterMsg>();
        let element = element!(GutterSection(
            header: format!("Packaging {pkg}..."),
            rx: Some(Mutex::new(rx)),
        ));
        let mut handler = GutterHandler::new(tx.clone());
        let result = run_section(
            element,
            tx,
            GutterMsg::Finish,
            build_file.run_package_for(build_file_path, &pkg, &formats, out_dir, &mut handler),
        )
        .await;
        match result {
            Ok(files) => {
                for file in &files {
                    println!("{} {}", "Package created:".green(), file.display());
                }
            }
            Err(err) => {
                dump_failure(&format!("package failed: {pkg}"), &handler.lines);
                return Err(ke(err));
            }
        }
    }

    // Optional cleanup of installed build deps.
    if args.rm_deps {
        let (tx, rx) = unbounded_channel::<InstallMsg>();
        let element = element!(InstallSection(rx: Some(Mutex::new(rx))));
        let mut handler = InstallHandler { tx: tx.clone() };
        run_section(
            element,
            tx,
            InstallMsg::Finish,
            pm.remove_installed(&mut handler),
        )
        .await
        .map_err(ke)?;
    }

    Ok(())
}

fn dump_failure(header: &str, lines: &[String]) {
    eprintln!("{}", header.red());
    for line in lines {
        eprintln!("  │ {line}");
    }
}
