use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use koca_proto::{DownloadEvent as ProtoDownloadEvent, ErrorCode, Event as ProtoEvent, ProtocolError};
use md5::Md5;
use sha1::Sha1;
use sha2::{Digest, Sha256};
use tokio::sync::{mpsc, Semaphore};

const ARCHIVES_DIR: &str = "/var/cache/apt/archives";
const PARTIAL_DIR: &str = "/var/cache/apt/archives/partial";
const PER_MIRROR_CONCURRENCY: usize = 3;
const EMIT_INTERVAL: Duration = Duration::from_millis(80);

#[derive(Clone, Copy)]
enum HashKind {
    Md5,
    Sha1,
    Sha256,
}

#[derive(Clone)]
pub(crate) struct DownloadItem {
    package: String,
    url: String,
    filename: String,
    size: u64,
    hash_kind: HashKind,
    hash_value: String,
    final_path: PathBuf,
    temp_path: PathBuf,
    mirror_key: String,
}

struct ProgressState {
    bytes_done: u64,
    active: Vec<String>,
}

struct AptArchiveInfo {
    filename: String,
    size: u64,
    sha256: String,
}

fn parse_hash_kind(s: &str) -> Option<HashKind> {
    match s.to_ascii_uppercase().as_str() {
        "MD5SUM" | "MD5" => Some(HashKind::Md5),
        "SHA1" => Some(HashKind::Sha1),
        "SHA256" => Some(HashKind::Sha256),
        _ => None,
    }
}

fn apt_package_for_filename(filename: &str, packages: &[String]) -> String {
    filename
        .split_once('_')
        .map(|(pkg, _)| pkg.to_string())
        .unwrap_or_else(|| packages.iter().find(|pkg| filename.starts_with(&format!("{pkg}_"))).cloned().unwrap_or_else(|| filename.to_string()))
}

/// NOTE: `apt-get --print-uris` is a de facto machine-readable CLI format, but
/// still a CLI text interface rather than a typed API. Keep parser coverage in
/// unit tests.
fn parse_print_uris_line(line: &str, packages: &[String]) -> Result<Option<DownloadItem>, ProtocolError> {
    let line = line.trim();
    if !line.starts_with('\'') {
        return Ok(None);
    }
    let end = line[1..].find('\'').ok_or_else(|| ProtocolError { code: ErrorCode::Internal, message: format!("invalid --print-uris line: {line}") })? + 1;
    let url = line[1..end].to_string();
    let mut parts = line[end + 1..].split_whitespace();
    let filename = parts.next().ok_or_else(|| ProtocolError { code: ErrorCode::Internal, message: format!("missing filename in --print-uris line: {line}") })?.to_string();
    let size = parts.next().ok_or_else(|| ProtocolError { code: ErrorCode::Internal, message: format!("missing size in --print-uris line: {line}") })?.parse().map_err(|_| ProtocolError { code: ErrorCode::Internal, message: format!("invalid size in --print-uris line: {line}") })?;
    let (hash_kind, hash_value) = match parts.next() {
        Some(hash) => {
            let (hash_name, hash_value) = hash.split_once(':').ok_or_else(|| ProtocolError { code: ErrorCode::Internal, message: format!("invalid hash in --print-uris line: {line}") })?;
            (parse_hash_kind(hash_name).ok_or_else(|| ProtocolError { code: ErrorCode::Internal, message: format!("unsupported hash type from apt: {hash_name}") })?, hash_value.to_string())
        }
        None => (HashKind::Sha256, String::new()),
    };
    let mirror_key = reqwest::Url::parse(&url).ok().and_then(|u| u.host_str().map(|host| format!("{}://{}", u.scheme(), host))).unwrap_or_else(|| "local".into());
    Ok(Some(DownloadItem {
        package: apt_package_for_filename(&filename, packages),
        url,
        temp_path: Path::new(PARTIAL_DIR).join(format!("{filename}.part")),
        final_path: Path::new(ARCHIVES_DIR).join(&filename),
        filename,
        size,
        hash_kind,
        hash_value,
        mirror_key,
    }))
}

