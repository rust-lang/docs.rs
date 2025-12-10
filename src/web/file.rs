//! Database based file handler

use super::cache::CachePolicy;
use crate::{Config, error::Result};
use axum::{
    body::Body,
    extract::Extension,
    http::StatusCode,
    response::{IntoResponse, Response as AxumResponse},
};
use axum_extra::{
    TypedHeader,
    headers::{ContentType, LastModified},
};
use docs_rs_headers::IfNoneMatch;
use docs_rs_storage::{AsyncStorage, Blob, StreamingBlob};
use std::time::SystemTime;
use tokio_util::io::ReaderStream;

#[derive(Debug)]
pub(crate) struct File(pub(crate) Blob);

impl File {
    /// Gets file from database
    pub(super) async fn from_path(
        storage: &AsyncStorage,
        path: &str,
        config: &Config,
    ) -> Result<File> {
        Ok(File(
            storage
                .get(path, config.storage.max_file_size_for(path))
                .await?,
        ))
    }
}

#[cfg(test)]
impl File {
    pub fn into_response(self, if_none_match: Option<&IfNoneMatch>) -> AxumResponse {
        let streaming_blob: StreamingBlob = self.0.into();
        StreamingFile(streaming_blob).into_response(if_none_match)
    }
}

#[derive(Debug)]
pub(crate) struct StreamingFile(pub(crate) StreamingBlob);

impl StreamingFile {
    /// Gets file from database
    pub(super) async fn from_path(storage: &AsyncStorage, path: &str) -> Result<StreamingFile> {
        Ok(StreamingFile(storage.get_stream(path).await?))
    }

    pub fn into_response(self, if_none_match: Option<&IfNoneMatch>) -> AxumResponse {
        const CACHE_POLICY: CachePolicy = CachePolicy::ForeverInCdnAndBrowser;
        let last_modified = LastModified::from(SystemTime::from(self.0.date_updated));

        if let Some(if_none_match) = if_none_match
            && let Some(ref etag) = self.0.etag
            && !if_none_match.precondition_passes(etag)
        {
            (
                StatusCode::NOT_MODIFIED,
                // it's generally recommended to repeat caching headers on 304 responses
                TypedHeader(etag.clone()),
                TypedHeader(last_modified),
                Extension(CACHE_POLICY),
            )
                .into_response()
        } else {
            // Convert the AsyncBufRead into a Stream of Bytes
            let stream = ReaderStream::new(self.0.content);

            (
                StatusCode::OK,
                TypedHeader(ContentType::from(self.0.mime)),
                TypedHeader(last_modified),
                self.0.etag.map(TypedHeader),
                Extension(CACHE_POLICY),
                Body::from_stream(stream),
            )
                .into_response()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{storage::CompressionAlgorithm, test::TestEnvironment, web::headers::compute_etag};
    use axum_extra::headers::{ETag, HeaderMapExt as _};
    use chrono::Utc;
    use http::header::{CACHE_CONTROL, ETAG, LAST_MODIFIED};
    use std::{io, rc::Rc};

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
    async fn test_stream_into_response() -> Result<()> {
        const CONTENT: &[u8] = b"Hello, world!";
        let etag: ETag = {
            // first request normal
            let stream = StreamingFile(streaming_blob(CONTENT, None));
            let resp = stream.into_response(None);
            assert!(resp.status().is_success());
            assert!(resp.headers().get(CACHE_CONTROL).is_none());
            let cache = resp
                .extensions()
                .get::<CachePolicy>()
                .expect("missing cache response extension");
            assert!(matches!(cache, CachePolicy::ForeverInCdnAndBrowser));
            assert!(resp.headers().get(LAST_MODIFIED).is_some());

            resp.headers().typed_get().unwrap()
        };

        let if_none_match = IfNoneMatch::from(etag);

        {
            // cached request
            let stream = StreamingFile(streaming_blob(CONTENT, None));
            let resp = stream.into_response(Some(&if_none_match));
            assert_eq!(resp.status(), StatusCode::NOT_MODIFIED);

            // cache related headers are repeated on the not-modified response
            assert!(resp.headers().get(CACHE_CONTROL).is_none());
            let cache = resp
                .extensions()
                .get::<CachePolicy>()
                .expect("missing cache response extension");
            assert!(matches!(cache, CachePolicy::ForeverInCdnAndBrowser));
            assert!(resp.headers().get(LAST_MODIFIED).is_some());
            assert!(resp.headers().get(ETAG).is_some());
        }

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn file_roundtrip_axum() -> Result<()> {
        let env = TestEnvironment::new().await?;

        let now = Utc::now();

        env.fake_release().await.create().await?;

        let mut file = File::from_path(
            env.async_storage(),
            "rustdoc/fake-package/1.0.0/fake-package/index.html",
            env.config(),
        )
        .await?;

        file.0.date_updated = now;

        let resp = file.into_response(None);
        assert!(resp.status().is_success());
        assert!(resp.headers().get(CACHE_CONTROL).is_none());
        let cache = resp
            .extensions()
            .get::<CachePolicy>()
            .expect("missing cache response extension");
        assert!(matches!(cache, CachePolicy::ForeverInCdnAndBrowser));
        assert_eq!(
            resp.headers().get(LAST_MODIFIED).unwrap(),
            &now.format("%a, %d %b %Y %T GMT").to_string(),
        );

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_max_size() -> Result<()> {
        const MAX_SIZE: usize = 1024;
        const MAX_HTML_SIZE: usize = 128;

        let env = Rc::new(
            TestEnvironment::with_config(
                TestEnvironment::base_config()
                    .max_file_size(MAX_SIZE)
                    .max_file_size_html(MAX_HTML_SIZE)
                    .build()?,
            )
            .await?,
        );

        env.fake_release()
            .await
            .name("dummy")
            .version("0.1.0")
            .rustdoc_file_with("small.html", &[b'A'; MAX_HTML_SIZE / 2] as &[u8])
            .rustdoc_file_with("exact.html", &[b'A'; MAX_HTML_SIZE] as &[u8])
            .rustdoc_file_with("big.html", &[b'A'; MAX_HTML_SIZE * 2] as &[u8])
            .rustdoc_file_with("small.js", &[b'A'; MAX_SIZE / 2] as &[u8])
            .rustdoc_file_with("exact.js", &[b'A'; MAX_SIZE] as &[u8])
            .rustdoc_file_with("big.js", &[b'A'; MAX_SIZE * 2] as &[u8])
            .create()
            .await?;

        let file = |path| {
            let env = env.clone();
            async move {
                File::from_path(
                    env.async_storage(),
                    &format!("rustdoc/dummy/0.1.0/{path}"),
                    env.config(),
                )
                .await
            }
        };
        let assert_len = |len, path| async move {
            assert_eq!(len, file(path).await.unwrap().0.content.len());
        };
        let assert_too_big = |path| async move {
            file(path)
                .await
                .unwrap_err()
                .downcast_ref::<std::io::Error>()
                .and_then(|io| io.get_ref())
                .and_then(|err| err.downcast_ref::<crate::error::SizeLimitReached>())
                .is_some()
        };

        assert_len(MAX_HTML_SIZE / 2, "small.html").await;
        assert_len(MAX_HTML_SIZE, "exact.html").await;
        assert_len(MAX_SIZE / 2, "small.js").await;
        assert_len(MAX_SIZE, "exact.js").await;

        assert_too_big("big.html").await;
        assert_too_big("big.js").await;

        Ok(())
    }
}
