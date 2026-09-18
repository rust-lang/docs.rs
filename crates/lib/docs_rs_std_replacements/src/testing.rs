//! Isolated HTTP fixture for replacement lookups.
use crate::{Config, ReplacementMap, StdReplacements};
use reqwest::{StatusCode, header::CONTENT_TYPE};
use std::sync::Arc;
use tokio::sync::Mutex;

struct Inner {
    server: mockito::ServerGuard,
    mocks: Vec<mockito::Mock>,
}

/// A replacement client backed by a local mock server, with retries disabled.
pub struct TestStdReplacements {
    inner: Mutex<Inner>,
    api: Arc<StdReplacements>,
    config: Arc<Config>,
}

impl TestStdReplacements {
    pub async fn new() -> anyhow::Result<Self> {
        let server = mockito::Server::new_async().await;
        let config = Arc::new(
            Config::builder()
                .url(server.url().parse()?)
                .max_retries(0)
                .build(),
        );
        let provider: docs_rs_opentelemetry::AnyMeterProvider =
            Arc::new(docs_rs_opentelemetry::NoopMeterProvider::new());
        let api = Arc::new(StdReplacements::from_config(&config, &provider)?);
        Ok(Self {
            inner: Mutex::new(Inner {
                server,
                mocks: Vec::new(),
            }),
            api,
            config,
        })
    }

    pub fn api(&self) -> &Arc<StdReplacements> {
        &self.api
    }

    pub fn test_config(&self) -> &Arc<Config> {
        &self.config
    }

    pub async fn assert_mocks(&self) {
        for mock in &self.inner.lock().await.mocks {
            mock.assert_async().await;
        }
    }

    /// Mock a successful standard-library replacement response.
    pub async fn mock_std_replacements(&self, replacements: ReplacementMap) {
        self.create_std_replacements_mock(move |mock| {
            mock.with_status(StatusCode::OK.as_u16().into())
                .with_header(CONTENT_TYPE, mime::APPLICATION_JSON.as_ref())
                .with_body(serde_json::to_vec(&replacements).unwrap())
        })
        .await;
    }

    /// Create a custom mock for the standard-library replacement `GET` request.
    ///
    /// The closure can configure the response body, status, and expected request count.
    /// The mock is checked by [`Self::assert_mocks`].
    pub async fn create_std_replacements_mock<F>(&self, mut f: F)
    where
        F: FnMut(mockito::Mock) -> mockito::Mock,
    {
        let mut inner = self.inner.lock().await;
        let mock = f(inner.server.mock("GET", "/")).create_async().await;
        inner.mocks.push(mock);
    }
}
