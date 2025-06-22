//! Database based file handler

use super::cache::CachePolicy;
use crate::{
    Config,
    error::Result,
    storage::{AsyncStorage, Blob, StreamingBlob},
};
use axum::{
    body::Body,
    extract::Extension,
    http::{
        StatusCode,
        header::{CONTENT_TYPE, LAST_MODIFIED},
    },
    response::{IntoResponse, Response as AxumResponse},
};
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
        let max_size = if path.ends_with(".html") {
            config.max_file_size_html
        } else {
            config.max_file_size
        };

        Ok(File(storage.get(path, max_size).await?))
    }
}

impl IntoResponse for File {
    fn into_response(self) -> AxumResponse {
        (
            StatusCode::OK,
            [
                (CONTENT_TYPE, self.0.mime.as_ref()),
                (
                    LAST_MODIFIED,
                    &self.0.date_updated.format("%a, %d %b %Y %T %Z").to_string(),
                ),
            ],
            Extension(CachePolicy::ForeverInCdnAndBrowser),
            self.0.content,
        )
            .into_response()
    }
}

#[derive(Debug)]
pub(crate) struct StreamingFile(pub(crate) StreamingBlob);

impl StreamingFile {
    /// Gets file from database
    pub(super) async fn from_path(storage: &AsyncStorage, path: &str) -> Result<StreamingFile> {
        Ok(StreamingFile(storage.get_stream(path).await?))
    }
}

impl IntoResponse for StreamingFile {
    fn into_response(self) -> AxumResponse {
        // Convert the AsyncBufRead into a Stream of Bytes
        let stream = ReaderStream::new(self.0.content);
        let body = Body::from_stream(stream);
        (
            StatusCode::OK,
            [
                (CONTENT_TYPE, self.0.mime.as_ref()),
                (
                    LAST_MODIFIED,
                    &self.0.date_updated.format("%a, %d %b %Y %T %Z").to_string(),
                ),
            ],
            Extension(CachePolicy::ForeverInCdnAndBrowser),
            body,
        )
            .into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test::async_wrapper;
    use chrono::Utc;
    use http::header::CACHE_CONTROL;

    #[test]
    fn file_roundtrip_axum() {
        async_wrapper(|env| async move {
            let now = Utc::now();

            env.fake_release().await.create().await?;

            let mut file = File::from_path(
                &*env.async_storage().await,
                "rustdoc/fake-package/1.0.0/fake-package/index.html",
                &env.config(),
            )
            .await
            .unwrap();
            file.0.date_updated = now;

            let resp = file.into_response();
            assert!(resp.headers().get(CACHE_CONTROL).is_none());
            let cache = resp
                .extensions()
                .get::<CachePolicy>()
                .expect("missing cache response extension");
            assert!(matches!(cache, CachePolicy::ForeverInCdnAndBrowser));
            assert_eq!(
                resp.headers().get(LAST_MODIFIED).unwrap(),
                &now.format("%a, %d %b %Y %T UTC").to_string(),
            );

            Ok(())
        });
    }

    #[test]
    fn test_max_size() {
        const MAX_SIZE: usize = 1024;
        const MAX_HTML_SIZE: usize = 128;

        async_wrapper(|env| async move {
            env.override_config(|config| {
                config.max_file_size = MAX_SIZE;
                config.max_file_size_html = MAX_HTML_SIZE;
            });

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
                        &*env.async_storage().await,
                        &format!("rustdoc/dummy/0.1.0/{path}"),
                        &env.config(),
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
        })
    }
}
