use crate::{
    Config, RegistryApi, SearchCursor,
    api::DOCSRS_AUTHORIZATION,
    models::{ApiError, ApiErrors, SearchCrate, SearchMeta, SearchResponse},
};
use anyhow::{Context as _, Result};
use bon::bon;
use docs_rs_types::KrateName;
use docs_rs_utils::spawn_blocking;
use gix::{ObjectId, objs::tree::EntryKind};
use mockito::Matcher;
use reqwest::{StatusCode, header::CONTENT_TYPE};
use serde::Serialize;
use std::{
    collections::{BTreeMap, HashMap},
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio::sync::Mutex;
use url::Url;

struct TestRegistryInner {
    api_server: mockito::ServerGuard,
    index_server: mockito::ServerGuard,
    #[allow(dead_code)]
    download_server: mockito::ServerGuard,
    // Some mock have a unique path whwen you mock twice,
    // we'll drop the old mocks.
    crate_mocks: HashMap<String, mockito::Mock>,
    other_mocks: Vec<mockito::Mock>,
    index_config: String,
    git_index: Option<PathBuf>,
}

/// A local registry fixture backed by isolated mock HTTP servers.
///
/// It provides a [`RegistryApi`] configured to use a temporary sparse-index cache, API server,
/// and download server. Add only the responses required by the test; unknown sparse-index crate
/// entries behave as not found. Enable the optional Git index to mirror those
/// entries into a local repository with a commit for each update.
pub struct TestRegistry {
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

        /// Optional token for registry API requests.
        token: Option<http::HeaderValue>,

        /// custom index config, in case we want to test broken index config
        /// error handling.
        /// By default, we'll generate a correct `IndexConfig`, that fits
        /// to the local http mocks we set up.
        index_config: Option<crates_index::IndexConfig>,
        meter_provider: Option<docs_rs_opentelemetry::AnyMeterProvider>,
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

        // Persistent Cargo/Rustwide workspaces cache registry configuration by URL.
        // Ports can be reused after a test exits, so include the unique fixture directory
        // in the index URL to avoid reusing a previous registry's download endpoint.
        let index_prefix = cargo_home.path().file_name().unwrap().to_str().unwrap();
        let config_path = format!("/{index_prefix}/config.json");
        let index_config_payload = serde_json::to_string(&serde_json::json!({
            "dl": index_config.dl,
            "api": index_config.api,
        }))?;

        let config_mock = index_server
            .mock("GET", config_path.as_str())
            .match_header(DOCSRS_AUTHORIZATION.as_str(), Matcher::Missing)
            .expect_at_least(1)
            .with_header(CONTENT_TYPE, mime::APPLICATION_JSON.as_ref())
            .with_body(index_config_payload.clone())
            .create_async()
            .await;

        // NOTE: cargo expects the `sparse+` schema prefix so it can differentiate
        // the sparse index url from a git-http URL.
        let index_url: Url = format!("sparse+{}/{index_prefix}/", index_server.url()).parse()?;

        let meter_provider = meter_provider
            .unwrap_or_else(|| Arc::new(docs_rs_opentelemetry::NoopMeterProvider::new()));

        let api = RegistryApi::new(
            index_url.clone(),
            retries,
            token.clone(),
            Some(cargo_home.path()),
            &meter_provider,
        )
        .await?;

        let config = Arc::new(
            Config::builder()
                .maybe_api_token(token)
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
                crate_mocks: HashMap::from_iter([(config_path, config_mock)]),
                other_mocks: vec![index_object_missing_mock],
                index_config: index_config_payload,
                git_index: None,
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
    /// Replaces any previous response for this crate and, when Git is enabled,
    /// commits the same payload to its index entry. An empty list returns 404
    /// and removes the Git entry instead of writing an empty file.
    pub async fn mock_index_response<L>(
        &self,
        krate: &KrateName,
        versions: impl IntoIterator<Item = L>,
    ) where
        L: Serialize,
    {
        let mut payload = Vec::with_capacity(4096);

        for version in versions {
            if !payload.is_empty() {
                payload.push(b'\n');
            }
            serde_json::to_writer(&mut payload, &version).unwrap();
        }

        self.mock_index_payload(krate, Some(payload)).await;
    }

    /// Enable a local Git index and return its `file://` remote URL.
    /// Starts with a config-only commit; later updates create one commit each.
    /// Mocks registered before enabling Git remain sparse-only, so enable it
    /// before updates when the test needs their history. Repeated calls return
    /// the same URL without creating another repository or commit.
    pub async fn enable_git_index(&self) -> Result<Url> {
        let mut inner = self.inner.lock().await;
        let path = if let Some(path) = &inner.git_index {
            path.clone()
        } else {
            let path = self.cargo_home.path().join("git-index");
            let mut entries = BTreeMap::new();
            entries.insert("config.json".into(), inner.index_config.clone());
            let init_path = path.clone();
            spawn_blocking(move || Self::init_git_index(&init_path, entries)).await?;
            inner.git_index = Some(path.clone());
            path
        };
        Url::from_directory_path(&path)
            .map_err(|_| anyhow::anyhow!("Invalid registry Git path: {}", path.display()))
    }

    /// Current Git commit in the registry repository.
    pub async fn git_head(&self) -> Result<gix::ObjectId> {
        let inner = self.inner.lock().await;
        let path = inner
            .git_index
            .clone()
            .context("Git index is not enabled")?;
        spawn_blocking(move || Ok(gix::open(path)?.head_id()?.into())).await
    }

    /// Remove a crate from both the sparse index and the Git index, if enabled.
    pub async fn mock_index_missing(&self, krate: &KrateName) {
        self.mock_index_payload(krate, None).await;
    }

    async fn mock_index_payload(&self, krate: &KrateName, payload: Option<Vec<u8>>) {
        let payload = payload.filter(|body| !body.is_empty());

        let crate_url: Url = self
            .api
            .sparse_index
            .crate_url(krate.as_str())
            .unwrap()
            .parse()
            .unwrap();

        let mut inner = self.inner.lock().await;

        if let Some(path) = inner.git_index.clone() {
            let entry_path = PathBuf::from(
                crate_url
                    .path()
                    .strip_prefix(self.config.sparse_index_host.path())
                    .unwrap(),
            );
            let payload = payload.clone();
            spawn_blocking(move || {
                // A missing payload removes the entry before committing.
                Self::write_git_entries(&gix::open(path)?, [(entry_path, payload)]).map(|_| ())
            })
            .await
            .expect("updating mocked Git index");
        }

        // if that path already was mocked, remove it,
        // for the one we create here.
        if let Some(previous_mock) = inner.crate_mocks.remove(crate_url.path()) {
            previous_mock.remove_async().await;
        }

        let mut mock = inner
            .index_server
            .mock("GET", crate_url.path())
            .match_header(DOCSRS_AUTHORIZATION.as_str(), Matcher::Missing);

        if let Some(payload) = payload {
            mock = mock
                .with_status(StatusCode::OK.as_u16().into())
                .with_body(payload);
        } else {
            mock = mock.with_status(StatusCode::NOT_FOUND.as_u16().into());
        }

        let mock = mock.create_async().await;
        inner.crate_mocks.insert(crate_url.path().to_owned(), mock);
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
            .match_header(DOCSRS_AUTHORIZATION.as_str(), Matcher::Missing)
            .with_status(StatusCode::OK.as_u16().into())
            .with_body(archive)
            .create_async()
            .await;
        inner.other_mocks.push(mock);
    }

    /// Create a custom mock for a registry API `GET` request.
    ///
    /// `path` may include a query string, whose URL-encoded pairs are matched independently of
    /// their order. The closure can configure status, headers, and response body on the
    /// underlying Mockito mock.
    /// The X-Docsrs-Authorization header must match the configured API token, or be absent without one.
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
        let authentication = self
            .config
            .api_token
            .as_ref()
            .map_or(Matcher::Missing, |token| {
                Matcher::Exact(token.to_str().unwrap().to_owned())
            });
        let mut mock = inner
            .api_server
            .mock("GET", url.path())
            .match_header(DOCSRS_AUTHORIZATION.as_str(), authentication);
        if !query_matchers.is_empty() {
            mock = mock.match_query(Matcher::AllOf(query_matchers));
        }
        let mock = f(mock).create_async().await;
        inner.other_mocks.push(mock);
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

        for mock in inner.crate_mocks.values() {
            mock.assert_async().await;
        }
        for mock in &inner.other_mocks {
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

impl TestRegistry {
    fn init_git_index(path: &Path, entries: BTreeMap<PathBuf, String>) -> Result<()> {
        let repo = gix::init_bare(path)?;
        // crates-index fetches master in addition to HEAD.
        repo.edit_reference(gix::refs::transaction::RefEdit {
            name: "HEAD".try_into()?,
            change: gix::refs::transaction::Change::Update {
                log: Default::default(),
                expected: gix::refs::transaction::PreviousValue::Any,
                new: gix::refs::Target::Symbolic("refs/heads/master".try_into()?),
            },
            deref: false,
        })?;
        Self::write_git_entries(
            &repo,
            entries
                .into_iter()
                .map(|(path, body)| (path, Some(body.into_bytes()))),
        )?;
        Ok(())
    }

    fn write_git_entries<P: AsRef<Path>>(
        repo: &gix::Repository,
        entries: impl IntoIterator<Item = (P, Option<Vec<u8>>)>,
    ) -> Result<ObjectId> {
        let parent = repo.head_id().ok().map(|id| id.detach());
        let base = if parent.is_some() {
            repo.head_tree_id()?.detach()
        } else {
            ObjectId::empty_tree(repo.object_hash())
        };
        let mut tree = repo.edit_tree(base)?;
        for (path, body) in entries {
            let path = path.as_ref();
            if let Some(body) = body {
                tree.upsert(
                    path.to_string_lossy().as_ref(),
                    EntryKind::Blob,
                    repo.write_blob(body)?,
                )?;
            } else {
                tree.remove(path.to_string_lossy().as_ref())?;
            }
        }
        let tree = tree.write()?;
        let seconds = match parent {
            Some(parent) => repo.find_commit(parent)?.time()?.seconds + 1,
            None => 1_700_000_000,
        };
        let time = format!("{seconds} +0000");
        let actor = gix::actor::SignatureRef {
            name: "Test".into(),
            email: "test@example.com".into(),
            time: &time,
        };
        Ok(repo
            .commit_as(actor, actor, "HEAD", "Update registry index", tree, parent)?
            .detach())
    }
}

#[cfg(test)]
mod git_tests {
    use super::*;
    use crate::testing::registry_entry;
    use docs_rs_types::testing::{KRATE, V1, V2};

    fn blob(repo: &gix::Repository, commit: &ObjectId, path: &str) -> Result<Option<Vec<u8>>> {
        let tree = repo.find_commit(*commit)?.tree()?;
        tree.lookup_entry_by_path(path)?
            .map(|entry| Ok(entry.object()?.data.clone()))
            .transpose()
    }

    #[tokio::test]
    async fn git_index_url_points_to_registry_remote() -> Result<()> {
        let registry = TestRegistry::new().await?;
        assert!(registry.git_head().await.is_err());
        let url = registry.enable_git_index().await?;
        assert_eq!(url.scheme(), "file");
        assert_eq!(
            url.to_file_path().unwrap(),
            registry.cargo_home.path().join("git-index")
        );
        assert_eq!(registry.enable_git_index().await?, url);
        registry
            .mock_index_response(&KRATE, [registry_entry(&KRATE, &V1, None, false)])
            .await;
        assert_eq!(
            gix::open(url.to_file_path().unwrap())?.head_id()?.detach(),
            registry.git_head().await?
        );
        Ok(())
    }

    #[test_case::test_case(false, false; "enable before mocks, explicit removal")]
    #[test_case::test_case(true, false; "enable after mocks, explicit removal")]
    #[test_case::test_case(false, true; "enable before mocks, empty payload")]
    #[test_case::test_case(true, true; "enable after mocks, empty payload")]
    #[tokio::test]
    async fn git_mirrors_sparse_updates_and_removals(
        enable_late: bool,
        empty_payload: bool,
    ) -> Result<()> {
        let registry = TestRegistry::new().await?;
        if !enable_late {
            registry.enable_git_index().await?;
        }
        registry
            .mock_index_response(&KRATE, [registry_entry(&KRATE, &V1, None, true)])
            .await;
        let path = registry.enable_git_index().await?.to_file_path().unwrap();
        let before = registry.git_head().await?;
        let old_payload = if enable_late {
            None
        } else {
            Some(serde_json::to_vec(&registry_entry(
                &KRATE, &V1, None, true,
            ))?)
        };
        let repo = gix::open(&path)?;
        assert_eq!(blob(&repo, &before, "kr/at/krate")?, old_payload.clone());

        // The previous sparse response has not been requested yet. Replacement
        // must still serve the latest state, matching the new Git commit.
        let versions = [
            registry_entry(&KRATE, &V1, None, false),
            registry_entry(&KRATE, &V2, None, false),
        ];
        registry.mock_index_response(&KRATE, &versions).await;
        let after = registry.git_head().await?;
        assert_ne!(before, after);
        let expected = versions
            .iter()
            .map(|v| serde_json::to_string(v).unwrap())
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(
            blob(&repo, &after, "kr/at/krate")?,
            Some(expected.into_bytes())
        );
        let sparse = registry.api().get_crate_from_index(&KRATE).await?.unwrap();
        assert_eq!(sparse.versions().len(), 2);
        assert!(!sparse.versions()[0].is_yanked());
        assert_eq!(blob(&repo, &before, "kr/at/krate")?, old_payload);

        let config_before = blob(&repo, &before, "config.json")?;
        assert!(config_before.is_some());
        if empty_payload {
            registry
                .mock_index_response(&KRATE, Vec::<serde_json::Value>::new())
                .await;
        } else {
            registry.mock_index_missing(&KRATE).await;
        }
        let removed = registry.git_head().await?;
        assert_ne!(after, removed);
        assert_eq!(blob(&repo, &removed, "kr/at/krate")?, None);
        assert_eq!(blob(&repo, &removed, "config.json")?, config_before);
        assert!(registry.api().get_crate_from_index(&KRATE).await?.is_none());
        Ok(())
    }

    #[test_case::test_case(false; "Git already enabled")]
    #[test_case::test_case(true; "Git enabled after empty payload")]
    #[tokio::test]
    async fn empty_payload_has_no_git_entry(enable_late: bool) -> Result<()> {
        let registry = TestRegistry::new().await?;
        if !enable_late {
            registry.enable_git_index().await?;
        }
        registry
            .mock_index_response(&KRATE, Vec::<serde_json::Value>::new())
            .await;
        let path = registry.enable_git_index().await?.to_file_path().unwrap();
        let head = registry.git_head().await?;
        assert_eq!(blob(&gix::open(path)?, &head, "kr/at/krate")?, None);
        let url = registry.api.sparse_index.crate_url(KRATE.as_str()).unwrap();
        assert_eq!(reqwest::get(url).await?.status(), StatusCode::NOT_FOUND);
        Ok(())
    }
}
