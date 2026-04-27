use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::os::unix::io::{AsRawFd, FromRawFd};
use std::process::Stdio;

mod download;

use super::transport::BackendSession;
use super::types::{
    ActionKind, ErrorCode, Event as ProtoEvent, InstallEvent as ProtoInstallEvent,
    InstalledStatus, Message, MessageBody, PackageStatus, PlannedAction, ProtocolError,
    RemoveEvent as ProtoRemoveEvent, ResultPayload,
};
use tokio::sync::mpsc;

use download::{download_packages, get_download_items};

fn run_cmd(program: &str, args: &[&str]) -> Result<std::process::Output, ProtocolError> {
    std::process::Command::new(program)
        .args(args)
        .env("LC_ALL", "C")
        .env("DEBIAN_FRONTEND", "noninteractive")
        .output()
        .map_err(|e| ProtocolError {
            code: ErrorCode::Internal,
            message: format!("failed to run {program}: {e}"),
        })
}

fn check_preconditions() -> Result<(), ProtocolError> {
    if !nix::unistd::geteuid().is_root() {
        return Err(ProtocolError {
            code: ErrorCode::NeedsElevation,
            message: "must be run as root".into(),
        });
    }
    Ok(())
}

fn classify_apt_error(stderr: &str) -> ErrorCode {
    if stderr.contains("Unable to locate package") {
        ErrorCode::PackageNotFound
    } else if stderr.contains("Unable to acquire the dpkg frontend lock") {
        ErrorCode::DatabaseLocked
    } else {
        ErrorCode::Internal
    }
}

struct DpkgPkg {
    version: String,
    status: String,
}

fn query_dpkg(packages: &[String]) -> HashMap<String, DpkgPkg> {
    let mut args: Vec<&str> = vec!["-W", "-f", "${Package}\\t${Version}\\t${db:Status-Status}\\n"];
    args.extend(packages.iter().map(|s| s.as_str()));
    let output = match run_cmd("dpkg-query", &args) {
        Ok(output) => output,
        Err(_) => return HashMap::new(),
    };
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            let parts: Vec<&str> = line.split('\t').collect();
            (parts.len() == 3).then(|| (parts[0].to_string(), DpkgPkg { version: parts[1].to_string(), status: parts[2].to_string() }))
        })
        .collect()
}

fn query_auto_installed(packages: &[String]) -> std::collections::HashSet<String> {
    let mut args: Vec<&str> = vec!["showmanual"];
    args.extend(packages.iter().map(|s| s.as_str()));
    let output = match run_cmd("apt-mark", &args) {
        Ok(output) => output,
        Err(_) => return std::collections::HashSet::new(),
    };
    String::from_utf8_lossy(&output.stdout).lines().map(|line| line.trim().to_string()).collect()
}

pub fn check_installed(packages: &[String]) -> Result<ResultPayload, ProtocolError> {
    let dpkg = query_dpkg(packages);
    let manual = query_auto_installed(packages);
    Ok(ResultPayload::CheckInstalled {
        packages: packages.iter().map(|name| match dpkg.get(name) {
            Some(pkg) if pkg.status == "installed" => PackageStatus {
                name: name.clone(),
                status: InstalledStatus::Installed,
                version: Some(pkg.version.clone()),
                is_auto: Some(!manual.contains(name)),
            },
            _ => PackageStatus { name: name.clone(), status: InstalledStatus::Missing, version: None, is_auto: None },
        }).collect(),
    })
}

struct InstLine {
    name: String,
    version: String,
    old_version: Option<String>,
}

/// NOTE: `Inst` lines come from `apt-get -s` human-oriented CLI output.
/// This has been stable in practice, but it is not a formal machine interface.
/// Keep parser coverage in unit tests.
fn parse_inst_lines(output: &str) -> Vec<InstLine> {
    let mut result = Vec::new();
    for line in output.lines() {
        if let Some(rest) = line.strip_prefix("Inst ") {
            let Some((name, rest)) = rest.split_once(' ') else { continue };
            let old_version = if rest.starts_with('[') {
                rest.find(']').map(|end| rest[1..end].to_string())
            } else {
                None
            };
            let version = rest.find('(').and_then(|start| rest[start + 1..].split_once(' ').map(|(v, _)| v.to_string())).unwrap_or_default();
            result.push(InstLine { name: name.to_string(), version, old_version });
        }
    }
    result
}

struct AptPkgInfo {
    download_size: u64,
    install_size_kb: u64,
}

