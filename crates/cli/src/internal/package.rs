use std::path::Path;

use koca::{BuildFile, BuildOutputLine, BuildOutputStream};

use crate::{
    cli::PackageArgs,
    error::{CliError, CliMultiError, CliMultiResult},
};
use zolt::Colorize;

pub async fn run(args: PackageArgs) -> CliMultiResult<()> {
    let mut build_file = match BuildFile::parse_file(&args.build_file).await {
        Ok(file) => file,
        Err(errs) => {
            return Err(CliMultiError(
                errs.into_iter().map(|err| CliError::Koca { err }).collect(),
            ))
        }
    };

    // Determine which packages to process.
    let pkg_names: Vec<String> = if args.package.is_empty() {
        build_file.pkgnames().to_vec()
    } else {
        args.package.clone()
    };

    std::fs::create_dir_all("koca-out").map_err(|err| CliError::Io { err })?;

    for pkg_name in &pkg_names {
        // Run the package function.
        let func_label = if build_file.pkgnames().len() > 1 {
            format!("{}:{}", koca::funcs::PACKAGE, pkg_name)
        } else {
            koca::funcs::PACKAGE.to_string()
        };
        zolt::infoln!("Running {} stage...", func_label.bold().blue());

        if let Err(err) = build_file
            .run_package_for_with_output(pkg_name, |line| {
                if let Some(line) = line {
                    print_build_output(line);
                }
            })
            .await
        {
            return Err(CliError::Koca { err }.into());
        }

        // Bundle into each output format.
        for bundle_format in args.output_type.bundle_formats() {
            let arch = build_file.arch()[0].clone();
            let file_name =
                bundle_format.output_filename(pkg_name, &build_file.version().to_string(), &arch);
            let out_path = Path::new("koca-out").join(&file_name);

            zolt::infoln!(
                "Creating package into {}{}...",
                "koca-out/".blue().bold(),
                file_name.blue().bold()
            );

            if let Err(err) = build_file.bundle(pkg_name, bundle_format, &out_path).await {
                return Err(CliError::Koca { err }.into());
            }
        }
    }

    zolt::infoln!("Package(s) created successfully.");

    Ok(())
}

fn print_build_output(line: BuildOutputLine) {
    match line.stream {
        BuildOutputStream::Stdout => println!("{}", line.line),
        BuildOutputStream::Stderr => eprintln!("{}", line.line),
    }
}
