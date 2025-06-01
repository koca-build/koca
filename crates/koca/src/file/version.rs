use std::{fmt, str::FromStr};

use crate::{KocaError, KocaParserError, KocaResult};

static EPOCH_SEPARATOR: &str = ":";
static PKGREL_SEPARATOR: &str = "-";

/// A package's version.
pub struct Version {
    /// The version's package version segment (`1.0.0` in `1.0.0-2`).
    pub pkgver: String,
    /// The version's package release segment (`2` in `1.0.0-2`).
    pub pkgrel: Option<u32>,
    /// The version's epoch segment (`3` in `1.0.0-3`).
    pub epoch: Option<u32>,
}

impl FromStr for Version {
    type Err = KocaError;

    /// Parse a version string into a [`Version`].
    ///
    /// Returns [`KocaParserError::InvalidVersion`] if the string is not a valid version.
    fn from_str(value: &str) -> KocaResult<Self> {
        let mut str_segment = value;
        let mut opt_pkgver: Option<String> = None;
        let mut opt_pkgrel: Option<u32> = None;
        let mut opt_epoch: Option<u32> = None;

        let version_err = || Err(KocaParserError::InvalidVersion(value.to_string()).into());

        // Make sure the version only contains, at maximum, a singular instance of ':' and '-'.
        for separator in [EPOCH_SEPARATOR, PKGREL_SEPARATOR] {
            if value.matches(separator).count() > 1 {
                return version_err();
            }
        }

        // Check for epoch.
        if let Some((epoch, remaining)) = value.split_once(EPOCH_SEPARATOR) {
            str_segment = remaining;

            match epoch.parse() {
                Ok(value) => opt_epoch = Some(value),
                Err(_) => return version_err(),
            }
        }

        // Check for pkgrel.
        if let Some((pkgrel, remaining)) = value.split_once(PKGREL_SEPARATOR) {
            str_segment = remaining;

            match pkgrel.parse() {
                Ok(value) => opt_pkgrel = Some(value),
                Err(_) => return version_err(),
            }
        }

        if str_segment.is_empty() {
            return version_err();
        }

        Ok(Self {
            pkgver: str_segment.to_string(),
            pkgrel: opt_pkgrel,
            epoch: opt_epoch,
        })
    }
}

impl fmt::Display for Version {
    /// Format the [`Version`] as a string.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(epoch) = self.epoch {
            write!(f, "{}{}", epoch, EPOCH_SEPARATOR)?;
        }
        write!(f, "{}", self.pkgver)?;
        if let Some(pkgrel) = self.pkgrel {
            write!(f, "{}{}", PKGREL_SEPARATOR, pkgrel)?;
        }

        Ok(())
    }
}