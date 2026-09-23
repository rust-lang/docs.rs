// Adapted from RustSec (MIT); see ../LICENSE-MIT.
//! Informational advisories: ones which don't represent an immediate security
//! threat, but something users of a crate should be warned of/aware of

use serde::{Deserialize, Serialize, de, ser};
use std::convert::Infallible as Error;
use std::{fmt, str::FromStr};

/// Categories of informational vulnerabilities
#[derive(Clone, Debug, Eq, Hash, PartialEq, PartialOrd, Ord)]
#[non_exhaustive]
pub enum Informational {
    /// Security notices for a crate which are published on <https://rustsec.org>
    /// but don't represent a vulnerability in a crate itself.
    Notice,

    /// Crate is unmaintained / abandoned
    Unmaintained,

    /// Crate is not [sound], i.e., unsound.
    ///
    /// A crate is unsound if, using its public API from safe code, it is possible to cause [Undefined Behavior].
    ///
    /// [sound]: https://rust-lang.github.io/unsafe-code-guidelines/glossary.html#soundness-of-code--of-a-library
    /// [Undefined Behavior]: https://doc.rust-lang.org/reference/behavior-considered-undefined.html
    Unsound,

    /// Other types of informational advisories: left open-ended to add
    /// more of them in the future.
    Other(String),
}

impl Informational {
    /// Get a `str` representing an [`Informational`] category
    pub fn as_str(&self) -> &str {
        match self {
            Self::Notice => "notice",
            Self::Unmaintained => "unmaintained",
            Self::Unsound => "unsound",
            Self::Other(other) => other,
        }
    }
}

impl fmt::Display for Informational {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl FromStr for Informational {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Error> {
        Ok(match s {
            "notice" => Self::Notice,
            "unmaintained" => Self::Unmaintained,
            "unsound" => Self::Unsound,
            other => Self::Other(other.to_owned()),
        })
    }
}

impl<'de> Deserialize<'de> for Informational {
    fn deserialize<D: de::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        use de::Error;
        let string = String::deserialize(deserializer)?;
        string.parse().map_err(D::Error::custom)
    }
}

impl Serialize for Informational {
    fn serialize<S: ser::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.to_string().serialize(serializer)
    }
}