fn run_apt_query(args: &[&str]) -> Result<std::process::Output, ProtocolError> {
    std::process::Command::new("apt-get")
        .args(args)
        .env("LC_ALL", "C")
        .env("DEBIAN_FRONTEND", "noninteractive")
        .output()
        .map_err(|e| ProtocolError {
            code: ErrorCode::Internal,
            message: format!("failed to run apt-get: {e}"),
        })
}

fn run_apt_cache_query(args: &[&str]) -> Result<std::process::Output, ProtocolError> {
    std::process::Command::new("apt-cache")
        .args(args)
        .env("LC_ALL", "C")
        .env("DEBIAN_FRONTEND", "noninteractive")
        .output()
        .map_err(|e| ProtocolError { code: ErrorCode::Internal, message: format!("failed to run apt-cache: {e}") })
}

fn query_archive_info(packages: &[String]) -> Result<HashMap<String, AptArchiveInfo>, ProtocolError> {
    let mut args: Vec<&str> = vec!["show", "--no-all-versions"];
    args.extend(packages.iter().map(|s| s.as_str()));
    let output = run_apt_cache_query(&args)?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(ProtocolError { code: ErrorCode::Internal, message: stderr.trim().to_string() });
    }
    let mut current: Option<AptArchiveInfo> = None;
    let mut result = HashMap::new();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        if line.is_empty() {
            if let Some(info) = current.take() {
                result.insert(info.filename.clone(), info);
            }
        } else if let Some(val) = line.strip_prefix("Filename: ") {
            let filename = val.rsplit('/').next().unwrap_or(val).trim().to_string();
            current = Some(AptArchiveInfo { filename, size: 0, sha256: String::new() });
        } else if let Some(val) = line.strip_prefix("Size: ") {
            if let Some(info) = current.as_mut() { info.size = val.trim().parse().unwrap_or(0); }
        } else if let Some(val) = line.strip_prefix("SHA256: ") {
            if let Some(info) = current.as_mut() { info.sha256 = val.trim().to_string(); }
        }
    }
    if let Some(info) = current.take() {
        result.insert(info.filename.clone(), info);
    }
    Ok(result)
}

pub(crate) fn get_download_items(packages: &[String]) -> Result<Vec<DownloadItem>, ProtocolError> {
    let mut args: Vec<&str> = vec!["install", "--print-uris", "-y"];
    args.extend(packages.iter().map(|s| s.as_str()));
    let output = run_apt_query(&args)?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(ProtocolError { code: ErrorCode::Internal, message: stderr.trim().to_string() });
    }
    let fallback = query_archive_info(packages)?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    stdout.lines().try_fold(Vec::new(), |mut items, line| {
        if let Some(item) = parse_print_uris_line(line, packages)? {
            let item = if item.hash_value.is_empty() {
                let info = fallback.get(&item.filename).ok_or_else(|| ProtocolError { code: ErrorCode::Internal, message: format!("missing fallback metadata for {}", item.filename) })?;
                DownloadItem { size: if item.size == 0 { info.size } else { item.size }, hash_value: info.sha256.clone(), ..item }
            } else {
                item
            };
            items.push(item);
        }
        Ok(items)
    })
}

fn ensure_cache_dirs() -> Result<(), ProtocolError> {
    fs::create_dir_all(ARCHIVES_DIR).map_err(|e| ProtocolError { code: ErrorCode::Internal, message: format!("create {}: {e}", ARCHIVES_DIR) })?;
    fs::create_dir_all(PARTIAL_DIR).map_err(|e| ProtocolError { code: ErrorCode::Internal, message: format!("create {}: {e}", PARTIAL_DIR) })?;
    Ok(())
}