/// NOTE: `Package`, `Size`, and `Installed-Size` are standard Debian control
/// fields surfaced by `apt-cache show`, which is more stable than parsing
/// `apt-get -s` text output.
fn query_apt_cache(packages: &[String]) -> HashMap<String, AptPkgInfo> {
    let mut args: Vec<&str> = vec!["show", "--no-all-versions"];
    args.extend(packages.iter().map(|s| s.as_str()));
    let output = match run_cmd("apt-cache", &args) {
        Ok(output) => output,
        Err(_) => return HashMap::new(),
    };

    let mut result = HashMap::new();
    let mut current_name = String::new();
    let mut download_size = 0;
    let mut install_size_kb = 0;
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        if line.is_empty() {
            if !current_name.is_empty() {
                result.insert(current_name.clone(), AptPkgInfo { download_size, install_size_kb });
            }
            current_name.clear();
            download_size = 0;
            install_size_kb = 0;
        } else if let Some(val) = line.strip_prefix("Package: ") {
            current_name = val.trim().to_string();
        } else if let Some(val) = line.strip_prefix("Size: ") {
            download_size = val.trim().parse().unwrap_or(0);
        } else if let Some(val) = line.strip_prefix("Installed-Size: ") {
            install_size_kb = val.trim().parse().unwrap_or(0);
        }
    }
    if !current_name.is_empty() {
        result.insert(current_name, AptPkgInfo { download_size, install_size_kb });
    }
    result
}

pub fn install_plan(packages: &[String]) -> Result<(ResultPayload, Vec<String>), ProtocolError> {
    let mut args: Vec<&str> = vec!["install", "-s", "-y"];
    args.extend(packages.iter().map(|s| s.as_str()));
    let output = run_cmd("apt-get", &args)?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(ProtocolError { code: classify_apt_error(&stderr), message: stderr.trim().to_string() });
    }

    let inst_lines = parse_inst_lines(&String::from_utf8_lossy(&output.stdout));
    let names: Vec<String> = inst_lines.iter().map(|line| line.name.clone()).collect();
    let cache = query_apt_cache(&names);
    let actions: Vec<PlannedAction> = inst_lines.into_iter().map(|line| {
        let info = cache.get(&line.name);
        PlannedAction {
            name: line.name,
            version: line.version,
            old_version: line.old_version.clone(),
            action: if line.old_version.is_some() { ActionKind::Upgrade } else { ActionKind::Install },
            download_size: info.map(|i| i.download_size).unwrap_or(0),
            install_size: info.map(|i| i.install_size_kb * 1024).unwrap_or(0),
        }
    }).collect();

    Ok((
        ResultPayload::InstallPlan {
            total_download: actions.iter().map(|a| a.download_size).sum(),
            total_install: actions.iter().map(|a| a.install_size).sum(),
            actions,
        },
        names,
    ))
}

struct AptStatusState {
    current: u32,
    seen: std::collections::HashSet<String>,
}

fn parse_status_line(line: &str, n_pkgs: u32, is_remove: bool, state: &mut AptStatusState) -> Vec<ProtoEvent> {
    let parts: Vec<&str> = line.splitn(4, ':').collect();
    if parts.len() < 4 || parts[0] != "pmstatus" {
        return Vec::new();
    }
    // Strip :arch suffix (e.g. "ca-certificates:amd64" -> "ca-certificates").
    let pkg = parts[1].split_once(':').map(|(name, _)| name).unwrap_or(parts[1]);
    let percent = parts[2].parse::<f64>().ok().map(|p| p as u32);
    let action = parts[3].to_lowercase();
    if pkg == "dpkg-exec" {
        return Vec::new();
    }
    // "Installed"/"Removed" = done for this package.
    if action.starts_with("installed ") || action.starts_with("removed ") {
        state.seen.remove(pkg);
        return vec![if is_remove {
            ProtoEvent::Remove { inner: ProtoRemoveEvent::ItemDone { package: pkg.to_string(), current: state.current, total: n_pkgs } }
        } else {
            ProtoEvent::Install { inner: ProtoInstallEvent::ItemDone { package: pkg.to_string(), current: state.current, total: n_pkgs } }
        }];
    }
    // First action for a new package = count it.
    if state.seen.insert(pkg.to_string()) {
        state.current += 1;
    }
    vec![if is_remove {
        ProtoEvent::Remove { inner: ProtoRemoveEvent::Action { package: pkg.to_string(), action, current: state.current, total: n_pkgs, percent } }
    } else {
        ProtoEvent::Install { inner: ProtoInstallEvent::Action { package: pkg.to_string(), action, current: state.current, total: n_pkgs, percent } }
    }]
}

