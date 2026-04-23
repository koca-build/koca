use koca::{
    backend::Backend,
    distro::Distro,
    resolve::{native_names, resolve_deps},
    BuildFile,
};
use koca_proto::{Command, InstalledStatus, ResultPayload};
use std::{io, str::FromStr};
use tokio::io::{AsyncBufReadExt, BufReader};

use crate::{
    cli::CreateArgs,
    error::{CliError, CliMultiError, CliMultiResult},
    tui::{CreateUi, KocaCreateCli, KocaCreateTui},
};

fn ke(e: koca::KocaError) -> CliMultiError {
    CliMultiError::from(CliError::Koca { err: e })
}

fn spawn_line_reader(
    reader: impl tokio::io::AsyncRead + Unpin + Send + 'static,
    tx: tokio::sync::mpsc::UnboundedSender<String>,
) {
    let mut lines = BufReader::new(reader).lines();
    tokio::spawn(async move {
        while let Ok(Some(line)) = lines.next_line().await {
            let _ = tx.send(line);
        }
    });
}

pub async fn run(args: CreateArgs) -> CliMultiResult<()> {
    let mut ui: Box<dyn CreateUi> = if std::io::IsTerminal::is_terminal(&io::stdout()) {
        Box::new(KocaCreateTui::new()?)
    } else {
        Box::new(KocaCreateCli::new())
    };

    let result = run_inner(&args, ui.as_mut()).await;

    ui.cleanup();

    result
}

