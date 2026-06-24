use koca::{
    backend::{Backend, Command, InstalledStatus, ResultPayload},
    distro::Distro,
    source::{fetch_source, SourceProgress, SourceProgressState},
    BuildFile,
};
use std::str::FromStr;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncBufReadExt, BufReader};

use crate::{
    cli::CreateArgs,
    error::{CliError, CliMultiError, CliMultiResult},
    tui::{CreateUi, KocaCreateUi},
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
    let mut ui: Box<dyn CreateUi> = Box::new(KocaCreateUi::new()?);

    let result = run_inner(&args, ui.as_mut()).await;

    ui.cleanup();

    result
}

async fn run_inner(args: &CreateArgs, ui: &mut dyn CreateUi) -> CliMultiResult<()> {
    let build_file_path = match &args.build_file {
        Some(p) => p.clone(),
        None => crate::discover::find_build_file()?,
    };

    let mut build_file = BuildFile::parse_file(&build_file_path)
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

    let backend_kind = distro.backend_kind();

    let mut newly_installed: Vec<String> = Vec::new();
    let total_download_bytes: u64;
    let installed_count: u32;

    if !makedepends.is_empty() || !depends.is_empty() {
        let makedep_natives: Vec<String> = makedepends.iter().map(|d| d.name.clone()).collect();

        if !makedep_natives.is_empty() {
            ui.start_resolve()?;
            let mut resolve_ticker = tokio::time::interval(std::time::Duration::from_millis(80));
            resolve_ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

            let mut check_backend = Backend::spawn(backend_kind, false).await.map_err(ke)?;

            let check_result = {
                let fut = check_backend.call(Command::CheckInstalled {
                    packages: makedep_natives.clone(),
                });
                tokio::pin!(fut);
                loop {
                    tokio::select! {
                        _ = resolve_ticker.tick() => { ui.tick().ok(); }
                        result = &mut fut => break result.map_err(ke)?,
                    }
                }
            };

            let statuses = match check_result {
                ResultPayload::CheckInstalled { packages } => packages,
                _ => unreachable!(),
            };

            // Hand the missing names to the native resolver (apt/alpm), which
            // does the actual dependency and version resolution.
            let missing: Vec<String> = statuses
                .iter()
                .filter(|s| s.status == InstalledStatus::Missing)
                .map(|s| s.name.clone())
                .collect();

            if !missing.is_empty() {
                let plan_result = {
                    let fut = check_backend.call(Command::InstallPlan {
                        packages: missing.clone(),
                    });
                    tokio::pin!(fut);
                    loop {
                        tokio::select! {
                            _ = resolve_ticker.tick() => { ui.tick().ok(); }
                            result = &mut fut => break result.map_err(ke)?,
                        }
                    }
                };

                let (actions, plan_download) = match plan_result {
                    ResultPayload::InstallPlan {
                        actions,
                        total_download,
                        ..
                    } => (actions, total_download),
                    _ => unreachable!(),
                };

                check_backend.shutdown().await.map_err(ke)?;
                ui.finish_resolve()?;

                let confirmed = ui.show_confirm(&actions, &depends, args.noconfirm)?;

                if !confirmed {
                    return Ok(());
                }

                ui.suspend()?;
                let mut sudo_backend = Backend::spawn(backend_kind, true).await.map_err(ke)?;
                ui.resume()?;

                let result = sudo_backend
                    .call_streaming(
                        Command::Install {
                            packages: missing.clone(),
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

                if let ResultPayload::Install { installed, .. } = &result {
                    newly_installed = installed.clone();
                }

                total_download_bytes = plan_download;
                installed_count = actions.len() as u32;
                ui.finish_install(total_download_bytes, installed_count)?;

                sudo_backend.shutdown().await.map_err(ke)?;
            } else {
                check_backend.shutdown().await.map_err(ke)?;
                ui.finish_resolve()?;
            }
        }
    }

    let arch = build_file.arch()[0].clone();
    let sources = build_file.sources(&arch).to_vec();

    if !sources.is_empty() {
        let srcdir = std::path::Path::new("koca-build/src");
        std::fs::create_dir_all(srcdir).map_err(|err| CliError::Io { err })?;

        let display_urls: Vec<String> = sources.iter().map(|s| s.display_url()).collect();
        let progress: SourceProgressState = Arc::new(Mutex::new(
            sources.iter().map(|_| SourceProgress::new()).collect(),
        ));

        ui.redraw_sources(&progress.lock().unwrap(), &display_urls)?;

        let mut handles = Vec::new();
        for (i, source) in sources.iter().enumerate() {
            let source = source.clone();
            let srcdir = srcdir.to_path_buf();
            let progress = Arc::clone(&progress);
            handles.push(tokio::spawn(async move {
                fetch_source(&source, &srcdir, i, &progress).await
            }));
        }

        let mut ticker = tokio::time::interval(std::time::Duration::from_millis(80));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        let all_done = async {
            let mut errors = Vec::new();
            for handle in handles {
                match handle.await {
                    Ok(Err(e)) => errors.push(e),
                    Err(join_err) => errors.push(join_err.to_string()),
                    Ok(Ok(())) => {}
                }
            }
            errors
        };
        tokio::pin!(all_done);

        let fetch_errors = loop {
            tokio::select! {
                _ = ticker.tick() => {
                    ui.tick().ok();
                    ui.redraw_sources(&progress.lock().unwrap(), &display_urls)?;
                }
                errors = &mut all_done => break errors,
            }
        };

        ui.finish_sources(&progress.lock().unwrap(), &display_urls)?;

        if !fetch_errors.is_empty() {
            return Err(CliError::Koca {
                err: koca::KocaError::InvalidSource(format!(
                    "{} source(s) failed to fetch",
                    fetch_errors.len()
                )),
            }
            .into());
        }
    }

    let pkgbase = build_file.pkgbase().to_string();
    let version = build_file.version().to_string();

    if build_file.has_build() {
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

        ui.finish_build(&pkgbase, &version)?;
    }

    let output_type_str = args.output_type.as_str();

    ui.start_package()?;

    let exe = std::env::current_exe().map_err(|err| CliError::Io { err })?;
    let mut child = tokio::process::Command::new("fakeroot")
        .arg(&exe)
        .arg("internal")
        .arg("package")
        .arg(&build_file_path)
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

    let status = loop {
        tokio::select! {
            _ = ticker.tick() => {
                ui.tick()?;
            }
            maybe_line = rx.recv() => {
                match maybe_line {
                    Some(line) => {
                        ui.on_package_line(&line).ok();
                    }
                    None => {
                        break child.wait().await.map_err(|err| CliError::Io { err })?;
                    }
                }
            }
        }
    };

    if !status.success() {
        ui.show_failure("package")?;
        return Err(CliError::PackageFailed.into());
    }

    let mut output_files = Vec::new();
    for name in build_file.pkgnames() {
        for fmt in args.output_type.bundle_formats() {
            output_files.push(format!(
                "koca-out/{}",
                fmt.output_filename(name, &version, &arch)
            ));
        }
    }

    ui.finish_package(&output_files.join(", "))?;

    if args.rm_deps && !newly_installed.is_empty() {
        zolt::infoln!("Removing {} makedepend(s)...", newly_installed.len());
        let mut rm_backend = Backend::spawn(backend_kind, true).await.map_err(ke)?;

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
