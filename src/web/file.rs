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
use tracing::warn;

// https://docs.fastly.com/products/compute-resource-limits#default-limits
// https://www.fastly.com/documentation/guides/full-site-delivery/performance/failure-modes-with-large-objects/
// https://www.fastly.com/documentation/guides/full-site-delivery/caching/segmented-caching/
const FASTLY_CACHE_MAX_OBJECT_SIZE: usize = 100 * 1024 * 1024; // 100 MB

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
    pub fn into_response(
        self,
        if_none_match: Option<&IfNoneMatch>,
        cache_policy: CachePolicy,
    ) -> AxumResponse {
        let streaming_blob: StreamingBlob = self.0.into();
        StreamingFile(streaming_blob).into_response(if_none_match, cache_policy)
    }
}

#[derive(Debug)]
pub(crate) struct StreamingFile(pub(crate) StreamingBlob);

impl StreamingFile {
    /// Gets file from database
    pub(super) async fn from_path(storage: &AsyncStorage, path: &str) -> Result<StreamingFile> {
        Ok(StreamingFile(storage.get_stream(path).await?))
    }

    pub fn into_response(
        self,
        if_none_match: Option<&IfNoneMatch>,
        mut cache_policy: CachePolicy,
    ) -> AxumResponse {
        // by default Fastly can only cache objects up to 100 MiB.
        // Since we're streaming the response via chunked encoding, fastly itself doesn't know
        // the object size until the streamed data size is > 100 MiB. In this case fastly just
        // cuts the connection.
        // To avoid issues with caching large files, we disable CDN caching for files that are too
        // big.
        //
        // See:
        //   https://docs.fastly.com/products/compute-resource-limits#default-limits
        //   https://www.fastly.com/documentation/guides/full-site-delivery/performance/failure-modes-with-large-objects/
        //   https://www.fastly.com/documentation/guides/full-site-delivery/caching/segmented-caching/
        //
        // For now I use the `NoStoreMustRevalidate` policy, the important cache-control statement
        // is only the `no-store` part.
        //
        // Future optimization could be:
        // * only forbid fastly to store, and browsers still could.
        // * implement segmented caching for large files somehow.
        if self.0.content_length > FASTLY_CACHE_MAX_OBJECT_SIZE
            && !matches!(cache_policy, CachePolicy::NoStoreMustRevalidate)
        {
            warn!(
                storage_path = self.0.path,
                content_length = self.0.content_length,
                "Disabling CDN caching for large file"
            );
            cache_policy = CachePolicy::NoStoreMustRevalidate;
        }

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
                Extension(cache_policy),
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
                Extension(cache_policy),
                Body::from_stream(stream),
            )
                .into_response()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{test::TestEnvironment, web::cache::STATIC_ASSET_CACHE_POLICY};
    use axum_extra::headers::{ETag, HeaderMapExt as _};
    use chrono::Utc;
    use docs_rs_headers::compute_etag;
    use docs_rs_storage::StorageKind;
    use docs_rs_types::CompressionAlgorithm;
    use http::header::{CACHE_CONTROL, ETAG, LAST_MODIFIED};
    use std::{io, rc::Rc};

    const CONTENT: &[u8] = b"Hello, world!";

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

    #[test]
    fn test_big_file_stream_drops_cache_policy() {
        let mut stream = streaming_blob(CONTENT, None);
        stream.content_length = FASTLY_CACHE_MAX_OBJECT_SIZE + 1;

        let response =
            StreamingFile(stream).into_response(None, CachePolicy::ForeverInCdnAndBrowser);
        // even though we passed a cache policy in `into_response`, it should be overridden to
        // `NoCaching` due to the large size of the file.
        let cache = response
            .extensions()
            .get::<CachePolicy>()
            .expect("missing cache response extension");
        assert!(matches!(cache, CachePolicy::NoStoreMustRevalidate));
    }

    #[tokio::test]
    async fn test_stream_into_response() -> Result<()> {
        let etag: ETag = {
            // first request normal
            let stream = StreamingFile(streaming_blob(CONTENT, None));
            let resp = stream.into_response(None, STATIC_ASSET_CACHE_POLICY);
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
            let resp = stream.into_response(Some(&if_none_match), STATIC_ASSET_CACHE_POLICY);
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

        let resp = file.into_response(None, STATIC_ASSET_CACHE_POLICY);
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
                    .storage(
                        docs_rs_storage::Config::test_config(StorageKind::Memory)?
                            .set(|mut cfg| {
                                cfg.max_file_size = MAX_SIZE;
                                cfg.max_file_size_html = MAX_HTML_SIZE;
                                cfg
                            })
                            .into(),
                    )
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
                .and_then(|err| err.downcast_ref::<docs_rs_storage::SizeLimitReached>())
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
