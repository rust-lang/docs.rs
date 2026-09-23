use crate::{ReplacementDetails, ReplacementMap};
use bon::bon;
use docs_rs_headers::CacheControl;
use docs_rs_reqwest::testing::MockExt as _;
use docs_rs_types::KrateName;
use http::StatusCode;
use std::sync::Arc;

pub struct StdReplacementMockServer {
    server: mockito::ServerGuard,
    mocks: Vec<mockito::Mock>,
}

#[bon]
impl StdReplacementMockServer {
    pub async fn new() -> Self {
        Self {
            server: mockito::Server::new_async().await,
            mocks: Vec::new(),
        }
    }

    #[builder(start_fn(name = mock), finish_fn(name = start))]
    pub async fn create_mock(
        mut self,
        #[builder(with = |krate: KrateName, details: ReplacementDetails| (krate, details))]
        replacement: Option<(KrateName, ReplacementDetails)>,
        #[builder(default, with = FromIterator::from_iter)] replacements: Vec<(
            KrateName,
            ReplacementDetails,
        )>,
        cache_control: Option<CacheControl>,
        #[builder(default = StatusCode::OK)] status_code: StatusCode,
    ) -> Self {
        let map = ReplacementMap::from_iter(
            replacements
                .into_iter()
                .chain(replacement)
                .map(|(krate, details)| (krate, Arc::new(details))),
        );

        let mut mock = self
            .server
            .mock("GET", "/all.json")
            .with_status_code(status_code);

        if let Some(cache_control) = cache_control {
            mock = mock.with_typed_header(cache_control);
        }

        self.mocks.push(
            mock.with_body(serde_json::to_string(&map).unwrap())
                .expect(1)
                .create_async()
                .await,
        );

        self
    }

    pub fn remove_mock(&mut self) {
        self.mocks.pop();
    }

    pub fn config(&self) -> crate::ConfigBuilder {
        crate::Config::builder()
            .url(format!("{}/all.json", self.server.url()).parse().unwrap())
            .max_retries(0)
    }

    pub async fn assert_async(self) {
        for mock in self.mocks {
            mock.assert_async().await;
        }
    }
}