fn compute_hash(path: &Path, kind: HashKind) -> Result<String, ProtocolError> {
    let mut file = File::open(path).map_err(|e| ProtocolError { code: ErrorCode::Internal, message: format!("open {}: {e}", path.display()) })?;
    let mut buf = [0u8; 64 * 1024];
    let mut md5 = matches!(kind, HashKind::Md5).then(Md5::new);
    let mut sha1 = matches!(kind, HashKind::Sha1).then(Sha1::new);
    let mut sha256 = matches!(kind, HashKind::Sha256).then(Sha256::new);
    loop {
        let read = file.read(&mut buf).map_err(|e| ProtocolError { code: ErrorCode::Internal, message: format!("read {}: {e}", path.display()) })?;
        if read == 0 {
            break;
        }
        if let Some(hasher) = md5.as_mut() { hasher.update(&buf[..read]); }
        if let Some(hasher) = sha1.as_mut() { hasher.update(&buf[..read]); }
        if let Some(hasher) = sha256.as_mut() { hasher.update(&buf[..read]); }
    }
    Ok(if let Some(hasher) = md5 { format!("{:x}", hasher.finalize()) } else if let Some(hasher) = sha1 { format!("{:x}", hasher.finalize()) } else { format!("{:x}", sha256.expect("sha256 hasher").finalize()) })
}

fn verify_download(path: &Path, item: &DownloadItem) -> Result<bool, ProtocolError> {
    let Ok(meta) = fs::metadata(path) else { return Ok(false) };
    if meta.len() != item.size {
        return Ok(false);
    }
    Ok(compute_hash(path, item.hash_kind)?.eq_ignore_ascii_case(&item.hash_value))
}

fn add_active(active: &mut Vec<String>, package: &str) {
    if !active.iter().any(|name| name == package) {
        active.push(package.to_string());
    }
}

fn remove_active(active: &mut Vec<String>, package: &str) {
    active.retain(|name| name != package);
}

fn emit_progress(tx: &mpsc::UnboundedSender<ProtoEvent>, state: &Arc<Mutex<ProgressState>>, total_bytes: u64) {
    let state = state.lock().unwrap();
    let _ = tx.send(ProtoEvent::Download {
        inner: ProtoDownloadEvent::Progress { bytes_done: state.bytes_done, bytes_total: total_bytes, percent: None, active: state.active.clone() },
    });
}

async fn download_one(client: &reqwest::Client, item: &DownloadItem, total_bytes: u64, state: &Arc<Mutex<ProgressState>>, tx: &mpsc::UnboundedSender<ProtoEvent>) -> Result<(), ProtocolError> {
    if item.url.starts_with("file:") {
        let url = reqwest::Url::parse(&item.url).map_err(|e| ProtocolError { code: ErrorCode::Internal, message: format!("invalid file URL {}: {e}", item.url) })?;
        let src = url.to_file_path().map_err(|_| ProtocolError { code: ErrorCode::Internal, message: format!("cannot convert file URL to path: {}", item.url) })?;
        fs::copy(&src, &item.temp_path).map_err(|e| ProtocolError { code: ErrorCode::Internal, message: format!("copy {} -> {}: {e}", src.display(), item.temp_path.display()) })?;
    } else {
        let mut response = client.get(&item.url).send().await.map_err(|e| ProtocolError { code: ErrorCode::Internal, message: format!("download failed for {}: {e}", item.filename) })?;
        if !response.status().is_success() {
            return Err(ProtocolError { code: ErrorCode::Internal, message: format!("HTTP {} for {}", response.status(), item.url) });
        }
        let mut file = File::create(&item.temp_path).map_err(|e| ProtocolError { code: ErrorCode::Internal, message: format!("create {}: {e}", item.temp_path.display()) })?;
        let mut last_emit = Instant::now();
        while let Some(chunk) = response.chunk().await.map_err(|e| ProtocolError { code: ErrorCode::Internal, message: format!("download {}: {e}", item.filename) })? {
            file.write_all(&chunk).map_err(|e| ProtocolError { code: ErrorCode::Internal, message: format!("write {}: {e}", item.temp_path.display()) })?;
            state.lock().unwrap().bytes_done += chunk.len() as u64;
            if last_emit.elapsed() >= EMIT_INTERVAL {
                emit_progress(tx, state, total_bytes);
                last_emit = Instant::now();
            }
        }
    }
    if !verify_download(&item.temp_path, item)? {
        let _ = fs::remove_file(&item.temp_path);
        return Err(ProtocolError { code: ErrorCode::Internal, message: format!("verification failed for {}", item.filename) });
    }
    fs::rename(&item.temp_path, &item.final_path).map_err(|e| ProtocolError { code: ErrorCode::Internal, message: format!("move {} -> {}: {e}", item.temp_path.display(), item.final_path.display()) })?;
    emit_progress(tx, state, total_bytes);
    Ok(())
}

