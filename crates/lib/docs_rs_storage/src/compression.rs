use anyhow::Error;
use bzip2::read::{BzDecoder, BzEncoder};
use docs_rs_types::CompressionAlgorithm;
use flate2::read::{GzDecoder, GzEncoder};
use std::io::{self, Read};
use tokio::io::{AsyncBufRead, AsyncRead, AsyncWrite};

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

/// async compression, reads from an AsyncRead, writes to an AsyncWrite.
pub async fn compress_async<'a, R, W>(
    mut reader: R,
    writer: W,
    algorithm: CompressionAlgorithm,
) -> io::Result<()>
where
    R: AsyncRead + Unpin + Send + 'a,
    W: AsyncWrite + Unpin + Send + 'a,
{
    use async_compression::tokio::write;
    use tokio::io::{self, AsyncWriteExt as _};

    match algorithm {
        CompressionAlgorithm::Zstd => {
            let mut enc = write::ZstdEncoder::new(writer);
            io::copy(&mut reader, &mut enc).await?;
            enc.shutdown().await?;
        }
        CompressionAlgorithm::Bzip2 => {
            let mut enc = write::BzEncoder::new(writer);
            io::copy(&mut reader, &mut enc).await?;
            enc.shutdown().await?;
        }
        CompressionAlgorithm::Gzip => {
            let mut enc = write::GzipEncoder::new(writer);
            io::copy(&mut reader, &mut enc).await?;
            enc.shutdown().await?;
        }
    }

    Ok(())
}

/// Wrap an AsyncRead for decompression.
///
/// You provide an AsyncRead that gives us the compressed data. With the
/// wrapper we return you can then read decompressed data from the wrapper.
pub fn wrap_reader_for_decompression<'a>(
    input: impl AsyncBufRead + Unpin + Send + 'a,
    algorithm: CompressionAlgorithm,
) -> Box<dyn AsyncBufRead + Unpin + Send + 'a> {
    use async_compression::tokio::bufread;
    use tokio::io;

    match algorithm {
        CompressionAlgorithm::Zstd => {
            Box::new(io::BufReader::new(bufread::ZstdDecoder::new(input)))
        }
        CompressionAlgorithm::Bzip2 => Box::new(io::BufReader::new(bufread::BzDecoder::new(input))),
        CompressionAlgorithm::Gzip => {
            Box::new(io::BufReader::new(bufread::GzipDecoder::new(input)))
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
    use crate::{StreamingBlob, errors::SizeLimitReached};
    use anyhow::Result;
    use chrono::Utc;
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
                    .and_then(|err| err.downcast_ref::<SizeLimitReached>())
                    .is_some()
            );
        }
    }

    #[tokio::test]
    #[test_case(CompressionAlgorithm::Zstd)]
    #[test_case(CompressionAlgorithm::Bzip2)]
    #[test_case(CompressionAlgorithm::Gzip)]
    async fn test_async_compression(alg: CompressionAlgorithm) -> Result<()> {
        const CONTENT: &[u8] = b"Hello, world! Hello, world! Hello, world! Hello, world!";

        let compressed_index_content = {
            let mut buf: Vec<u8> = Vec::new();
            compress_async(&mut io::Cursor::new(CONTENT.to_vec()), &mut buf, alg).await?;
            buf
        };

        {
            // try low-level async decompression
            let mut decompressed_buf: Vec<u8> = Vec::new();
            let mut reader = wrap_reader_for_decompression(
                io::Cursor::new(compressed_index_content.clone()),
                alg,
            );

            tokio::io::copy(&mut reader, &mut io::Cursor::new(&mut decompressed_buf)).await?;

            assert_eq!(decompressed_buf, CONTENT);
        }

        {
            // try sync decompression
            let decompressed_buf: Vec<u8> = decompress(
                io::Cursor::new(compressed_index_content.clone()),
                alg,
                usize::MAX,
            )?;

            assert_eq!(decompressed_buf, CONTENT);
        }

        // try decompress via storage API
        let blob = StreamingBlob {
            path: "some_path.db".into(),
            mime: mime::APPLICATION_OCTET_STREAM,
            date_updated: Utc::now(),
            etag: None,
            compression: Some(alg),
            content_length: compressed_index_content.len(),
            content: Box::new(io::Cursor::new(compressed_index_content)),
        }
        .decompress()
        .await?
        .materialize(usize::MAX)
        .await?;

        assert_eq!(blob.compression, None);
        assert_eq!(blob.content, CONTENT);

        Ok(())
    }
}
