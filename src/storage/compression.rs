use anyhow::Error;
use bzip2::read::{BzDecoder, BzEncoder};
use flate2::read::{GzDecoder, GzEncoder};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    io::{self, Read},
};
use strum::{Display, EnumIter, EnumString, FromRepr};
use tokio::io::{AsyncRead, AsyncWrite};

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
    Gzip = 2,
}

impl CompressionAlgorithm {
    pub fn file_extension(&self) -> &'static str {
        file_extension_for(*self)
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

pub(crate) fn file_extension_for(algorithm: CompressionAlgorithm) -> &'static str {
    match algorithm {
        CompressionAlgorithm::Zstd => "zst",
        CompressionAlgorithm::Bzip2 => "bz2",
        CompressionAlgorithm::Gzip => "gz",
    }
}

pub(crate) fn compression_from_file_extension(ext: &str) -> Option<CompressionAlgorithm> {
    match ext {
        "zst" => Some(CompressionAlgorithm::Zstd),
        "bz2" => Some(CompressionAlgorithm::Bzip2),
        "gz" => Some(CompressionAlgorithm::Gzip),
        _ => None,
    }
}

// public for benchmarking
pub fn compress(content: impl Read, algorithm: CompressionAlgorithm) -> Result<Vec<u8>, Error> {
    match algorithm {
        CompressionAlgorithm::Zstd => Ok(zstd::encode_all(content, 9)?),
        CompressionAlgorithm::Bzip2 => {
            let mut compressor = BzEncoder::new(content, bzip2::Compression::best());

            let mut data = vec![];
            compressor.read_to_end(&mut data)?;
            Ok(data)
        }
        CompressionAlgorithm::Gzip => {
            let mut compressor = GzEncoder::new(content, flate2::Compression::default());
            let mut data = vec![];
            compressor.read_to_end(&mut data)?;
            Ok(data)
        }
    }
}

/// Wrap an AsyncWrite sink for compression using the specified algorithm.
///
/// Will return an AsyncWrite you can just write data to, we will compress
/// the data, and then write the compressed data into the provided output sink.
pub fn wrap_writer_for_compression<'a>(
    output_sink: impl AsyncWrite + Unpin + Send + 'a,
    algorithm: CompressionAlgorithm,
) -> Box<dyn AsyncWrite + Unpin + 'a> {
    use async_compression::tokio::write;
    use tokio::io;

    match algorithm {
        CompressionAlgorithm::Zstd => {
            Box::new(io::BufWriter::new(write::ZstdEncoder::new(output_sink)))
        }
        CompressionAlgorithm::Bzip2 => {
            Box::new(io::BufWriter::new(write::BzEncoder::new(output_sink)))
        }
        CompressionAlgorithm::Gzip => {
            Box::new(io::BufWriter::new(write::GzipEncoder::new(output_sink)))
        }
    }
}

/// Wrap an AsyncRead for decompression.
///
/// You provide an AsyncRead that gives us the compressed data. With the
/// wrapper we return you can then read decompressed data from the wrapper.
pub fn wrap_reader_for_decompression<'a>(
    input: impl AsyncRead + Unpin + Send + 'a,
    algorithm: CompressionAlgorithm,
) -> Box<dyn AsyncRead + Unpin + Send + 'a> {
    use async_compression::tokio::bufread;
    use tokio::io;

    match algorithm {
        CompressionAlgorithm::Zstd => {
            Box::new(bufread::ZstdDecoder::new(io::BufReader::new(input)))
        }
        CompressionAlgorithm::Bzip2 => Box::new(bufread::BzDecoder::new(io::BufReader::new(input))),
        CompressionAlgorithm::Gzip => {
            Box::new(bufread::GzipDecoder::new(io::BufReader::new(input)))
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
        CompressionAlgorithm::Gzip => {
            io::copy(&mut GzDecoder::new(content), &mut buffer)?;
        }
    }

    Ok(buffer.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use strum::IntoEnumIterator;
    use test_case::test_case;

    #[test]
    fn test_compression() {
        let orig = "fn main() {}";
        for alg in CompressionAlgorithm::iter() {
            println!("testing algorithm {alg}");

            let data = compress(orig.as_bytes(), alg).unwrap();
            assert_eq!(
                decompress(data.as_slice(), alg, usize::MAX).unwrap(),
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
            assert!(
                err.downcast_ref::<std::io::Error>()
                    .and_then(|io| io.get_ref())
                    .and_then(|err| err.downcast_ref::<crate::error::SizeLimitReached>())
                    .is_some()
            );
        }
    }

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
        assert_eq!(file_extension_for(alg), expected);
        assert_eq!(compression_from_file_extension(expected), Some(alg));
    }
}
