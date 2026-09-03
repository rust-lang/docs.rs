use crate::{
    Config, RegistryApi, SearchCursor,
    models::{ApiError, ApiErrors, SearchCrate, SearchMeta, SearchResponse},
};
use anyhow::Result;
use bon::bon;
use docs_rs_types::KrateName;
use docs_rs_utils::spawn_blocking;
use mockito::Matcher;
use reqwest::{StatusCode, header::CONTENT_TYPE};
use serde::Serialize;
use std::sync::Arc;
use tokio::sync::Mutex;
use url::Url;

struct TestRegistryInner {
    api_server: mockito::ServerGuard,
    index_server: mockito::ServerGuard,
    #[allow(dead_code)]
    download_server: mockito::ServerGuard,
    mocks: Vec<mockito::Mock>,
}

/// A local registry fixture backed by isolated mock HTTP servers.
///
/// It provides a [`RegistryApi`] configured to use a temporary sparse-index cache, API server,
/// and download server. Add only the responses required by the test; unknown sparse-index crate
/// entries behave as not found.
pub struct TestRegistry {
    #[allow(dead_code)]
    cargo_home: tempfile::TempDir,
    inner: Mutex<TestRegistryInner>,
    api: Arc<RegistryApi>,
    config: Arc<Config>,
}

#[bon]
impl TestRegistry {
    pub async fn new() -> Result<Self> {
        Self::builder().build().await
    }