fn run_apt_with_status(pkgs: &[String], is_remove: bool, n_pkgs: u32, event_tx: &mpsc::UnboundedSender<ProtoEvent>) -> Result<Vec<String>, ProtocolError> {
    let (status_read, status_write) = nix::unistd::pipe().map_err(|e| ProtocolError { code: ErrorCode::Internal, message: format!("pipe failed: {e}") })?;
    let status_fd_opt = format!("APT::Status-Fd={}", status_write.as_raw_fd());
    let mut args = vec![if is_remove { "remove" } else { "install" }, "-y", "-o", &status_fd_opt, "-o", "Dpkg::Use-Pty=0"];
    for pkg in pkgs {
        args.push(pkg);
    }
    let child = std::process::Command::new("apt-get").args(&args).env("LC_ALL", "C").env("DEBIAN_FRONTEND", "noninteractive").stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().map_err(|e| ProtocolError { code: ErrorCode::Internal, message: format!("failed to spawn apt-get: {e}") })?;
    drop(status_write);
    let fd_tx = event_tx.clone();
    let status_reader = unsafe { std::fs::File::from_raw_fd(status_read.as_raw_fd()) };
    std::mem::forget(status_read);
    let fd_handle = std::thread::spawn(move || {
        let mut state = AptStatusState { current: 0, seen: std::collections::HashSet::new() };
        for line in BufReader::new(status_reader).lines().map_while(Result::ok) {
            for evt in parse_status_line(&line, n_pkgs, is_remove, &mut state) {
                let _ = fd_tx.send(evt);
            }
        }
    });
    let output = child.wait_with_output().map_err(|e| ProtocolError { code: ErrorCode::Internal, message: format!("failed to wait for apt-get: {e}") })?;
    let _ = fd_handle.join();
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(ProtocolError { code: classify_apt_error(&stderr), message: stderr.trim().to_string() });
    }
    // Mark all installed packages as auto so they can be cleaned up with autoremove.
    if !is_remove {
        let mut mark_args: Vec<&str> = vec!["auto"];
        mark_args.extend(pkgs.iter().map(|s| s.as_str()));
        let _ = run_cmd("apt-mark", &mark_args);
    }
    Ok(pkgs.to_vec())
}

pub async fn commit_transaction(msg_id: u64, packages: Vec<String>, is_remove: bool, session: &mut BackendSession) {
    if let Err(error) = check_preconditions() {
        let _ = session.send(&Message { id: msg_id, body: MessageBody::Error { error } }).await;
        return;
    }
    let (event_tx, mut event_rx) = mpsc::unbounded_channel::<ProtoEvent>();
    let pkgs = packages.clone();
    let n_pkgs = pkgs.len() as u32;
    let join_handle = tokio::spawn(async move {
        if is_remove {
            let _ = event_tx.send(ProtoEvent::Remove { inner: ProtoRemoveEvent::Start { total_packages: n_pkgs } });
            let tx = event_tx.clone();
            let result = tokio::task::spawn_blocking(move || run_apt_with_status(&pkgs, true, n_pkgs, &tx)).await;
            let _ = event_tx.send(ProtoEvent::Remove { inner: ProtoRemoveEvent::Done });
            return result.unwrap_or_else(|_| Err(ProtocolError { code: ErrorCode::Internal, message: "remove task panicked".into() }));
        }
        // Resolve the full dep count via dry-run before downloading.
        let resolved_count = {
            let p = pkgs.clone();
            tokio::task::spawn_blocking(move || {
                let mut args: Vec<&str> = vec!["install", "-s", "-y"];
                args.extend(p.iter().map(|s| s.as_str()));
                let output = run_cmd("apt-get", &args)?;
                Ok::<u32, ProtocolError>(parse_inst_lines(&String::from_utf8_lossy(&output.stdout)).len() as u32)
            }).await.unwrap_or(Ok(0)).unwrap_or(0)
        };
        let items = tokio::task::spawn_blocking({ let pkgs = pkgs.clone(); move || get_download_items(&pkgs) }).await.unwrap_or_else(|_| Err(ProtocolError { code: ErrorCode::Internal, message: "download URL task panicked".into() }))?;
        download_packages(&items, &event_tx).await?;
        let install_count = if resolved_count > 0 { resolved_count } else { items.len() as u32 };
        let _ = event_tx.send(ProtoEvent::Install { inner: ProtoInstallEvent::Start { total_packages: install_count } });
        let tx = event_tx.clone();
        let result = tokio::task::spawn_blocking(move || run_apt_with_status(&pkgs, false, install_count, &tx)).await;
        let _ = event_tx.send(ProtoEvent::Install { inner: ProtoInstallEvent::Done });
        result.unwrap_or_else(|_| Err(ProtocolError { code: ErrorCode::Internal, message: "install task panicked".into() }))
    });
    while let Some(event) = event_rx.recv().await {
        let _ = session.send(&Message { id: msg_id, body: MessageBody::Event { event } }).await;
    }
    let body = match join_handle.await {
        Ok(Ok(names)) => MessageBody::Result { result: if is_remove { ResultPayload::Remove { success: true, removed: names } } else { ResultPayload::Install { success: true, installed: names } } },
        Ok(Err(error)) => MessageBody::Error { error },
        Err(_) => MessageBody::Error { error: ProtocolError { code: ErrorCode::Internal, message: "backend task panicked".into() } },
    };
    let _ = session.send(&Message { id: msg_id, body }).await;
}

#[cfg(test)]
mod tests {
    use super::parse_inst_lines;

    #[test]
    fn parses_inst_lines_with_and_without_old_version() {
        let lines = parse_inst_lines("Inst gcc (4:13.2.0-7 Debian:unstable [amd64])\nInst libc6 [2.36-9] (2.37-1 Debian:unstable [amd64])\n");
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].name, "gcc");
        assert_eq!(lines[0].old_version, None);
        assert_eq!(lines[1].name, "libc6");
        assert_eq!(lines[1].old_version.as_deref(), Some("2.36-9"));
    }
}
