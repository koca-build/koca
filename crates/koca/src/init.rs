//! Process bootstrap and helper dispatch.
//!
//! Koca obtains privilege and a faked root by **re-executing its own binary**
//! into a helper mode, signalled through reserved `__KOCA_*` environment
//! variables. [`init`] catches those re-execs — call it once, as the first
//! thing in `main`, exactly like [`fakeroost::init`].
//!
//! The `__KOCA_*` variables are an internal contract: consumers must never set
//! or rely on them.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::backend::BackendKind;
use crate::{BuildFile, BundleFormat};

/// Names the helper mode for a re-exec'd koca process (`backend-apt`,
/// `backend-alpm`, or `package`).
pub const HELPER_VAR: &str = "__KOCA_HELPER";
/// Socket name a backend helper connects back to.
pub const SOCKET_VAR: &str = "__KOCA_SOCKET";
/// Path to the JSON [`PackageSpec`] for the `package` helper.
pub const SPEC_VAR: &str = "__KOCA_SPEC";

/// The inputs handed to a `package` helper, serialized to a temp JSON file
/// whose path travels in [`SPEC_VAR`]. The parent writes it; the helper (which
/// runs under the fakeroot supervisor) reads it.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct PackageSpec {
    /// The build file to re-parse inside the helper.
    pub build_file: PathBuf,
    /// The single package to build and bundle.
    pub pkg: String,
    /// Directory the bundled artifact(s) are written into.
    pub out_dir: PathBuf,
    /// Formats to bundle the package into.
    pub formats: Vec<BundleFormat>,
}

/// Become a koca helper if this process was re-exec'd as one — call this
/// **once, as the first thing in `main`** (before building a runtime).
///
/// Calls [`fakeroost::init`] first, so a process launched as the fakeroot
/// supervisor is caught there and never reaches the koca dispatch below.
/// (Ordering matters: the supervisor process still carries any inherited
/// `__KOCA_HELPER` in its environment, so checking it first would make the
/// supervisor wrongly run the helper instead of supervising.)
///
/// On a normal launch this returns immediately. On a helper re-exec it runs
/// the helper to completion and exits the process — it never returns.
pub fn init() {
    // Becomes the fakeroot supervisor if launched as one (never returns then).
    fakeroost::init();

    let Some(helper) = std::env::var_os(HELPER_VAR) else {
        return; // normal launch
    };
    let helper = helper.to_string_lossy().into_owned();

    // Helpers run async library code, but `init` is called before the
    // application builds its own runtime — so spin one up just for the helper.
    let rt = tokio::runtime::Runtime::new().expect("failed to build koca helper runtime");
    let code = rt.block_on(run_helper(&helper));
    std::process::exit(code);
}

async fn run_helper(helper: &str) -> i32 {
    match helper {
        "backend-apt" => run_backend_helper(BackendKind::Apt).await,
        "backend-alpm" => run_backend_helper(BackendKind::Alpm).await,
        "package" => run_package_helper().await,
        other => {
            eprintln!("koca: unknown helper '{other}'");
            1
        }
    }
}

/// Required-var lookup that fails loudly: a missing `__KOCA_*` here means the
/// parent built a malformed helper spec, which is a koca bug.
fn require_var(name: &str) -> Result<String, i32> {
    std::env::var(name).map_err(|_| {
        eprintln!("koca: helper missing required {name}");
        1
    })
}

async fn run_backend_helper(kind: BackendKind) -> i32 {
    let socket = match require_var(SOCKET_VAR) {
        Ok(s) => s,
        Err(code) => return code,
    };
    match crate::backend::run_backend(&socket, kind).await {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("koca: backend helper error: {e}");
            1
        }
    }
}

/// Run `package()` for one package and bundle it, under the fakeroot supervisor
/// this process is already running beneath. Output goes to stdout/stderr, which
/// the parent captures and forwards to a [`crate::handler::BuildHandler`].
async fn run_package_helper() -> i32 {
    let spec_path = match require_var(SPEC_VAR) {
        Ok(p) => p,
        Err(code) => return code,
    };
    let spec: PackageSpec = match std::fs::read(&spec_path)
        .map_err(|e| e.to_string())
        .and_then(|bytes| serde_json::from_slice(&bytes).map_err(|e| e.to_string()))
    {
        Ok(spec) => spec,
        Err(e) => {
            eprintln!("koca: invalid package spec at {spec_path}: {e}");
            return 1;
        }
    };

    let mut build_file = match BuildFile::parse_file(&spec.build_file).await {
        Ok(bf) => bf,
        Err(errs) => {
            for err in errs {
                eprintln!("koca: {err}");
            }
            return 1;
        }
    };

    // Run package() in-process; print every captured line straight through.
    if let Err(e) = build_file
        .exec_package(&spec.pkg, |line| print_build_line(line))
        .await
    {
        eprintln!("koca: {e}");
        return 1;
    }

    let arch = build_file.arch()[0].clone();
    let version = build_file.version().to_string();
    if let Err(e) = std::fs::create_dir_all(&spec.out_dir) {
        eprintln!("koca: failed to create output dir {}: {e}", spec.out_dir.display());
        return 1;
    }
    for format in spec.formats {
        let out_path = spec
            .out_dir
            .join(format.output_filename(&spec.pkg, &version, &arch));
        if let Err(e) = build_file.bundle(&spec.pkg, format, &out_path) {
            eprintln!("koca: {e}");
            return 1;
        }
    }

    0
}

fn print_build_line(line: &crate::file::BuildOutputLine) {
    match line.stream {
        crate::file::BuildOutputStream::Stdout => println!("{}", line.line),
        crate::file::BuildOutputStream::Stderr => eprintln!("{}", line.line),
    }
}
