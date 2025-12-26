use serde::{Deserialize, Serialize};
use strum::{Display, EnumIter, EnumString, FromRepr};

#[derive(
    Copy,
    Clone,
    Debug,
    PartialEq,
    Eq,
    Hash,
    Serialize,
    Deserialize,
    Default,
    EnumString,
    Display,
    FromRepr,
    EnumIter,
)]
pub enum CompressionAlgorithm {
    #[default]
    Zstd = 0,
    Bzip2 = 1,
    Gzip = 2,
}

impl CompressionAlgorithm {
    pub fn file_extension(&self) -> &'static str {
        match self {
            CompressionAlgorithm::Zstd => "zst",
            CompressionAlgorithm::Bzip2 => "bz2",
            CompressionAlgorithm::Gzip => "gz",
        }
    }
}

impl std::convert::TryFrom<i32> for CompressionAlgorithm {
    type Error = i32;
    fn try_from(i: i32) -> Result<Self, Self::Error> {
        if i >= 0 {
            match Self::from_repr(i as usize) {
                Some(alg) => Ok(alg),
                None => Err(i),
            }
        } else {
            Err(i)
        }
    }
}

pub fn compression_from_file_extension(ext: &str) -> Option<CompressionAlgorithm> {
    match ext {
        "zst" => Some(CompressionAlgorithm::Zstd),
        "bz2" => Some(CompressionAlgorithm::Bzip2),
        "gz" => Some(CompressionAlgorithm::Gzip),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_case::test_case;

    #[test_case(CompressionAlgorithm::Zstd, "Zstd")]
    #[test_case(CompressionAlgorithm::Bzip2, "Bzip2")]
    #[test_case(CompressionAlgorithm::Gzip, "Gzip")]
    fn test_enum_display(alg: CompressionAlgorithm, expected: &str) {
        assert_eq!(alg.to_string(), expected);
    }

    #[test_case(CompressionAlgorithm::Zstd, "zst")]
    #[test_case(CompressionAlgorithm::Bzip2, "bz2")]
    #[test_case(CompressionAlgorithm::Gzip, "gz")]
    fn test_file_extensions(alg: CompressionAlgorithm, expected: &str) {
        assert_eq!(alg.file_extension(), expected);
        assert_eq!(compression_from_file_extension(expected), Some(alg));
    }
}
