//! The system-side package-manager actor.
//!
//! [`PackageManager`] is the counterpart to [`crate::BuildFile`]: where the
//! build file is the *recipe*, this is the *actor* that touches the system's
//! package database. It resolves dependencies, installs them, and can remove
//! exactly what it installed. It is privilege-agnostic — root is obtained per
//! mutation through [`DependencyHandler::elevate`], never by spawning `sudo`.

use std::collections::HashSet;

use crate::backend::{
    dispatch_check_installed, dispatch_install_plan, Backend, BackendKind, Command,
    InstalledStatus, PlannedAction, ProtocolError, ResultPayload,
};
use crate::distro::Distro;
use crate::handler::DependencyHandler;
use crate::{KocaError, KocaResult};

/// A resolved set of package actions plus their aggregate sizes.
///
/// Returned by [`PackageManager::resolve`]. It is pure data: the consumer
/// inspects it (e.g. to render a confirmation prompt) and decides whether to
/// pass it to [`PackageManager::install`]. The library never prompts.
pub struct Plan {
    /// Every action the package manager will take, for display.
    pub actions: Vec<PlannedAction>,
    /// Total bytes to download.
    pub total_download: u64,
    /// Total bytes installed on disk.
    pub total_install: u64,
    /// The package names to hand the backend at install time (what resolution
    /// found missing). Kept private so the plan stays a faithful description.
    missing: Vec<String>,
}

impl Plan {
    /// Whether there is nothing to install.
    pub fn is_empty(&self) -> bool {
        self.actions.is_empty()
    }

    fn empty() -> Self {
        Self {
            actions: Vec::new(),
            total_download: 0,
            total_install: 0,
            missing: Vec::new(),
        }
    }
}

/// Drives dependency resolution, installation, and removal against the system
/// package manager, owning the install receipt so removal undoes exactly what
/// it installed.
pub struct PackageManager {
    kind: BackendKind,
    /// Names this manager has installed, for `remove_installed`.
    installed: Vec<String>,
}

impl PackageManager {
    /// Detect the package manager from the running distro.
    pub fn detect() -> KocaResult<Self> {
        Ok(Self::for_distro(&Distro::detect()?))
    }

    /// Use the package manager appropriate for `distro` (honors `--target`).
    pub fn for_distro(distro: &Distro) -> Self {
        Self {
            kind: distro.backend_kind(),
            installed: Vec::new(),
        }
    }

    /// Resolve `deps` into a [`Plan`].
    ///
    /// Runs entirely unprivileged and in-process: the simulate/query commands
    /// (`apt-get -s`, `pacman -Sp`, `dpkg-query`, …) need no root. Checks what
    /// is already installed, then asks the package manager to plan the rest.
    pub async fn resolve(
        &mut self,
        deps: Vec<rfpm::relation::Relation>,
        handler: &mut impl DependencyHandler,
    ) -> KocaResult<Plan> {
        handler.on_resolve_start();
        let result = self.resolve_inner(deps).await;
        handler.on_resolve_end();
        result
    }

    async fn resolve_inner(&self, deps: Vec<rfpm::relation::Relation>) -> KocaResult<Plan> {
        // Unique package names, declaration order preserved.
        let mut seen = HashSet::new();
        let names: Vec<String> = deps
            .iter()
            .map(|d| d.name.clone())
            .filter(|n| seen.insert(n.clone()))
            .collect();
        if names.is_empty() {
            return Ok(Plan::empty());
        }

        let statuses = match self.check_installed(names).await? {
            ResultPayload::CheckInstalled { packages } => packages,
            _ => unreachable!("check-installed returns a CheckInstalled payload"),
        };
        let missing: Vec<String> = statuses
            .into_iter()
            .filter(|s| s.status == InstalledStatus::Missing)
            .map(|s| s.name)
            .collect();
        if missing.is_empty() {
            return Ok(Plan::empty());
        }

        let (actions, total_download, total_install) =
            match self.install_plan(missing.clone()).await? {
                ResultPayload::InstallPlan {
                    actions,
                    total_download,
                    total_install,
                } => (actions, total_download, total_install),
                _ => unreachable!("install-plan returns an InstallPlan payload"),
            };

        Ok(Plan {
            actions,
            total_download,
            total_install,
            missing,
        })
    }

    async fn check_installed(&self, names: Vec<String>) -> KocaResult<ResultPayload> {
        // Copy the field out: a `move` closure that named `self.kind` would
        // capture the `&self` borrow, which can't outlive into `'static`.
        let kind = self.kind;
        tokio::task::spawn_blocking(move || dispatch_check_installed(kind, &names))
            .await
            .map_err(|e| KocaError::IO(std::io::Error::other(e.to_string())))?
            .map_err(proto_err)
    }

    async fn install_plan(&self, names: Vec<String>) -> KocaResult<ResultPayload> {
        let kind = self.kind;
        tokio::task::spawn_blocking(move || dispatch_install_plan(kind, &names).map(|(r, _)| r))
            .await
            .map_err(|e| KocaError::IO(std::io::Error::other(e.to_string())))?
            .map_err(proto_err)
    }

    /// Install everything in `plan`, recording the installed names as a receipt
    /// so [`remove_installed`](Self::remove_installed) can undo them. Privilege
    /// is obtained through `handler.elevate`.
    pub async fn install(
        &mut self,
        plan: &Plan,
        handler: &mut impl DependencyHandler,
    ) -> KocaResult<()> {
        if plan.is_empty() {
            return Ok(());
        }
        // Both counts come from the plan: every action is installed; the subset
        // with bytes to fetch is downloaded.
        let installs = plan.actions.len() as u32;
        let downloads = plan.actions.iter().filter(|a| a.download_size > 0).count() as u32;
        handler.on_install_start(downloads, installs);
        let mut backend = Backend::connect_elevated(self.kind, handler).await?;
        let result = backend
            .call_streaming(
                Command::Install {
                    packages: plan.missing.clone(),
                },
                handler,
            )
            .await;
        // Tear the backend down regardless of the streaming outcome.
        let _ = backend.shutdown().await;
        if let ResultPayload::Install { installed, .. } = result? {
            self.installed.extend(installed);
        }
        handler.on_install_end();
        Ok(())
    }

    /// Remove exactly the packages this manager installed (for `--rm-deps`).
    /// A no-op if nothing was installed.
    pub async fn remove_installed(&mut self, handler: &mut impl DependencyHandler) -> KocaResult<()> {
        if self.installed.is_empty() {
            return Ok(());
        }
        let packages = std::mem::take(&mut self.installed);
        handler.on_remove_start(packages.len() as u32);
        let mut backend = Backend::connect_elevated(self.kind, handler).await?;
        let result = backend
            .call_streaming(Command::Remove { packages }, handler)
            .await;
        let _ = backend.shutdown().await;
        result?;
        handler.on_remove_end();
        Ok(())
    }

    /// The names this manager has installed so far (the receipt).
    pub fn installed(&self) -> &[String] {
        &self.installed
    }
}

fn proto_err(e: ProtocolError) -> KocaError {
    KocaError::IO(std::io::Error::other(e.to_string()))
}
