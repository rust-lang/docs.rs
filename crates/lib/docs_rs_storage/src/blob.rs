use crate::{compression::wrap_reader_for_decompression, utils::sized_buffer::SizedBuffer};
use anyhow::Result;
use chrono::{DateTime, Utc};
use docs_rs_headers::{ETag, compute_etag};
use docs_rs_types::CompressionAlgorithm;
use mime::Mime;
use std::io;
use tokio::io::{AsyncBufRead, AsyncBufReadExt};

/// represents a blob to be uploaded to storage.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlobUpload {
    pub path: String,
    pub mime: Mime,
    pub content: Vec<u8>,
    pub compression: Option<CompressionAlgorithm>,
}

impl From<Blob> for BlobUpload {
    fn from(value: Blob) -> Self {
        Self {
            path: value.path,
            mime: value.mime,
            content: value.content,
            compression: value.compression,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Blob {
    pub path: String,
    pub mime: Mime,
    pub date_updated: DateTime<Utc>,
    pub etag: Option<ETag>,
    pub content: Vec<u8>,
    pub compression: Option<CompressionAlgorithm>,
}

impl From<BlobUpload> for Blob {
    fn from(value: BlobUpload) -> Self {
        Self {
            path: value.path,
            mime: value.mime,
            date_updated: Utc::now(),
            etag: compute_etag(&value.content).into(),
            content: value.content,
            compression: value.compression,
        }
    }
}

pub struct StreamingBlob {
    pub path: String,
    pub mime: Mime,
    pub date_updated: DateTime<Utc>,
    pub etag: Option<ETag>,
    pub compression: Option<CompressionAlgorithm>,
    pub content_length: usize,
    pub content: Box<dyn AsyncBufRead + Unpin + Send>,
}

impl std::fmt::Debug for StreamingBlob {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StreamingBlob")
            .field("path", &self.path)
            .field("mime", &self.mime)
            .field("date_updated", &self.date_updated)
            .field("etag", &self.etag)
            .field("compression", &self.compression)
            .finish()
    }
}

impl StreamingBlob {
    /// wrap the content stream in a streaming decompressor according to the
    /// algorithm found in `compression` attribute.
    pub async fn decompress(mut self) -> Result<Self, io::Error> {
        let Some(alg) = self.compression else {
            return Ok(self);
        };

        self.content = wrap_reader_for_decompression(self.content, alg);

        // We fill the first bytes here to force the compressor to start decompressing.
        // This is because we want a failure here in this method when the data is corrupted,
        // so we can directly act on that, and users don't have any errors when they just
        // stream the data.
        // This won't _comsume_ the bytes. The user of this StreamingBlob will still be able
        // to stream the whole content.
        //
        // This doesn't work 100% of the time. We might get other i/o error here,
        // or the decompressor might stumble on corrupted data later during streaming.
        //
        // But: the most common error is that the format "magic bytes" at the beginning
        // of the stream are missing, and that's caught here.
        let decompressed_buf = self.content.fill_buf().await?;
        debug_assert!(
            !decompressed_buf.is_empty(),
            "we assume if we have > 0 decompressed bytes, start of the decompression works."
        );

        self.compression = None;
        // not touching the etag, it should represent the original content
        Ok(self)
    }

    /// consume the inner stream and materialize the full blob into memory.
    pub async fn materialize(mut self, max_size: usize) -> Result<Blob> {
        let mut content = SizedBuffer::new(max_size);
        content.reserve(self.content_length);

        tokio::io::copy(&mut self.content, &mut content).await?;

        Ok(Blob {
            path: self.path,
            mime: self.mime,
            date_updated: self.date_updated,
            etag: self.etag, // downloading doesn't change the etag
            content: content.into_inner(),
            compression: self.compression,
        })
    }
}

impl From<Blob> for StreamingBlob {
    fn from(value: Blob) -> Self {
        Self {
            path: value.path,
            mime: value.mime,
            date_updated: value.date_updated,
            etag: value.etag,
            compression: value.compression,
            content_length: value.content.len(),
            content: Box::new(io::Cursor::new(value.content)),
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::compress_async;
    use docs_rs_headers::compute_etag;

    const ZSTD_EOF_BYTES: [u8; 3] = [0x01, 0x00, 0x00];

    fn streaming_blob(
        content: impl Into<Vec<u8>>,
        alg: Option<CompressionAlgorithm>,
    ) -> StreamingBlob {
        let content = content.into();
        StreamingBlob {
            path: "some_path.db".into(),
            mime: mime::APPLICATION_OCTET_STREAM,
            date_updated: Utc::now(),
            compression: alg,
            etag: Some(compute_etag(&content)),
            content_length: content.len(),
            content: Box::new(io::Cursor::new(content)),
        }
    }

    #[tokio::test]
    async fn test_streaming_blob_uncompressed() -> Result<()> {
        const CONTENT: &[u8] = b"Hello, world!";

        // without decompression
        {
            let stream = streaming_blob(CONTENT, None);
            let blob = stream.materialize(usize::MAX).await?;
            assert_eq!(blob.content, CONTENT);
            assert!(blob.compression.is_none());
        }

        // with decompression, does nothing
        {
            let stream = streaming_blob(CONTENT, None);
            let blob = stream.decompress().await?.materialize(usize::MAX).await?;
            assert_eq!(blob.content, CONTENT);
            assert!(blob.compression.is_none());
        }

        Ok(())
    }

    #[tokio::test]
    async fn test_streaming_broken_zstd_blob() -> Result<()> {
        const NOT_ZSTD: &[u8] = b"Hello, world!";
        let alg = CompressionAlgorithm::Zstd;

        // without decompression
        // Doesn't fail because we don't call `.decompress`
        {
            let stream = streaming_blob(NOT_ZSTD, Some(alg));
            let blob = stream.materialize(usize::MAX).await?;
            assert_eq!(blob.content, NOT_ZSTD);
            assert_eq!(blob.compression, Some(alg));
        }

        // with decompression
        // should fail in the `.decompress` call,
        // not later when materializing / streaming.
        {
            let err = streaming_blob(NOT_ZSTD, Some(alg))
                .decompress()
                .await
                .unwrap_err();

            assert_eq!(err.kind(), io::ErrorKind::Other);

            assert_eq!(
                err.to_string(),
                "Unknown frame descriptor",
                "unexpected error: {}",
                err
            );
        }

        Ok(())
    }

    #[tokio::test]
    async fn test_streaming_blob_zstd() -> Result<()> {
        const CONTENT: &[u8] = b"Hello, world!";
        let mut compressed_content = Vec::new();
        let alg = CompressionAlgorithm::Zstd;
        compress_async(
            &mut io::Cursor::new(CONTENT.to_vec()),
            &mut compressed_content,
            alg,
        )
        .await?;

        // without decompression
        {
            let stream = streaming_blob(compressed_content.clone(), Some(alg));
            let blob = stream.materialize(usize::MAX).await?;
            assert_eq!(blob.content, compressed_content);
            assert_eq!(blob.content.last_chunk::<3>().unwrap(), &ZSTD_EOF_BYTES);
            assert_eq!(blob.compression, Some(alg));
        }

        // with decompression
        {
            let blob = streaming_blob(compressed_content.clone(), Some(alg))
                .decompress()
                .await?
                .materialize(usize::MAX)
                .await?;
            assert_eq!(blob.content, CONTENT);
            assert!(blob.compression.is_none());
        }

        Ok(())
    }
}
