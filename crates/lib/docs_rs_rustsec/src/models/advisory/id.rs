// Adapted from RustSec (MIT); see ../LICENSE-MIT.
//! Advisory identifiers

use super::date::{YEAR_MAX, YEAR_MIN};
use anyhow::{Error, bail};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error as DeError};
use std::{
    fmt::{self, Display},
    str::FromStr,
};

/// An identifier for an individual advisory
#[derive(Clone, Debug, Eq, Hash, PartialEq, PartialOrd, Ord)]
pub struct Id {
    /// The actual string representing the identifier
    string: String,
}

impl Id {
    /// Placeholder advisory name: shouldn't be used until an ID is assigned
    pub const PLACEHOLDER: &'static str = "RUSTSEC-0000-0000";

    /// Get a string reference to this advisory ID
    pub fn as_str(&self) -> &str {
        self.string.as_ref()
    }
}

impl Display for Id {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Id {
    type Err = Error;

    /// Create an `Id` from the given string
    fn from_str(advisory_id: &str) -> Result<Self, Error> {
        if advisory_id == Self::PLACEHOLDER {
            return Ok(Self {
                string: advisory_id.into(),
            });
        }

        let kind = IdKind::detect(advisory_id);

        // Ensure known advisory types are well-formed
        match kind {
            IdKind::RustSec | IdKind::Cve | IdKind::Talos => {
                parse_year(advisory_id)?;
            }
            _ => {}
        }

        Ok(Self {
            string: advisory_id.into(),
        })
    }
}

impl Serialize for Id {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.string)
    }
}

impl<'de> Deserialize<'de> for Id {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::from_str(&String::deserialize(deserializer)?).map_err(D::Error::custom)
    }
}

/// Known kinds of advisory IDs
#[derive(Copy, Clone, Debug, Eq, Hash, PartialEq, PartialOrd, Ord)]
#[non_exhaustive]
enum IdKind {
    /// Our advisory namespace
    RustSec,

    /// Common Vulnerabilities and Exposures
    Cve,

    /// GitHub Security Advisory
    Ghsa,

    /// Cisco Talos identifiers
    Talos,

    /// Other types of advisory identifiers we don't know about
    Other,
}

impl IdKind {
    /// Detect the identifier kind for the given string
    fn detect(string: &str) -> Self {
        if string.starts_with("RUSTSEC-") {
            Self::RustSec
        } else if string.starts_with("CVE-") {
            Self::Cve
        } else if string.starts_with("TALOS-") {
            Self::Talos
        } else if string.starts_with("GHSA-") {
            Self::Ghsa
        } else {
            Self::Other
        }
    }
}

/// Parse the year from an advisory identifier
fn parse_year(advisory_id: &str) -> Result<u32, Error> {
    let mut parts = advisory_id.split('-');
    parts.next().unwrap();

    let year = match parts.next().unwrap_or_default().parse::<u32>() {
        Ok(n) => match n {
            YEAR_MIN..=YEAR_MAX => n,
            _ => bail!("out-of-range year in advisory ID: {}", advisory_id),
        },
        _ => bail!("malformed year in advisory ID: {}", advisory_id),
    };

    if let Some(num) = parts.next() {
        if num.parse::<u32>().is_err() {
            bail!("malformed advisory ID: {}", advisory_id);
        }
    } else {
        bail!("incomplete advisory ID: {}", advisory_id);
    }

    if parts.next().is_some() {
        bail!("malformed advisory ID: {}", advisory_id);
    }

    Ok(year)
}
