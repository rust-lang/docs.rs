use bon::bon;
use docs_rs_headers::CacheControl;
use docs_rs_reqwest::testing::MockExt as _;
use docs_rs_types::KrateName;
use http::StatusCode;

pub struct RustsecMockServer {
    server: mockito::ServerGuard,
    mocks: Vec<mockito::Mock>,
}

#[bon]
impl RustsecMockServer {
    pub async fn new() -> Self {
        Self {
            server: mockito::Server::new_async().await,
            mocks: Vec::new(),
        }
    }

    #[builder(start_fn(name = mock), finish_fn(name = start))]
    pub async fn create_mock(
        mut self,
        #[builder(start_fn)] krate: KrateName,
        #[builder(default = StatusCode::OK)] status_code: StatusCode,
        #[builder(default = false)] empty: bool,
        cache_control: Option<CacheControl>,
        #[builder(into)] body: Option<String>,
    ) -> Self {
        let mut mock = self
            .server
            .mock("GET", format!("/packages/{krate}.json").as_str())
            .with_status_code(status_code);

        if let Some(cache_control) = cache_control {
            mock = mock.with_typed_header(cache_control);
        }

        let empty = empty || (status_code.is_client_error() || status_code.is_server_error());

        let body = body.unwrap_or_else(|| {
            if empty {
                String::new()
            } else {
                include_str!("../../tests/fixtures/owned-alloc.json").to_owned()
            }
        });
        self.mocks
            .push(mock.with_body(body).expect(1).create_async().await);

        self
    }

    pub fn config(&self) -> crate::ConfigBuilder {
        crate::Config::builder()
            .base_url(self.server.url().parse().unwrap())
            .max_retries(0)
    }

    pub async fn assert_async(self) {
        for mock in self.mocks {
            mock.assert_async().await;
        }
    }
}
