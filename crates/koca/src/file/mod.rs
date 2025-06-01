mod arch;
mod parser;
mod version;

use crate::{KocaMultiResult, KocaParserError, KocaResult};
pub use arch::Arch;
use brush::{CreateOptions, Shell};
use brush_parser::{ast::Word, word::WordPiece};
use itertools::Itertools;
use parser::DeclValue;
use std::{fs::File, io::Read, path::Path};
pub use version::Version;

/// A package's Koca build file.
pub struct BuildFile {
    /// The package's name.
    pkgname: String,
    /// The package's version.
    version: Version,
    /// The package's architecture.
    arch: Vec<Arch>,
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

    /// Get the string out of a [`WordPiece`], while also making sure no expansion is present.
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
            _ => return Err(expansion_err().into()),
        }
    }

    /// Parse a [`DeclValue`] into a `pkgname`.
    fn parse_pkgname(value: DeclValue) -> KocaResult<String> {
        let string_value = &value
            .as_word()
            .ok_or(KocaParserError::NotString(vars::PKGNAME.to_string()))?
            .value;

        let piece = brush_parser::word::parse(string_value, &Default::default())
            .unwrap()
            .into_iter()
            .exactly_one()
            .expect("Word parser should not return 2+ elements for a string")
            .piece;

        Self::get_piece_string(vars::PKGNAME, piece)
    }

    /// Parse a [`DeclValue`] into a `version`.
    fn parse_version(value: DeclValue) -> KocaResult<Version> {
        let string_value = &value
            .as_word()
            .ok_or(KocaParserError::NotString(vars::VERSION.to_string()))?
            .value;

        let piece = brush_parser::word::parse(string_value, &Default::default())
            .unwrap()
            .into_iter()
            .exactly_one()
            .expect("Word parser should not return 2+ elements for a string")
            .piece;

        Self::get_piece_string(vars::VERSION, piece)?.parse()
    }

    // /// Parse a [`DeclValue`] into an `arch`.
    // fn parse_arch(value: DeclValue) -> KocaMultiResult<Vec<Arch>> {
    //     let mut errs = vec![];
    //     let mut archs = vec![];

    //     let string_values: Vec<_> = value
    //         .as_array()
    //         .ok_or(KocaParserError::NotArray(vars::ARCH.to_string()))?
    //         .iter()
    //         .map(|word| &word.value)
    //         .collect();

    //     for string_value in string_values {
    //     let piece = brush_parser::word::parse(string_value, &Default::default())
    //         .unwrap()
    //         .into_iter()
    //         .exactly_one()
    //         .expect("Word parser should not return 2+ elements for a string")
    //         .piece;
    //     }

    //     let arch_str = Self::get_piece_string(vars::ARCH, piece)?;
    //     Arch::parse(arch_str)
    // }

    /// Parse a Koca build script from the reader.
    ///
    /// Returns a [`KocaError::Parser`] error if the input is an invalid script.
    pub async fn parse<R: Read>(reader: R) -> KocaMultiResult<Self> {
        let create_options = Self::create_options();
        let mut shell = Shell::new(&create_options)
            .await
            .expect("shell options should be valid");
        let program = shell
            .parse(reader)
            .map_err(|err| vec![KocaParserError::from(err).into()])?;
        let decl_items = parser::get_decls(&program).map_err(|err| vec![err])?;

        let mut opt_pkgname: Option<String> = None;
        let mut opt_version: Option<Version> = None;
        let mut opt_arch: Option<Arch> = None;

        for (key, value) in decl_items.vars {
            match key.as_str() {
                vars::PKGNAME => {
                    opt_pkgname = Self::parse_pkgname(value).map_err(|err| vec![err])?.into()
                }
                vars::VERSION => {
                    opt_version = Self::parse_version(value).map_err(|err| vec![err])?.into()
                },
                vars::ARCH => todo!("De-comment the above function. Thx!"),
                _ => continue,
            }
        }

        todo!()
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

    /// Get the package's name.
    pub fn pkgname(&self) -> &str {
        &self.pkgname
    }

    /// Get the package's version.
    pub fn version(&self) -> &Version {
        &self.version
    }
}

/// A mapping of `const` variable names to their stringified values.
mod vars {
    pub const PKGNAME: &str = "pkgname";
    pub const VERSION: &str = "version";
    pub const ARCH: &str = "arch";
}
