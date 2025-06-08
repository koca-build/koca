mod arch;
mod parser;
mod version;

use crate::{KocaMultiResult, KocaParserError, KocaResult};
pub use arch::Arch;
use brush::{env::EnvironmentScope, CreateOptions, Shell, ShellVariable};
use brush_parser::{ast::FunctionDefinition, word::WordPiece};
use itertools::Itertools;
use parser::DeclValue;
use std::{
    fs::{self, File},
    io::Read,
    mem,
    path::{self, Path, PathBuf},
    str::FromStr,
};
pub use version::{PkgVersion, Version};

/// Koca build file options.
pub struct BuildFileOpts {
    /// The path to the `nfpm` binary (defaults to the one on the system's `PATH`).
    pub nfpm: PathBuf,
    /// The path to the `yq` binary (defaults to the one on the system's `PATH`).
    pub yq: PathBuf,
}

impl Default for BuildFileOpts {
    fn default() -> Self {
        Self {
            nfpm: PathBuf::from("nfpm"),
            yq: PathBuf::from("yq"),
        }
    }
}

/// A package's Koca build file.
pub struct BuildFile {
    /// The [`Shell`] instance to use.
    shell: Shell,
    /// The [`BuildFileOpts`] to use.
    opts: BuildFileOpts,
    /// The package's name.
    var_pkgname: String,
    /// The package's version.
    var_version: Version,
    /// The package's architecture.
    var_arch: Vec<Arch>,
    /// The package's `build` function.
    build_func: FunctionDefinition,
    /// The package's `package` function.
    package_func: FunctionDefinition,
}

impl BuildFile {
    /// Get the [`CreateOptions`].
    fn create_options() -> CreateOptions {
        CreateOptions {
            no_profile: true,
            no_rc: true,
            do_not_inherit_env: true,
            ..Default::default()
        }
    }

    /// Get the string out of a possibly quoted string, while also making sure no expansion is present.
    fn get_piece_string(var_name: &str, piece: WordPiece) -> KocaResult<String> {
        let expansion_err = || KocaParserError::InvalidExpansion(var_name.to_string());

        match piece {
            WordPiece::Text(text) => Ok(text),
            WordPiece::SingleQuotedText(text) => Ok(text),
            WordPiece::AnsiCQuotedText(text) => Ok(text),
            WordPiece::DoubleQuotedSequence(seq) => Self::get_piece_string(
                var_name,
                seq.into_iter()
                    .exactly_one()
                    .map_err(|_| expansion_err())?
                    .piece,
            ),
            _ => Err(expansion_err().into()),
        }
    }

    /// Parse a [`DeclValue::String`] into the variables actual value, with single/double quotes removed.
    fn get_decl_string(var_name: &str, value: DeclValue) -> KocaResult<String> {
        let string_value = &value
            .as_word()
            .ok_or(KocaParserError::NotString(var_name.to_string()))?
            .value;

        let piece = brush_parser::word::parse(string_value, &Default::default())
            .unwrap()
            .into_iter()
            .exactly_one()
            .expect("Word parser should not return 2+ elements for a string")
            .piece;

        Self::get_piece_string(var_name, piece)
    }

    /// Parse a [`DeclValue`] into an `arch`.
    fn parse_arch(value: DeclValue) -> KocaMultiResult<Vec<Arch>> {
        let mut errs = vec![];
        let mut archs = vec![];

        let string_values: Vec<_> = value
            .as_array()
            .ok_or(vec![
                KocaParserError::NotArray(vars::ARCH.to_string()).into()
            ])?
            .iter()
            .map(|word| &word.value)
            .collect();

        for string_value in string_values {
            let piece = brush_parser::word::parse(string_value, &Default::default())
                .unwrap()
                .into_iter()
                .exactly_one()
                .expect("Word parser should not return 2+ elements for a string")
                .piece;

            let arch_str = match Self::get_piece_string(vars::ARCH, piece) {
                Ok(arch) => arch,
                Err(err) => {
                    errs.push(err);
                    continue;
                }
            };

            match Arch::from_str(&arch_str) {
                Ok(arch) => archs.push(arch),
                Err(_) => errs.push(KocaParserError::InvalidArch(arch_str).into()),
            }
        }

        if !errs.is_empty() {
            return Err(errs);
        }
        Ok(archs)
    }

