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

    // Run `package`.
    zolt::infoln!("Running {} stage...", koca::funcs::PACKAGE.bold().blue());
    if let Err(err) = build_file
        .run_package_with_output(|line| {
            if let Some(line) = line {
                print_build_output(line);
            }
        })
        .await
    {
        return Err(CliError::Koca { err }.into());
    }

    // Run the bundle stage.
    for bundle_format in args.output_type.bundle_formats() {
        let arch = build_file.arch()[0].clone();
        let file_name = bundle_format.output_filename(
            build_file.pkgname(),
            &build_file.version().to_string(),
            &arch,
        );

        zolt::infoln!(
            "Creating package into {}{}...",
            "./".blue().bold(),
            file_name.blue().bold()
        );

        if let Err(err) = build_file
            .bundle(bundle_format, Path::new(&file_name))
            .await
        {
            return Err(CliError::Koca { err }.into());
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
