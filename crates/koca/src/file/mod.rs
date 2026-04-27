mod arch;
mod parser;
mod version;

use crate::{
    nfpm::{self, NfpmConfig},
    KocaError, KocaMultiResult, KocaParserError, KocaResult,
};
pub use arch::Arch;
use brush::{CreateOptions, Shell, ShellVariable};
use brush_parser::{ast::FunctionDefinition, word::WordPiece};
use itertools::Itertools;
use nfpm_sys::NfpmError;
use parser::DeclValue;
use std::{
    collections::HashMap,
    env, fmt,
    fs::{self, File},
    io::{BufRead, BufReader, Read},
    path::{self, Path},
    str::FromStr,
};
use tokio::sync::mpsc;
pub use version::{PkgVersion, Version};

/// The output bundle format.
pub enum BundleFormat {
    /// A `.deb` package.
    Deb,
    /// A `.rpm` package.
    Rpm,
}

impl BundleFormat {
    /// File extension for this bundle format (also the nfpm format string).
    pub fn extension(&self) -> &'static str {
        match self {
            BundleFormat::Deb => "deb",
            BundleFormat::Rpm => "rpm",
        }
    }

    /// Return the architecture string appropriate for this format.
    pub fn arch_string(&self, arch: &Arch) -> &'static str {
        match self {
            BundleFormat::Deb => arch.get_deb_string(),
            BundleFormat::Rpm => arch.get_rpm_string(),
        }
    }

    /// Build the output filename for a package in this format.
    pub fn output_filename(&self, pkgname: &str, version: &str, arch: &Arch) -> String {
        format!(
            "{}_{}_{}.{}",
            pkgname,
            version,
            self.arch_string(arch),
            self.extension()
        )
    }
}

/// A Koca build file function.
#[derive(Debug)]
pub enum KocaFunction {
    /// The `build` function.
    Build,
    /// The `package` function.
    Package,
}

