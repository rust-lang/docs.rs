use std::{num::ParseIntError, ops::RangeInclusive, str::FromStr};
use strum::EnumString;

pub type FileRange = RangeInclusive<u64>;

#[derive(Debug, Copy, Clone, EnumString)]
#[strum(serialize_all = "snake_case")]
pub enum StorageKind {
    #[cfg(any(test, feature = "testing"))]
    Memory,
    S3,
}

impl Default for StorageKind {
    fn default() -> Self {
        #[cfg(any(test, feature = "testing"))]
        return StorageKind::Memory;
        #[cfg(not(any(test, feature = "testing")))]
        return StorageKind::S3;
    }
}

#[derive(strum::Display, Debug, PartialEq, Eq, Clone, Copy)]
#[strum(serialize_all = "snake_case")]
pub enum RustdocJsonFormatVersion {
    #[strum(serialize = "{0}")]
    Version(u16),
    Latest,
}

impl FromStr for RustdocJsonFormatVersion {
    type Err = ParseIntError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s == "latest" {
            Ok(RustdocJsonFormatVersion::Latest)
        } else {
            s.parse::<u16>().map(RustdocJsonFormatVersion::Version)
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use test_case::test_case;

    #[test_case("latest", RustdocJsonFormatVersion::Latest)]
    #[test_case("42", RustdocJsonFormatVersion::Version(42))]
    fn test_json_format_version(input: &str, expected: RustdocJsonFormatVersion) {
        // test Display
        assert_eq!(expected.to_string(), input);
        // test FromStr
        assert_eq!(expected, input.parse().unwrap());
    }
}