    #[builder(
        finish_fn(name = build),
        on(_, into)
    )]
    pub(crate) async fn builder(
        /// set http retries in case of errors.
        /// Only needed for testing the retry behaviour in this crate.
        /// The workspace-wide `TestRegistry` doesn't retry.
        #[builder(default)]
        retries: u32,

        /// custom index config, in case we want to test broken index config
        /// error handling.
        /// By default, we'll generate a correct `IndexConfig`, that fits
        /// to the local http mocks we set up.
        index_config: Option<crates_index::IndexConfig>,
    ) -> Result<Self> {
        let cargo_home = spawn_blocking(|| Ok(tempfile::tempdir()?)).await?;
        let api_server = mockito::Server::new_async().await;
        let mut index_server = mockito::Server::new_async().await;
        let download_server = mockito::Server::new_async().await;

        let index_config = index_config.unwrap_or_else(|| crates_index::IndexConfig {
            dl: format!("{}/crates", download_server.url()),
            api: Some(api_server.url()),
        });

        // Mockito chooses matching mocks that still expect hits first, then the last match. An
        // optional fallback registered first therefore yields to the config and crate mocks below.
        let index_object_missing_mock = index_server
            .mock("GET", Matcher::Any)
            .expect_at_least(0)
            .with_status(StatusCode::NOT_FOUND.as_u16().into())
            .create_async()
            .await;

        let config_mock = index_server
            .mock("GET", "/config.json")
            .with_header(CONTENT_TYPE, mime::APPLICATION_JSON.as_ref())
            .with_body(serde_json::to_vec(&serde_json::json!({
                "dl": index_config.dl,
                "api": index_config.api,
            }))?)
            .create_async()
            .await;

        // NOTE: cargo expects the `sparse+` schema prefix so it can differentiate
        // the sparse index url from a git-http URL.
        let index_url: Url = format!("sparse+{}", index_server.url()).parse()?;

        let api = RegistryApi::new(index_url.clone(), retries, Some(cargo_home.path())).await?;

        let config = Arc::new(
            Config::builder()
                .crates_io_api_call_retries(retries)
                .sparse_index_host(index_url)
                .build(),
        );

        Ok(Self {
            cargo_home,
            inner: Mutex::new(TestRegistryInner {
                api_server,
                index_server,
                download_server,
                mocks: vec![config_mock, index_object_missing_mock],
            }),
            api: Arc::new(api),
            config,
        })
    }

    /// dummy config for the global context.
    ///
    /// Matches the setup of the `TestEnvironment`.
    pub fn test_config(&self) -> &Arc<Config> {
        &self.config
    }

    /// Mock the sparse-index response for `krate`.
    ///
    /// Each item in `versions` is one JSON line from a sparse-index crate entry.
    pub async fn mock_index_response<L>(
        &self,
        krate: &KrateName,
        versions: impl IntoIterator<Item = L>,
    ) where
        L: Serialize,
    {
        let crate_url: Url = self
            .api
            .sparse_index
            .crate_url(krate.as_str())
            .unwrap()
            .parse()
            .unwrap();

        let payload = versions
            .into_iter()
            .map(|v| serde_json::to_string(&v).unwrap())
            .collect::<Vec<_>>()
            .join("\n");

        let mut inner = self.inner.lock().await;

        let index_mock = inner
            .index_server
            .mock("GET", crate_url.path())
            .with_status(StatusCode::OK.as_u16().into())
            .with_body(payload)
            .create_async()
            .await;

        inner.mocks.push(index_mock);
    }

    /// Mock downloading a crate archive from the URL advertised by the sparse index.
    pub async fn mock_download(
        &self,
        krate: &KrateName,
        version: &docs_rs_types::Version,
        archive: Vec<u8>,
    ) {
        let url = self.api.download_url(krate, version).unwrap();

        let mut inner = self.inner.lock().await;
        let mock = inner
            .download_server
            .mock("GET", url.path())
            .with_status(StatusCode::OK.as_u16().into())
            .with_body(archive)
            .create_async()
            .await;
        inner.mocks.push(mock);
    }

    /// Create a custom mock for a registry API `GET` request.
    ///
    /// `path` may include a query string, whose URL-encoded pairs are matched independently of
    /// their order. The closure can configure status, headers, and response body on the
    /// underlying Mockito mock.
    pub(crate) async fn create_api_mock<F>(&self, path: impl AsRef<str>, mut f: F)
    where
        F: FnMut(mockito::Mock) -> mockito::Mock,
    {
        let url = Url::parse("http://registry.test")
            .unwrap()
            .join(path.as_ref())
            .unwrap();
        let query_matchers: Vec<_> = url
            .query_pairs()
            .map(|(name, value)| Matcher::UrlEncoded(name.into_owned(), value.into_owned()))
            .collect();

        let mut inner = self.inner.lock().await;
        let mut mock = inner.api_server.mock("GET", url.path());
        if !query_matchers.is_empty() {
            mock = mock.match_query(Matcher::AllOf(query_matchers));
        }
        let mock = f(mock).create_async().await;
        inner.mocks.push(mock);
    }

    /// Mock the registry search API with an error response.
    ///
    /// first, call either `client_error` or `server_error`.
    ///
    /// Calling either disables the other.
    ///
    /// Then, you can call `api_errors`, `api_error_messages`
    /// or `error_text` (for client error / server error) for
    /// the body of the response.
    ///
    /// You can also just call `.create`, without defining client/server error, then
    /// we'll mock a INTERNAL_SERVER_ERROR.
    #[builder(
        builder_type = SearchErrorMockBuilder,
        state_mod = search_error_mock_builder,
        finish_fn = create
    )]
    pub async fn mock_search_error<C>(
        &self,
        #[builder(start_fn)] cursor: C,

        #[builder(
            setters(vis = "", name = client_status_internal)
        )]
        client_status: Option<StatusCode>,

        #[builder(
            into,
            setters(vis = "", name = api_errors_internal)
        )]
        api_errors: Option<ApiErrors>,

        #[builder(
            default = StatusCode::INTERNAL_SERVER_ERROR,
            setters(vis = "", name = server_status_internal)
        )]
        server_status: StatusCode,

        #[builder(
            default,
            into,
            setters(vis = "", name = error_text_internal)
        )]
        error_text: String,
    ) where
        C: Into<SearchCursor>,
    {
        let cursor = cursor.into();

        self.create_api_mock(format!("/api/v1/crates{cursor}"), move |mock| {
            // NOTE: the builder ensures at compile or build-time that
            // * either client status or server status are set
            // * any status that is set, is a correct client/server status.
            if let Some(status) = client_status {
                let mock = mock.with_status(status.as_u16().into());

                if let Some(api_errors) = api_errors.as_ref() {
                    mock.with_header(CONTENT_TYPE, mime::APPLICATION_JSON.as_ref())
                        .with_body(serde_json::to_vec(&api_errors).unwrap())
                } else {
                    // without error messages, the body should by empty
                    mock
                }
            } else {
                mock.with_status(server_status.as_u16().into())
                    .with_header(CONTENT_TYPE, mime::TEXT_PLAIN.as_ref())
                    .with_body(&error_text)
            }
        })
        .await;
    }

    /// Mock the registry search API with domain data instead of a raw HTTP response.
    ///
    /// The cursor's URL-encoded pairs are matched independently of their order.
    #[builder(
        builder_type = SearchMockBuilder,
        state_mod = search_mock_builder,
        finish_fn = create
    )]
    pub async fn mock_search<C>(
        &self,
        #[builder(start_fn)] cursor: C,
        search_result: Vec<SearchCrate>,
        next_page: Option<SearchCursor>,
        prev_page: Option<SearchCursor>,
    ) where
        C: Into<SearchCursor>,
    {
        let cursor = cursor.into();

        let response = SearchResponse {
            crates: Some(search_result),
            meta: Some(SearchMeta {
                next_page,
                prev_page,
            }),
        };

        self.create_api_mock(format!("/api/v1/crates{cursor}"), move |mock| {
            mock.with_status(StatusCode::OK.as_u16().into())
                .with_header(CONTENT_TYPE, mime::APPLICATION_JSON.as_ref())
                .with_body(serde_json::to_vec(&response).unwrap())
        })
        .await;
    }

    /// Assert that every required mock received its expected requests.
    pub async fn assert_mocks(&self) {
        let inner = self.inner.lock().await;

        for mock in &inner.mocks {
            mock.assert_async().await;
        }
    }

    /// Return the registry client configured for this fixture.
    pub fn api(&self) -> &Arc<RegistryApi> {
        &self.api
    }
}