async fn run_inner(args: &CreateArgs, ui: &mut dyn CreateUi) -> CliMultiResult<()> {
    let mut build_file = BuildFile::parse_file(&args.build_file)
        .await
        .map_err(|errs| {
            CliMultiError(errs.into_iter().map(|err| CliError::Koca { err }).collect())
        })?;

    let depends = build_file.depends().to_vec();
    let makedepends = build_file.makedepends().to_vec();

    let distro = if let Some(target) = &args.target {
        Distro::from_str(target).map_err(ke)?
    } else {
        Distro::detect().map_err(ke)?
    };

    let repo = distro.repology_repo();
    let backend_bin = distro.backend_binary();

    let mut newly_installed: Vec<String> = Vec::new();
    let total_download_bytes: u64;
    let installed_count: u32;

    if !makedepends.is_empty() || !depends.is_empty() {
        let resolved_makedeps = if makedepends.is_empty() {
            vec![]
        } else {
            ui.start_resolve()?;

            let mut ticker = tokio::time::interval(std::time::Duration::from_millis(80));
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

            let mut resolve_fut = std::pin::pin!(resolve_deps(&makedepends, &repo));
            loop {
                tokio::select! {
                    result = &mut resolve_fut => break result.map_err(ke)?,
                    _ = ticker.tick() => { ui.tick()?; }
                }
            }
        };

        let makedep_natives = native_names(&resolved_makedeps);

        if !makedep_natives.is_empty() {
            // Build a map from native package name → original constraint for
            // version satisfaction checks.
            let native_to_constraint: std::collections::HashMap<&str, &koca::dep::DepConstraint> =
                resolved_makedeps
                    .iter()
                    .flat_map(|r| r.native_names.iter().map(|n| (n.as_str(), &r.constraint)))
                    .collect();

            let mut check_backend = Backend::spawn(backend_bin, false).await.map_err(ke)?;

            let check_result = check_backend
                .call(Command::CheckInstalled {
                    packages: makedep_natives.clone(),
                })
                .await
                .map_err(ke)?;

            let statuses = match check_result {
                ResultPayload::CheckInstalled { packages } => packages,
                _ => unreachable!(),
            };

            let missing: Vec<String> = statuses
                .iter()
                .filter(|s| {
                    if s.status == InstalledStatus::Missing {
                        return true;
                    }
                    // Installed — check if the version satisfies the constraint.
                    if let (Some(ver), Some(constraint)) =
                        (&s.version, native_to_constraint.get(s.name.as_str()))
                    {
                        return !constraint.satisfied_by(ver);
                    }
                    false
                })
                .map(|s| s.name.clone())
                .collect();

            if !missing.is_empty() {
                let plan_result = check_backend
                    .call(Command::InstallPlan {
                        packages: missing.clone(),
                    })
                    .await
                    .map_err(ke)?;

                let (actions, plan_download) = match plan_result {
                    ResultPayload::InstallPlan {
                        actions,
                        total_download,
                        ..
                    } => (actions, total_download),
                    _ => unreachable!(),
                };

                check_backend.shutdown().await.map_err(ke)?;

                let confirmed = ui.show_confirm(&actions, &depends, args.noconfirm)?;

                if !confirmed {
                    return Ok(());
                }

                ui.suspend()?;
                let mut sudo_backend = Backend::spawn(backend_bin, true).await.map_err(ke)?;
                ui.resume()?;

                sudo_backend
                    .call(Command::InstallPlan {
                        packages: missing.clone(),
                    })
                    .await
                    .map_err(ke)?;

                let result = sudo_backend
                    .call_streaming(Command::Confirm, |event| match event {
                        None => {
                            ui.tick().ok();
                        }
                        Some(ev) => {
                            ui.on_event(ev).ok();
                        }
                    })
                    .await
                    .map_err(ke)?;

                if let ResultPayload::Install { installed, .. } = &result {
                    newly_installed = installed.clone();
                }

                total_download_bytes = plan_download;
                installed_count = actions.len() as u32;
                ui.finish_install(total_download_bytes, installed_count)?;

                sudo_backend.shutdown().await.map_err(ke)?;
            } else {
                check_backend.shutdown().await.map_err(ke)?;
            }
        }
    }

    ui.start_build()?;

    let build_result = build_file
        .run_build_with_output(|line| match line {
            Some(line) => {
                ui.on_build_line(&line.line).ok();
            }
            None => {
                ui.tick().ok();
            }
        })
        .await;

    if let Err(err) = build_result {
        ui.show_failure("build")?;
        return Err(CliError::Koca { err }.into());
    }

    let pkgname = build_file.pkgname().to_string();
    let version = build_file.version().to_string();
    ui.finish_build(&pkgname, &version)?;

    let output_type_str = args.output_type.as_str();

    ui.start_package()?;

    let exe = std::env::current_exe().map_err(|err| CliError::Io { err })?;
    let mut child = tokio::process::Command::new("fakeroot")
        .arg(&exe)
        .arg("internal")
        .arg("package")
        .arg(&args.build_file)
        .arg("--output-type")
        .arg(output_type_str)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|err| {
            if err.kind() == std::io::ErrorKind::NotFound {
                CliError::FakerootNotFound
            } else {
                CliError::Io { err }
            }
        })?;

    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    let mut ticker = tokio::time::interval(std::time::Duration::from_millis(80));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    spawn_line_reader(stdout, tx.clone());
    spawn_line_reader(stderr, tx);

    loop {
        ticker.tick().await;
        ui.tick()?;

        let mut done = false;
        loop {
            match rx.try_recv() {
                Ok(line) => {
                    ui.on_package_line(&line).ok();
                }
                Err(tokio::sync::mpsc::error::TryRecvError::Empty) => break,
                Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => {
                    done = true;
                    break;
                }
            }
        }
        if done {
            break;
        }
    }

    let status = child.wait().await.map_err(|err| CliError::Io { err })?;

    if !status.success() {
        ui.show_failure("package")?;
        return Err(CliError::PackageFailed.into());
    }

    let arch = build_file.arch()[0].clone();
    let output_files: Vec<String> = args
        .output_type
        .bundle_formats()
        .iter()
        .map(|fmt| format!("./{}", fmt.output_filename(&pkgname, &version, &arch)))
        .collect();

    ui.finish_package(&output_files.join(", "))?;

    if args.rm_deps && !newly_installed.is_empty() {
        zolt::infoln!("Removing {} makedepend(s)...", newly_installed.len());
        let mut rm_backend = Backend::spawn(backend_bin, true).await.map_err(ke)?;

        rm_backend
            .call_streaming(
                Command::Remove {
                    packages: newly_installed,
                },
                |event| match event {
                    None => {
                        ui.tick().ok();
                    }
                    Some(ev) => {
                        ui.on_event(ev).ok();
                    }
                },
            )
            .await
            .map_err(ke)?;

        rm_backend.shutdown().await.map_err(ke)?;
    }

    Ok(())
}
