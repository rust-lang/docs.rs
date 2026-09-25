use crate::{ReplacementDetails, ReplacementMap};
use bon::bon;
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
        #[builder(field)] replacements: Vec<(KrateName, Arc<ReplacementDetails>)>,
        #[builder(default = StatusCode::OK)] status_code: StatusCode,
        raw_body: Option<String>,
    ) -> Self {
        let map = ReplacementMap::from_iter(replacements);
        debug_assert!(
            raw_body.is_none() || map.is_empty(),
            "a mock cannot define both raw_body and replacements",
        );

        self.mocks.push(
            self.server
                .mock("GET", "/all.json")
                .with_status(status_code.as_u16().into())
                .with_body(raw_body.unwrap_or_else(|| serde_json::to_string(&map).unwrap()))
                .expect(1)
                .create_async()
                .await,
        );

        self
    }

    pub async fn assert_and_remove_mock(&mut self) {
        if let Some(mock) = self.mocks.pop() {
            mock.assert_async().await;
            mock.remove_async().await;
        }
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

use std_replacement_mock_server_create_mock_builder::State;

impl<S: State> StdReplacementMockServerCreateMockBuilder<S> {
    pub fn replacement(mut self, krate: KrateName, details: ReplacementDetails) -> Self {
        self.replacements.push((krate, Arc::new(details)));
        self
    }
}