use search_mock_builder::{IsUnset, SetSearchResult, State};

/// Extends the builder that is returned from `mock_search`.
impl<'a, C, S> SearchMockBuilder<'a, C, S>
where
    C: Into<SearchCursor>,
    S: State,
{
    pub fn crate_names<I, N>(self, names: I) -> SearchMockBuilder<'a, C, SetSearchResult<S>>
    where
        N: Into<String>,
        I: IntoIterator<Item = N>,
        S::SearchResult: IsUnset,
    {
        let crates: Vec<_> = names
            .into_iter()
            .map(|name| SearchCrate { name: name.into() })
            .collect();

        self.search_result(crates)
    }
}

use search_error_mock_builder::{
    IsSet as EIsSet, IsUnset as EIsUnset, SetApiErrors, SetClientStatus, SetErrorText,
    SetServerStatus, State as EState,
};

/// Extends the builder that is returned from `mock_search_error`.
impl<'a, C, S> SearchErrorMockBuilder<'a, C, S>
where
    C: Into<SearchCursor>,
    S: EState,
{
    pub fn client_error(
        self,
        status: StatusCode,
    ) -> SearchErrorMockBuilder<'a, C, SetClientStatus<S>>
    where
        S::ClientStatus: EIsUnset,
        S::ApiErrors: EIsUnset,
        S::ServerStatus: EIsUnset,
        S::ErrorText: EIsUnset,
    {
        assert!(status.is_client_error());
        self.client_status_internal(status)
    }

    pub fn api_errors(self, api_errors: ApiErrors) -> SearchErrorMockBuilder<'a, C, SetApiErrors<S>>
    where
        S::ClientStatus: EIsSet,
        S::ApiErrors: EIsUnset,
        S::ServerStatus: EIsUnset,
        S::ErrorText: EIsUnset,
    {
        self.api_errors_internal(api_errors)
    }

    pub fn api_error_messages<I, E>(
        self,
        api_error_messages: I,
    ) -> SearchErrorMockBuilder<'a, C, SetApiErrors<S>>
    where
        E: Into<String>,
        I: IntoIterator<Item = E>,
        S::ClientStatus: EIsSet,
        S::ApiErrors: EIsUnset,
        S::ServerStatus: EIsUnset,
        S::ErrorText: EIsUnset,
    {
        let errors: Vec<_> = api_error_messages
            .into_iter()
            .map(|e| ApiError {
                detail: Some(e.into()),
            })
            .collect();

        self.api_errors(ApiErrors { errors })
    }

    pub fn server_error(
        self,
        status: StatusCode,
    ) -> SearchErrorMockBuilder<'a, C, SetServerStatus<S>>
    where
        S::ClientStatus: EIsUnset,
        S::ApiErrors: EIsUnset,
        S::ServerStatus: EIsUnset,
        S::ErrorText: EIsUnset,
    {
        assert!(status.is_server_error());
        self.server_status_internal(status)
    }

    pub fn error_text<E>(self, error_text: E) -> SearchErrorMockBuilder<'a, C, SetErrorText<S>>
    where
        E: Into<String>,
        S::ClientStatus: EIsUnset,
        S::ApiErrors: EIsUnset,
        S::ServerStatus: EIsSet,
        S::ErrorText: EIsUnset,
    {
        self.error_text_internal(error_text.into())
    }
}
