use anyhow::Error;
use bzip2::read::{BzDecoder, BzEncoder};
use bzip2::Compression;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    io::{self, Read},
};
use strum::{Display, EnumIter, EnumString, FromRepr};

pub type CompressionAlgorithms = HashSet<CompressionAlgorithm>;

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

// public for benchmarking
pub fn compress(content: impl Read, algorithm: CompressionAlgorithm) -> Result<Vec<u8>, Error> {
    match algorithm {
        CompressionAlgorithm::Zstd => Ok(zstd::encode_all(content, 9)?),
        CompressionAlgorithm::Bzip2 => {
            let mut compressor = BzEncoder::new(content, Compression::best());

            let mut data = vec![];
            compressor.read_to_end(&mut data)?;
            Ok(data)
        }
    }
}

pub fn decompress(
    content: impl Read,
    algorithm: CompressionAlgorithm,
    max_size: usize,
) -> Result<Vec<u8>, Error> {
    // The sized buffer prevents a malicious file from decompressing to multiple times its size.
    let mut buffer = crate::utils::sized_buffer::SizedBuffer::new(max_size);

    match algorithm {
        CompressionAlgorithm::Zstd => zstd::stream::copy_decode(content, &mut buffer)?,
        CompressionAlgorithm::Bzip2 => {
            io::copy(&mut BzDecoder::new(content), &mut buffer)?;
        }
    }

    Ok(buffer.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use strum::IntoEnumIterator;

    #[test]
    fn test_compression() {
        let orig = "fn main() {}";
        for alg in CompressionAlgorithm::iter() {
            println!("testing algorithm {alg}");

            let data = compress(orig.as_bytes(), alg).unwrap();
            assert_eq!(
                decompress(data.as_slice(), alg, std::usize::MAX).unwrap(),
                orig.as_bytes()
            );
        }
    }

    #[test]
    fn test_decompression_too_big() {
        const MAX_SIZE: usize = 1024;

        let small = &[b'A'; MAX_SIZE / 2] as &[u8];
        let exact = &[b'A'; MAX_SIZE] as &[u8];
        let big = &[b'A'; MAX_SIZE * 2] as &[u8];

        for alg in CompressionAlgorithm::iter() {
            let compressed_small = compress(small, alg).unwrap();
            let compressed_exact = compress(exact, alg).unwrap();
            let compressed_big = compress(big, alg).unwrap();

            // Ensure decompressing within the limit works.
            assert_eq!(
                small.len(),
                decompress(compressed_small.as_slice(), alg, MAX_SIZE)
                    .unwrap()
                    .len()
            );
            assert_eq!(
                exact.len(),
                decompress(compressed_exact.as_slice(), alg, MAX_SIZE)
                    .unwrap()
                    .len()
            );

            // Ensure decompressing a file over the limit returns a SizeLimitReached error.
            let err = decompress(compressed_big.as_slice(), alg, MAX_SIZE).unwrap_err();
            assert!(err
                .downcast_ref::<std::io::Error>()
                .and_then(|io| io.get_ref())
                .and_then(|err| err.downcast_ref::<crate::error::SizeLimitReached>())
                .is_some());
        }
    }

    #[test]
    fn test_enum_display() {
        assert_eq!(CompressionAlgorithm::Zstd.to_string(), "Zstd");
        assert_eq!(CompressionAlgorithm::Bzip2.to_string(), "Bzip2");
    }
}