    /// Parse a Koca build script from the reader.
    ///
    /// Returns a [`KocaError::Parser`] error if the input is an invalid script.
    pub async fn parse<R: Read>(reader: R, opts: BuildFileOpts) -> KocaMultiResult<Self> {
        // Create the shell.
        let create_options = Self::create_options();
        let shell = Shell::new(&create_options)
            .await
            .expect("shell options should be valid");
        let program = shell
            .parse(reader)
            .map_err(|err| vec![KocaParserError::from(err).into()])?;
        let decl_items = parser::get_decls(&program).map_err(|err| vec![err])?;

        // Define variables and function we need to extract.
        let mut opt_pkgname: Option<String> = None;
        let mut opt_pkgver: Option<String> = None;
        let mut opt_pkgrel: Option<String> = None;
        let mut opt_epoch: Option<String> = None;
        let mut opt_arch: Option<Vec<Arch>> = None;

        let mut opt_build_func: Option<FunctionDefinition> = None;
        let mut opt_package_func: Option<FunctionDefinition> = None;

        let mut errs = vec![];

        // Extract variables.
        for (key, value) in decl_items.vars {
            match key.as_str() {
                vars::PKGNAME => match Self::get_decl_string(vars::PKGNAME, value) {
                    Ok(pkgname) => opt_pkgname = Some(pkgname),
                    Err(err) => errs.push(err),
                },
                vars::PKGVER => match Self::get_decl_string(vars::PKGVER, value) {
                    Ok(pkgver) => opt_pkgver = Some(pkgver),
                    Err(err) => errs.push(err),
                },
                vars::PKGREL => match Self::get_decl_string(vars::PKGREL, value) {
                    Ok(pkgrel) => opt_pkgrel = Some(pkgrel),
                    Err(err) => errs.push(err),
                },
                vars::EPOCH => match Self::get_decl_string(vars::EPOCH, value) {
                    Ok(epoch) => opt_epoch = Some(epoch),
                    Err(err) => errs.push(err),
                },
                vars::ARCH => match Self::parse_arch(value) {
                    Ok(archs) => opt_arch = Some(archs),
                    Err(arch_errs) => errs.extend(arch_errs),
                },
                _ => continue,
            }
        }

        // Extract functions.
        for func in decl_items.funcs {
            match func.fname.as_str() {
                funcs::BUILD => opt_build_func = Some(func),
                funcs::PACKAGE => opt_package_func = Some(func),
                _ => continue,
            }
        }

        // Check that required variables are set.
        let required_vars = [
            (vars::PKGNAME, opt_pkgname.is_some()),
            (vars::PKGVER, opt_pkgver.is_some()),
            (vars::ARCH, opt_arch.is_some()),
        ];

        for (var_name, is_set) in required_vars {
            if !is_set {
                errs.push(KocaParserError::MissingRequiredVariable(var_name.to_string()).into());
            }
        }

        // Check that required functions are set.
        let required_funcs = [
            (funcs::BUILD, opt_build_func.is_some()),
            (funcs::PACKAGE, opt_package_func.is_some()),
        ];

        for (func_name, is_set) in required_funcs {
            if !is_set {
                errs.push(KocaParserError::MissingRequiredFunction(func_name.to_string()).into());
            }
        }

        // TODO: We need to handle this better so the user knows if the epoch/pkgrel itself is invalid.
        let parsed_version = if let Some(mut pkgver) = opt_pkgver {
            if let Some(epoch) = opt_epoch {
                pkgver = format!("{epoch}:{pkgver}");
            }
            if let Some(pkgrel) = opt_pkgrel {
                pkgver = format!("{pkgver}-{pkgrel}");
            }
            match Version::from_str(&pkgver) {
                Ok(version) => Some(version),
                Err(_) => {
                    errs.push(KocaParserError::InvalidVersion(pkgver).into());
                    None
                }
            }
        } else {
            None
        };

        // Return errors if any, otherwise return the parsed build file.
        if !errs.is_empty() {
            return Err(errs);
        }

        Ok(Self {
            shell: shell,
            opts: opts,
            var_pkgname: opt_pkgname.expect("pkgname should be set"),
            var_version: parsed_version.expect("version should be valid by this point"),
            var_arch: opt_arch.expect("arch should be set"),
            build_func: opt_build_func.expect("build function should be set"),
            package_func: opt_package_func.expect("package function should be set"),
        })
    }

