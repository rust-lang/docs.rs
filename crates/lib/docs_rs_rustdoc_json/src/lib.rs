use anyhow::Result;
use docs_rs_types::CompressionAlgorithm;
use serde::Deserialize;
use std::{io::BufReader, num::ParseIntError, str::FromStr};

pub const RUSTDOC_JSON_COMPRESSION_ALGORITHMS: &[CompressionAlgorithm] =
    &[CompressionAlgorithm::Zstd, CompressionAlgorithm::Gzip];

/// read the format version from a rustdoc JSON file.
pub fn read_format_version_from_rustdoc_json(
    reader: impl std::io::Read,
) -> Result<RustdocJsonFormatVersion> {
    let reader = BufReader::new(reader);

    #[derive(Deserialize)]
    struct RustdocJson {
        format_version: u16,
    }

    let rustdoc_json: RustdocJson = serde_json::from_reader(reader)?;

    Ok(RustdocJsonFormatVersion::Version(
        rustdoc_json.format_version,
    ))
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
