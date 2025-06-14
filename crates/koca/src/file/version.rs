use crate::{KocaError, KocaParserError, KocaResult};
use std::{cmp::Ordering, fmt, str::FromStr};

static EPOCH_SEPARATOR: &str = ":";
static PKGREL_SEPARATOR: &str = "-";

/// A package's `pkgver` component.
#[derive(Clone, PartialEq)]
pub struct PkgVersion {
    /// The major version component.
    pub major: u32,
    /// The minor version component.
    pub minor: u32,
    /// The patch release component.
    pub patch: u32,
}

impl FromStr for PkgVersion {
    type Err = KocaError;

    /// Parse a `pkgver` string into a [`PkgVersion`].
    ///
    /// Returns [`KocaParserError::InvalidVersion`] if the string is not a valid `pkgver`.
    fn from_str(pkgver: &str) -> KocaResult<Self> {
        let version_err = || Err(KocaParserError::InvalidVersion(pkgver.to_string()).into());

        if pkgver.chars().filter(|c| c == &'.').count() != 2 {
            return version_err();
        }

        let mut sections = vec![];
        for part in pkgver.split('.') {
            match part.parse::<u32>() {
                Ok(value) => sections.push(value),
                Err(_) => return version_err(),
            }
        }

        Ok(Self {
            major: sections[0],
            minor: sections[1],
            patch: sections[2],
        })
    }
}

impl fmt::Display for PkgVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

impl PartialOrd for PkgVersion {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        if self.major != other.major {
            self.major.partial_cmp(&other.major)
        } else if self.minor != other.minor {
            self.minor.partial_cmp(&other.minor)
        } else {
            self.patch.partial_cmp(&other.patch)
        }
    }
}

/// A package's version.
#[derive(Clone, PartialEq)]
pub struct Version {
    /// The version's package version segment (`1.0.0` in `1.0.0-2`).
    pub pkgver: PkgVersion,
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
        if let Some((epoch, remaining)) = str_segment.split_once(EPOCH_SEPARATOR) {
            str_segment = remaining;

            match epoch.parse() {
                Ok(value) => opt_epoch = Some(value),
                Err(_) => {
                    return version_err();
                }
            }
        }

        // Check for pkgrel.
        if let Some((remaining, pkgrel)) = str_segment.split_once(PKGREL_SEPARATOR) {
            str_segment = remaining;

            match pkgrel.parse() {
                Ok(value) => opt_pkgrel = Some(value),
                Err(_) => {
                    return version_err();
                }
            }
        }

        Ok(Self {
            pkgver: PkgVersion::from_str(str_segment)?,
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

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        if self == other {
            return Some(Ordering::Equal);
        }

        // First check if one epoch is bigger than the other.
        let self_epoch = self.epoch.unwrap_or(0);
        let other_epoch = other.epoch.unwrap_or(0);
        let epoch_order = self_epoch
            .partial_cmp(&other_epoch)
            .expect("epoch should be comparable");

        if epoch_order != Ordering::Equal {
            return Some(epoch_order);
        }

        // Next, check against the pkgver.
        let pkgver_order = self
            .pkgver
            .partial_cmp(&other.pkgver)
            .expect("pkgver should be comparable");
        if pkgver_order != Ordering::Equal {
            return Some(pkgver_order);
        }

        // Lastly, compare against pkgrel if all else fails.
        let self_pkgrel = self.pkgrel.unwrap_or(0);
        let other_pkgrel = other.pkgrel.unwrap_or(0);

        self_pkgrel.partial_cmp(&other_pkgrel)
    }
}