    /// Read a Koca build script from the input file.
    ///
    /// Returns a:
    /// - [`KocaError::Parser`] error if the input is an invalid script.
    /// - [`KocaError::IO`] error if the input file can't be read.
    pub async fn parse_file<P: AsRef<Path>>(path: P, opts: BuildFileOpts) -> KocaMultiResult<Self> {
        let file = File::open(path).map_err(|err| vec![err.into()])?;
        Self::parse(file, opts).await
    }

    /// Run the `build` function of the package.
    ///
    /// Returns a:
    /// - [`KocaError::IO`] if the build directory couldn't be created.
    /// - [`KocaError::Func`] if the `build` function failed to execute.
    pub async fn run_build(&mut self) -> KocaResult<()> {
        self.shell.funcs.clear();
        self.shell.funcs.update(
            self.build_func.fname.clone(),
            self.build_func.clone().into(),
        );

        let existing_dir = mem::replace(&mut self.shell.working_dir, dirs::SRC.into());
        fs::create_dir_all(dirs::SRC)?;

        self.shell
            .invoke_function(&self.build_func.fname, &[])
            .await?;
        self.shell.working_dir = existing_dir;

        Ok(())
    }

    /// Run the `package` function of the package.
    /// /// Returns a:
    /// - [`KocaError::IO`] if the package directory couldn't be created.
    /// - [`KocaError::Func`] if the `package` function failed to execute.
    pub async fn run_package(&mut self) -> KocaResult<()> {
        self.shell.funcs.clear();
        self.shell.funcs.update(
            self.package_func.fname.clone(),
            self.package_func.clone().into(),
        );

        let existing_dir = mem::replace(&mut self.shell.working_dir, dirs::SRC.into());
        fs::create_dir_all(dirs::PKG)?;

        let absolute_pkgdir = path::absolute(dirs::PKG)
            .expect("directory should exist at this point")
            .to_string_lossy()
            .into_owned();
        self.shell
            .env
            .add(
                "pkgdir",
                ShellVariable::new(absolute_pkgdir.into()),
                EnvironmentScope::Global,
            )
            .expect("shell adding shouldn't fail");

        self.shell
            .invoke_function(&self.package_func.fname, &[])
            .await?;
        self.shell.working_dir = existing_dir;

        Ok(())
    }

    /// Run

    /// Get the package's name.
    pub fn pkgname(&self) -> &str {
        &self.var_pkgname
    }

    /// Get the package's version.
    pub fn version(&self) -> &Version {
        &self.var_version
    }

    /// Get the package's architecture.
    pub fn arch(&self) -> &[Arch] {
        &self.var_arch
    }
}

/// A mapping of `const` variable names to their stringified values.
pub mod vars {
    pub const PKGNAME: &str = "pkgname";
    pub const PKGVER: &str = "pkgver";
    pub const PKGREL: &str = "pkgrel";
    pub const EPOCH: &str = "epoch";
    pub const ARCH: &str = "arch";
}

/// A mapping of `const` function names to their stringified values.
pub mod funcs {
    pub const BUILD: &str = "build";
    pub const PACKAGE: &str = "package";
}

/// The directories used by Koca.
mod dirs {
    /// The directory where Koca stores source files.
    pub const SRC: &str = "koca/src";
    /// The directory where Koca stores package files.
    pub const PKG: &str = "koca/pkg";
}