pub(crate) async fn download_packages(items: &[DownloadItem], tx: &mpsc::UnboundedSender<ProtoEvent>) -> Result<(), ProtocolError> {
    ensure_cache_dirs()?;
    let mut cached = Vec::new();
    let mut needed = Vec::new();
    for item in items {
        if verify_download(&item.final_path, item)? { cached.push(item.clone()); } else { let _ = fs::remove_file(&item.final_path); let _ = fs::remove_file(&item.temp_path); needed.push(item.clone()); }
    }
    let total_bytes: u64 = needed.iter().map(|item| item.size).sum();
    let _ = tx.send(ProtoEvent::Download { inner: ProtoDownloadEvent::Start { total_bytes, total_packages: items.len() as u32 } });
    for item in cached {
        let _ = tx.send(ProtoEvent::Download { inner: ProtoDownloadEvent::ItemDone { package: item.package } });
    }
    if needed.is_empty() {
        let _ = tx.send(ProtoEvent::Download { inner: ProtoDownloadEvent::Done });
        return Ok(());
    }
    let client = reqwest::Client::new();
    let state = Arc::new(Mutex::new(ProgressState { bytes_done: 0, active: Vec::new() }));
    let semaphores: Arc<HashMap<String, Arc<Semaphore>>> = Arc::new(needed.iter().map(|item| (item.mirror_key.clone(), Arc::new(Semaphore::new(PER_MIRROR_CONCURRENCY)))).collect());
    let mut handles = Vec::new();
    for item in needed {
        let client = client.clone(); let tx = tx.clone(); let state = state.clone(); let semaphores = semaphores.clone();
        handles.push(tokio::spawn(async move {
            let permit = semaphores.get(&item.mirror_key).expect("mirror semaphore").clone().acquire_owned().await.map_err(|_| ProtocolError { code: ErrorCode::Internal, message: "download semaphore closed".into() })?;
            { let mut state = state.lock().unwrap(); add_active(&mut state.active, &item.package); }
            emit_progress(&tx, &state, total_bytes);
            let result = download_one(&client, &item, total_bytes, &state, &tx).await;
            { let mut state = state.lock().unwrap(); remove_active(&mut state.active, &item.package); }
            drop(permit);
            emit_progress(&tx, &state, total_bytes);
            result?;
            let _ = tx.send(ProtoEvent::Download { inner: ProtoDownloadEvent::ItemDone { package: item.package } });
            Ok::<(), ProtocolError>(())
        }));
    }
    for handle in handles {
        handle.await.unwrap_or_else(|_| Err(ProtocolError { code: ErrorCode::Internal, message: "download task panicked".into() }))?;
    }
    let _ = tx.send(ProtoEvent::Download { inner: ProtoDownloadEvent::Done });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::parse_print_uris_line;

    #[test]
    fn parses_print_uris_line() {
        let item = parse_print_uris_line("'https://deb.debian.org/debian/pool/main/g/gcc/gcc_4%3a13.2.0-7_amd64.deb' gcc_4%3a13.2.0-7_amd64.deb 12345 SHA256:deadbeef", &[String::from("gcc")]).unwrap().unwrap();
        assert_eq!(item.package, "gcc");
        assert_eq!(item.filename, "gcc_4%3a13.2.0-7_amd64.deb");
        assert_eq!(item.size, 12345);
        assert_eq!(item.hash_value, "deadbeef");
    }

    #[test]
    fn parses_print_uris_line_without_hash() {
        let item = parse_print_uris_line("'https://deb.debian.org/debian/pool/main/g/gcc/gcc_4%3a13.2.0-7_amd64.deb' gcc_4%3a13.2.0-7_amd64.deb 12345", &[String::from("gcc")]).unwrap().unwrap();
        assert_eq!(item.filename, "gcc_4%3a13.2.0-7_amd64.deb");
        assert_eq!(item.size, 12345);
        assert!(item.hash_value.is_empty());
    }
}
