//! Database based file handler

use super::cache::CachePolicy;
use crate::{
    error::Result,
    storage::{AsyncStorage, Blob},
    Config,
};

use axum::{
    extract::Extension,
    http::{
        header::{CONTENT_TYPE, LAST_MODIFIED},
        StatusCode,
    },
    response::{IntoResponse, Response as AxumResponse},
};

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test::wrapper;
    use chrono::Utc;
    use http::header::CACHE_CONTROL;

    #[test]
    fn file_roundtrip_axum() {
        wrapper(|env| {
            let now = Utc::now();

            env.fake_release().create()?;

            let mut file = env
                .runtime()
                .block_on(File::from_path(
                    &env.runtime().block_on(env.async_storage()),
                    "rustdoc/fake-package/1.0.0/fake-package/index.html",
                    &env.config(),
                ))
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

        wrapper(|env| {
            env.override_config(|config| {
                config.max_file_size = MAX_SIZE;
                config.max_file_size_html = MAX_HTML_SIZE;
            });

            env.fake_release()
                .name("dummy")
                .version("0.1.0")
                .rustdoc_file_with("small.html", &[b'A'; MAX_HTML_SIZE / 2] as &[u8])
                .rustdoc_file_with("exact.html", &[b'A'; MAX_HTML_SIZE] as &[u8])
                .rustdoc_file_with("big.html", &[b'A'; MAX_HTML_SIZE * 2] as &[u8])
                .rustdoc_file_with("small.js", &[b'A'; MAX_SIZE / 2] as &[u8])
                .rustdoc_file_with("exact.js", &[b'A'; MAX_SIZE] as &[u8])
                .rustdoc_file_with("big.js", &[b'A'; MAX_SIZE * 2] as &[u8])
                .create()?;

            let file = |path| {
                env.runtime().block_on(File::from_path(
                    &env.runtime().block_on(env.async_storage()),
                    &format!("rustdoc/dummy/0.1.0/{path}"),
                    &env.config(),
                ))
            };
            let assert_len = |len, path| {
                assert_eq!(len, file(path).unwrap().0.content.len());
            };
            let assert_too_big = |path| {
                file(path)
                    .unwrap_err()
                    .downcast_ref::<std::io::Error>()
                    .and_then(|io| io.get_ref())
                    .and_then(|err| err.downcast_ref::<crate::error::SizeLimitReached>())
                    .is_some()
            };

            assert_len(MAX_HTML_SIZE / 2, "small.html");
            assert_len(MAX_HTML_SIZE, "exact.html");
            assert_len(MAX_SIZE / 2, "small.js");
            assert_len(MAX_SIZE, "exact.js");

            assert_too_big("big.html");
            assert_too_big("big.js");

            Ok(())
        })
    }
}
