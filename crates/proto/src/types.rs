use clap::Parser;
use serde::{Deserialize, Serialize};

// ── Backend CLI args (shared across all backends) ─────────────────────────

/// CLI arguments shared by all koca backend binaries.
///
/// Each backend binary uses this as its top-level clap struct:
/// ```rust,ignore
/// let args = BackendArgs::parse();
/// let session = BackendSession::connect(&args.socket).await?;
/// ```
#[derive(Debug, Parser)]
#[command(about = "koca package manager backend")]
pub struct BackendArgs {
    /// Socket name to connect back to koca (created by the koca parent process).
    #[arg(long)]
    pub socket: String,
}

// ── Requests (koca → backend) ─────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize)]
pub struct Request {
    pub id: u64,
    #[serde(flatten)]
    pub cmd: Command,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "kebab-case")]
pub enum Command {
    CheckInstalled { packages: Vec<String> },
    InstallPlan { packages: Vec<String> },
    Confirm,
    Abort,
    Remove { packages: Vec<String> },
    Shutdown,
}

// ── Responses (backend → koca) ────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize)]
pub struct Message {
    pub id: u64,
    #[serde(flatten)]
    pub body: MessageBody,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum MessageBody {
    Result { result: ResultPayload },
    Event { event: Event },
    Error { error: ProtocolError },
}

// ── Result payloads ───────────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "kebab-case")]
pub enum ResultPayload {
    CheckInstalled {
        packages: Vec<PackageStatus>,
    },
    InstallPlan {
        actions: Vec<PlannedAction>,
        total_download: u64,
        total_install: u64,
    },
    Install {
        success: bool,
        installed: Vec<String>,
    },
    Remove {
        success: bool,
        removed: Vec<String>,
    },
    Aborted,
}

/// Status of a single package from `check-installed`.
#[derive(Debug, Serialize, Deserialize)]
pub struct PackageStatus {
    pub name: String,
    pub status: InstalledStatus,
    /// Present when `status` is `Installed`.
    pub version: Option<String>,
    /// Whether the package is auto-installed (a dep) vs explicitly installed.
    /// `None` if the package is not installed.
    pub is_auto: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum InstalledStatus {
    Installed,
    Missing,
}

/// A single package action in an install plan.
#[derive(Debug, Serialize, Deserialize)]
pub struct PlannedAction {
    pub name: String,
    pub version: String,
    /// Present for upgrades/downgrades.
    pub old_version: Option<String>,
    pub action: ActionKind,
    /// Bytes to download.
    pub download_size: u64,
    /// Bytes on disk after install.
    pub install_size: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ActionKind {
    Install,
    Upgrade,
    Downgrade,
    Reinstall,
    Remove,
}

// ── Streaming events ──────────────────────────────────────────────────────

/// A progress event emitted during a streaming command (install/remove).
///
/// Wire format: `{"phase":"download","event":"start","total_bytes":...}`
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "phase", rename_all = "kebab-case")]
pub enum Event {
    Download {
        #[serde(flatten)]
        inner: DownloadEvent,
    },
    Install {
        #[serde(flatten)]
        inner: InstallEvent,
    },
    Remove {
        #[serde(flatten)]
        inner: RemoveEvent,
    },
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "kebab-case")]
pub enum DownloadEvent {
    Start {
        total_bytes: u64,
        total_packages: u32,
    },
    Progress {
        package: String,
        bytes_done: u64,
        bytes_total: u64,
    },
    ItemDone {
        package: String,
    },
    Done,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "kebab-case")]
pub enum InstallEvent {
    Start {
        total_packages: u32,
    },
    Action {
        package: String,
        action: String,
        current: u32,
        total: u32,
        percent: Option<u32>,
    },
    ItemDone {
        package: String,
        current: u32,
        total: u32,
    },
    Hook {
        name: String,
        current: u32,
        total: u32,
    },
    Done,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "kebab-case")]
pub enum RemoveEvent {
    Start {
        total_packages: u32,
    },
    Action {
        package: String,
        action: String,
        current: u32,
        total: u32,
        percent: Option<u32>,
    },
    ItemDone {
        package: String,
        current: u32,
        total: u32,
    },
    Done,
}

// ── Errors ────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize)]
pub struct ProtocolError {
    pub code: ErrorCode,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ErrorCode {
    /// Backend lacks root privileges. koca should re-launch with sudo.
    NeedsElevation,
    PackageNotFound,
    DependencyConflict,
    TransactionFailed,
    DatabaseLocked,
    Internal,
}

impl std::fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}: {}", self.code, self.message)
    }
}

impl std::error::Error for ProtocolError {}