impl fmt::Display for KocaFunction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            KocaFunction::Build => write!(f, "build"),
            KocaFunction::Package => write!(f, "package"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuildOutputStream {
    Stdout,
    Stderr,
}

#[derive(Debug, Clone)]
pub struct BuildOutputLine {
    pub stream: BuildOutputStream,
    pub line: String,
}

/// Whether the build file describes a single package or a split (multi-package) build.
pub enum PackageKind {
    /// A single package with just `pkgname`.
    Single(String),
    /// A split build with `pkgbase` and an array of `pkgname`s.
    Split { base: String, names: Vec<String> },
}

impl PackageKind {
    /// Return all package names.
    pub fn names(&self) -> &[String] {
        match self {
            PackageKind::Single(name) => std::slice::from_ref(name),
            PackageKind::Split { names, .. } => names,
        }
    }

    /// Return the pkgbase. For single packages this is the pkgname.
    pub fn base(&self) -> &str {
        match self {
            PackageKind::Single(name) => name,
            PackageKind::Split { base, .. } => base,
        }
    }
}

/// A package's Koca build file.
pub struct BuildFile {
    /// The [`Shell`] instance to use.
    shell: Shell,
    /// The raw list of defined variables.
    vars: HashMap<String, DeclValue>,
    /// Single vs. split package metadata.
    packages: PackageKind,
    /// The package's version.
    var_version: Version,
    /// The package's architecture.
    var_arch: Vec<Arch>,
    /// The package's description.
    var_pkgdesc: String,
    /// The package's runtime dependencies (repology project names + optional version constraints).
    var_depends: Vec<crate::dep::DepConstraint>,
    /// The package's build-time dependencies (repology project names + optional version constraints).
    var_makedepends: Vec<crate::dep::DepConstraint>,
    /// The package's `build` function.
    build_func: FunctionDefinition,
    /// Package functions keyed by package name.
    /// Single: one entry. Split: one per pkgname element.
    package_funcs: Vec<(String, FunctionDefinition)>,
}

impl BuildFile {
    /// Get the [`CreateOptions`].
    fn create_options() -> CreateOptions {
        CreateOptions {
            no_profile: true,
            no_rc: true,
            do_not_inherit_env: true,
            builtins: brush_builtins::default_builtins(brush_builtins::BuiltinSet::BashMode),
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
    fn get_decl_string(var_name: &str, value: &DeclValue) -> KocaResult<String> {
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

    fn parse_string_array(var_name: &str, value: &DeclValue) -> KocaMultiResult<Vec<String>> {
        let mut errs = vec![];
        let mut result = vec![];

        for string_value in value
            .as_array()
            .ok_or(vec![KocaParserError::NotArray(var_name.to_string()).into()])?
            .iter()
            .map(|word| &word.value)
        {
            let piece = brush_parser::word::parse(string_value, &Default::default())
                .unwrap()
                .into_iter()
                .exactly_one()
                .expect("Word parser should not return 2+ elements for a string")
                .piece;

            match Self::get_piece_string(var_name, piece) {
                Ok(s) => result.push(s),
                Err(err) => errs.push(err),
            }
        }

        if !errs.is_empty() {
            return Err(errs);
        }
        Ok(result)
    }

    /// Parse a string array into `Vec<DepConstraint>`, propagating parse errors.
    fn parse_dep_array(
        var_name: &str,
        value: &DeclValue,
    ) -> KocaMultiResult<Vec<crate::dep::DepConstraint>> {
        let strings = Self::parse_string_array(var_name, value)?;
        let mut errs = vec![];
        let mut result = vec![];
        for s in strings {
            match crate::dep::DepConstraint::parse(&s) {
                Ok(dep) => result.push(dep),
                Err(err) => errs.push(err),
            }
        }
        if !errs.is_empty() {
            return Err(errs);
        }
        Ok(result)
    }

    /// Parse a [`DeclValue`] into an `arch`.
    fn parse_arch(value: &DeclValue) -> KocaMultiResult<Vec<Arch>> {
        let mut errs = vec![];
        let mut archs = vec![];

        let strings = Self::parse_string_array(vars::ARCH, value)?;
        for arch_str in strings {
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
    pub async fn parse<R: Read>(reader: R) -> KocaMultiResult<Self> {
        // Create the shell.
        let create_options = Self::create_options();
        let shell = Shell::new(create_options)
            .await
            .expect("shell options should be valid");
        let program = shell
            .parse(reader)
            .map_err(|err| vec![KocaParserError::from(err).into()])?;
        let decl_items = parser::get_decls(&program).map_err(|err| vec![err])?;

        // Define variables and function we need to extract.
        let mut opt_pkgbase: Option<String> = None;
        let mut opt_pkgname_single: Option<String> = None;
        let mut opt_pkgname_array: Option<Vec<String>> = None;
        let mut opt_pkgver: Option<String> = None;
        let mut opt_pkgrel: Option<String> = None;
        let mut opt_epoch: Option<String> = None;
        let mut opt_arch: Option<Vec<Arch>> = None;
        let mut opt_pkgdesc: Option<String> = None;
        let mut opt_depends: Vec<crate::dep::DepConstraint> = vec![];
        let mut opt_makedepends: Vec<crate::dep::DepConstraint> = vec![];

        let mut opt_build_func: Option<FunctionDefinition> = None;
        let mut single_package_func: Option<FunctionDefinition> = None;
        let mut split_package_funcs: HashMap<String, FunctionDefinition> = HashMap::new();

        let mut errs = vec![];

        // Extract variables.
        for (key, value) in &decl_items.vars {
            match key.as_str() {
                vars::PKGBASE => match Self::get_decl_string(vars::PKGBASE, value) {
                    Ok(pkgbase) => opt_pkgbase = Some(pkgbase),
                    Err(err) => errs.push(err),
                },
                vars::PKGNAME => match value {
                    DeclValue::Array(_) => match Self::parse_string_array(vars::PKGNAME, value) {
                        Ok(names) => opt_pkgname_array = Some(names),
                        Err(arr_errs) => errs.extend(arr_errs),
                    },
                    DeclValue::String(_) => match Self::get_decl_string(vars::PKGNAME, value) {
                        Ok(name) => opt_pkgname_single = Some(name),
                        Err(err) => errs.push(err),
                    },
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
                vars::PKGDESC => match Self::get_decl_string(vars::PKGDESC, value) {
                    Ok(pkgdesc) => opt_pkgdesc = Some(pkgdesc),
                    Err(err) => errs.push(err),
                },
                vars::DEPENDS => match Self::parse_dep_array(vars::DEPENDS, value) {
                    Ok(depends) => opt_depends = depends,
                    Err(dep_errs) => errs.extend(dep_errs),
                },
                vars::MAKEDEPENDS => match Self::parse_dep_array(vars::MAKEDEPENDS, value) {
                    Ok(makedepends) => opt_makedepends = makedepends,
                    Err(dep_errs) => errs.extend(dep_errs),
                },
                _ => continue,
            }
        }

        // Extract functions.
        for func in decl_items.funcs {
            let fname = func.fname.value.as_str();
            if fname == funcs::BUILD {
                opt_build_func = Some(func);
            } else if fname == funcs::PACKAGE {
                single_package_func = Some(func);
            } else if let Some(pkg_name) = fname.strip_prefix(funcs::PACKAGE_PREFIX) {
                split_package_funcs.insert(pkg_name.to_string(), func);
            }
        }

        // Check that required variables are set.
        let has_pkgname = opt_pkgname_single.is_some() || opt_pkgname_array.is_some();
        let required_vars = [
            (vars::PKGNAME, has_pkgname),
            (vars::PKGVER, opt_pkgver.is_some()),
            (vars::ARCH, opt_arch.is_some()),
            (vars::PKGDESC, opt_pkgdesc.is_some()),
        ];

        for (var_name, is_set) in required_vars {
            if !is_set {
                errs.push(KocaParserError::MissingRequiredVariable(var_name.to_string()).into());
            }
        }

        // Build PackageKind and validate package functions.
        let opt_packages: Option<PackageKind>;
        let mut package_funcs: Vec<(String, FunctionDefinition)> = vec![];

        if let Some(names) = opt_pkgname_array {
            // Split mode: require pkgbase, require package:NAME() for each, forbid package().
            if opt_pkgbase.is_none() {
                errs.push(
                    KocaParserError::MissingRequiredVariable(vars::PKGBASE.to_string()).into(),
                );
            }
            if single_package_func.is_some() {
                errs.push(
                    KocaParserError::SplitPackageConflict(
                        "package() cannot be used with split packages; use package:NAME() instead"
                            .to_string(),
                    )
                    .into(),
                );
            }
            for name in &names {
                match split_package_funcs.remove(name) {
                    Some(func) => package_funcs.push((name.clone(), func)),
                    None => errs.push(
                        KocaParserError::MissingRequiredFunction(format!("package:{name}")).into(),
                    ),
                }
            }
            // Warn about extra package:NAME() functions not in pkgname array.
            for extra_name in split_package_funcs.keys() {
                errs.push(
                    KocaParserError::SplitPackageConflict(format!(
                        "package:{extra_name}() defined but '{extra_name}' is not in pkgname"
                    ))
                    .into(),
                );
            }
            opt_packages = opt_pkgbase.map(|base| PackageKind::Split { base, names });
        } else if let Some(name) = opt_pkgname_single {
            // Single mode: require package(), forbid pkgbase, forbid package:NAME().
            if opt_pkgbase.is_some() {
                errs.push(
                    KocaParserError::SplitPackageConflict(
                        "pkgbase cannot be used with a single-string pkgname".to_string(),
                    )
                    .into(),
                );
            }
            if !split_package_funcs.is_empty() {
                errs.push(
                    KocaParserError::SplitPackageConflict(
                        "package:NAME() functions cannot be used with a single-string pkgname"
                            .to_string(),
                    )
                    .into(),
                );
            }
            match single_package_func {
                Some(func) => package_funcs.push((name.clone(), func)),
                None => errs.push(
                    KocaParserError::MissingRequiredFunction(funcs::PACKAGE.to_string()).into(),
                ),
            }
            opt_packages = Some(PackageKind::Single(name));
        } else {
            opt_packages = None;
        }

        // Check that build() is defined.
        if opt_build_func.is_none() {
            errs.push(
                KocaParserError::MissingRequiredFunction(funcs::BUILD.to_string()).into(),
            );
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
            shell,
            vars: decl_items.vars,
            packages: opt_packages.expect("packages should be set"),
            var_version: parsed_version.expect("version should be valid by this point"),
            var_arch: opt_arch.expect("arch should be set"),
            var_pkgdesc: opt_pkgdesc.expect("pkgdesc should be set"),
            var_depends: opt_depends,
            var_makedepends: opt_makedepends,
            build_func: opt_build_func.expect("build function should be set"),
            package_funcs,
        })
    }

    /// Read a Koca build script from the input file.
    ///
    /// Returns a:
    /// - [`KocaError::Parser`] error if the input is an invalid script.
    /// - [`KocaError::IO`] error if the input file can't be read.
    pub async fn parse_file<P: AsRef<Path>>(path: P) -> KocaMultiResult<Self> {
        let file = File::open(path).map_err(|err| vec![err.into()])?;
        Self::parse(file).await
    }

    /// Add environment variables to the environment.
    fn add_vars(&mut self) {
        // Add inherited vars.
        for (key, var) in env::vars() {
            let mut shell_var = ShellVariable::new(var);
            shell_var.export();
            self.shell
                .set_env_global(&key, shell_var)
                .expect("setting environment variable shouldn't fail");
        }

        // Add built-in vars.
        for (key, var) in &self.vars {
            let shell_var = match var {
                DeclValue::String(val) => ShellVariable::new(val.value.clone()),
                DeclValue::Array(vals) => {
                    let string_values: Vec<String> =
                        vals.iter().map(|word| word.value.clone()).collect();
                    ShellVariable::new(string_values)
                }
            };

            self.shell
                .set_env_global(key, shell_var)
                .expect("setting environment variable shouldn't fail");
        }
    }

    /// Run the `build` function of the package.
    ///
    /// Returns a:
    /// - [`KocaError::IO`] if the build directory couldn't be created.
    /// - [`KocaError::Func`] if the `build` function failed to execute.
    pub async fn run_build(&mut self) -> KocaResult<()> {
        self.run_build_with_output(|_| {}).await
    }

    /// Run `build()` with a callback. Called with `Some(line)` for output,
    /// `None` every ~80ms for tick/spinner animation.
    pub async fn run_build_with_output(
        &mut self,
        callback: impl FnMut(Option<BuildOutputLine>),
    ) -> KocaResult<()> {
        self.run_function_with_output(KocaFunction::Build, self.build_func.clone(), vec![], callback)
            .await
    }

    /// Run a named `package` (or `package:NAME`) function.
    pub async fn run_package_for(&mut self, pkg_name: &str) -> KocaResult<()> {
        self.run_package_for_with_output(pkg_name, |_| {}).await
    }

    pub async fn run_package_for_with_output(
        &mut self,
        pkg_name: &str,
        callback: impl FnMut(Option<BuildOutputLine>),
    ) -> KocaResult<()> {
        let (_, func) = self
            .package_funcs
            .iter()
            .find(|(name, _)| name == pkg_name)
            .ok_or_else(|| {
                KocaError::FuncError(KocaFunction::Package)
            })?;
        let func = func.clone();

        let pkg_dir = dirs::pkg_for(pkg_name);
        fs::create_dir_all(&pkg_dir)?;
        let absolute_pkgdir = path::absolute(&pkg_dir)
            .expect("directory should be valid")
            .to_string_lossy()
            .into_owned();

        let extra_env = vec![
            ("pkgdir", absolute_pkgdir),
            ("pkgname", pkg_name.to_string()),
            ("pkgbase", self.packages.base().to_string()),
        ];

        self.run_function_with_output(KocaFunction::Package, func, extra_env, callback)
            .await
    }

    async fn run_function_with_output(
        &mut self,
        function_kind: KocaFunction,
        function: FunctionDefinition,
        extra_env: Vec<(&str, String)>,
        mut callback: impl FnMut(Option<BuildOutputLine>),
    ) -> KocaResult<()> {
        self.shell.undefine_func(funcs::BUILD);
        self.shell.undefine_func(funcs::PACKAGE);
        // Also undefine any split package functions.
        for (name, _) in &self.package_funcs {
            self.shell
                .undefine_func(&format!("{}{}", funcs::PACKAGE_PREFIX, name));
        }
        self.shell
            .define_func(function.fname.value.clone(), function.clone());

        self.add_vars();
        fs::create_dir_all(dirs::SRC)?;

        for (name, value) in extra_env {
            self.shell
                .set_env_global(name, ShellVariable::new(value))
                .expect("setting environment variable shouldn't fail");
        }

        let existing_dir = self.shell.working_dir().to_path_buf();
        self.shell.set_working_dir(dirs::SRC)?;

        let result = self
            .invoke_function_with_output(function.fname.value.as_str(), &mut callback)
            .await;

        self.shell.set_working_dir(existing_dir)?;

        let exit_code = result?;
        if exit_code != 0 {
            return Err(KocaError::FuncError(function_kind));
        }

        Ok(())
    }

    async fn invoke_function_with_output(
        &mut self,
        function_name: &str,
        callback: &mut impl FnMut(Option<BuildOutputLine>),
    ) -> KocaResult<u8> {
        let (stdout_reader, stdout_writer) = std::io::pipe()?;
        let (stderr_reader, stderr_writer) = std::io::pipe()?;

        let (tx, mut rx) = mpsc::unbounded_channel();
        let stdout_handle =
            spawn_output_reader(stdout_reader, BuildOutputStream::Stdout, tx.clone());
        let stderr_handle = spawn_output_reader(stderr_reader, BuildOutputStream::Stderr, tx);

        let mut params = self.shell.default_exec_params();
        params.set_fd(1, stdout_writer.into());
        params.set_fd(2, stderr_writer.into());

        let mut ticker = tokio::time::interval(std::time::Duration::from_millis(80));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        let exit_code = {
            let invoke_params = params.clone();
            let mut invoke = std::pin::pin!(self.shell.invoke_function(
                function_name,
                std::iter::empty::<&str>(),
                &invoke_params
            ));

            loop {
                tokio::select! {
                    result = &mut invoke => break result?,
                    maybe_line = rx.recv() => {
                        if let Some(line) = maybe_line {
                            callback(Some(line));
                        }
                    }
                    _ = ticker.tick() => {
                        callback(None);
                    }
                }
            }
        };

        drop(params);

        while let Some(line) = rx.recv().await {
            callback(Some(line));
        }

        let _ = stdout_handle.join();
        let _ = stderr_handle.join();

        Ok(exit_code)
    }

    /// Bundle the named package into the given file format.
    pub async fn bundle(
        &self,
        pkg_name: &str,
        format: BundleFormat,
        out_file: &Path,
    ) -> KocaResult<()> {
        let system_arch = format.arch_string(&self.var_arch[0]);
        let pkg_dir = dirs::pkg_for(pkg_name);

        let config = NfpmConfig {
            name: pkg_name.to_string(),
            // TODO: We need to figure out what architecture should properly be used at runtime of the built package.
            arch: system_arch.to_owned(),
            // TODO: We'll need to modify this when we support Windows/macOS in the future.
            platform: "linux".to_string(),
            epoch: self.var_version.epoch,
            version: self.var_version.pkgver.to_string(),
            release: self.var_version.pkgrel,
            // TODO: We need to get the maintainer from the build file.
            // Dependent on https://github.com/reubeno/brush/issues/513.
            maintainer: "Foo Bar <foobar@example.com>".to_string(),
            description: self.var_pkgdesc.clone(),
            // TODO: We need to check for this somehow - figure out what the build file implementation looks like.
            license: "CONTACT-PUBLISHER".to_string(),
            depends: self.var_depends.iter().map(|d| d.to_string()).collect(),
            contents: nfpm::get_nfpm_files(Path::new(&pkg_dir)),
        };
        let config_json =
            serde_json::to_string(&config).expect("build config should be valid json");

        // Make sure we can access the build file.
        // our nFPM bindings check for this too, but it's hard to get the error out of it.
        if let Err(err) = File::create(out_file) {
            return Err(err.into());
        }

        // Build with `nfpm`.
        let nfpm_res = nfpm_sys::run_bundle(
            &out_file.display().to_string(),
            format.extension(),
            &config_json,
        );

        if let Err(err) = nfpm_res {
            match err {
                NfpmError::JSON => unreachable!("build config should always deserialize"),
                NfpmError::OutputFile => {
                    unreachable!("build file should have been checked earlier")
                }
                NfpmError::PkgCreation => unreachable!("package creation should always work"),
            }
        }
        Ok(())
    }

    /// Get the package kind (single vs split).
    pub fn packages(&self) -> &PackageKind {
        &self.packages
    }

    /// Get all package names.
    pub fn pkgnames(&self) -> &[String] {
        self.packages.names()
    }

    /// Get the package base name.
    pub fn pkgbase(&self) -> &str {
        self.packages.base()
    }

    /// Get the package's version.
    pub fn version(&self) -> &Version {
        &self.var_version
    }

    /// Get the package's architecture.
    pub fn arch(&self) -> &[Arch] {
        &self.var_arch
    }

    /// Get the package's description.
    pub fn pkgdesc(&self) -> &str {
        &self.var_pkgdesc
    }

    /// Get the package's runtime dependency constraints.
    pub fn depends(&self) -> &[crate::dep::DepConstraint] {
        &self.var_depends
    }

    /// Get the package's build-time dependency constraints.
    pub fn makedepends(&self) -> &[crate::dep::DepConstraint] {
        &self.var_makedepends
    }
}

fn spawn_output_reader(
    reader: std::io::PipeReader,
    stream: BuildOutputStream,
    tx: mpsc::UnboundedSender<BuildOutputLine>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let mut reader = BufReader::new(reader);
        let mut line = String::new();

        loop {
            line.clear();
            match reader.read_line(&mut line) {
                Ok(0) => break,
                Ok(_) => {
                    while line.ends_with('\n') || line.ends_with('\r') {
                        line.pop();
                    }

                    let _ = tx.send(BuildOutputLine {
                        stream,
                        line: line.clone(),
                    });
                }
                Err(_) => break,
            }
        }
    })
}

/// A mapping of `const` variable names to their stringified values.
pub mod vars {
    pub const PKGBASE: &str = "pkgbase";
    pub const PKGNAME: &str = "pkgname";
    pub const PKGVER: &str = "pkgver";
    pub const PKGREL: &str = "pkgrel";
    pub const EPOCH: &str = "epoch";
    pub const ARCH: &str = "arch";
    pub const PKGDESC: &str = "pkgdesc";
    pub const DEPENDS: &str = "depends";
    pub const MAKEDEPENDS: &str = "makedepends";
}

/// A mapping of `const` function names to their stringified values.
pub mod funcs {
    pub const BUILD: &str = "build";
    pub const PACKAGE: &str = "package";
    /// Prefix for split package functions (e.g. `package:koca`).
    pub const PACKAGE_PREFIX: &str = "package:";
}

/// The directories used by Koca.
mod dirs {
    /// The directory where Koca stores source files.
    pub const SRC: &str = "koca/src";

    /// Return the package directory for a named sub-package.
    pub fn pkg_for(name: &str) -> String {
        format!("koca/pkg/{name}")
    }
}
