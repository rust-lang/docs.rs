use crate::{
    Config,
    error::{Error, Result},
    models::{
        ApiErrors, CrateData, CrateOwner, OwnerKind, ReleaseData, Search, SearchCursor,
        SearchResponse,
    },
};
use anyhow::Context as _;
use docs_rs_types::{KrateName, Version};
use docs_rs_utils::APP_USER_AGENT;
use reqwest::header::ACCEPT;
use reqwest_middleware::{ClientBuilder, ClientWithMiddleware};
use reqwest_retry::{RetryTransientMiddleware, policies::ExponentialBackoff};
use serde::{Deserialize, de::DeserializeOwned};
use std::{fmt, io, path::Path};
use tracing::instrument;
use url::Url;

/// Send a `crates_index` request via reqwest.
///
/// This is needed since the `crates_index` API expects `http::Request` and `http::Response`
/// objects and we're using async `reqwest` to do the actual requests.
///
/// See
/// https://github.com/frewsxcv/rust-crates-index/blob/master/examples/sparse_http_reqwest.rs
#[instrument(skip_all)]
async fn send_sparse_request(
    client: &ClientWithMiddleware,
    request: http::Request<()>,
) -> Result<http::Response<Vec<u8>>> {
    let (mut parts, _) = request.into_parts();

    // NOTE: oddly, when testing, hyper / reqwest connected via HTTP/1 to
    // the sparse index.
    // The prepared requests from `crates_index` try to force HTTP/2, and then
    // fail.
    // For now we just use HTTP/1.
    parts.version = http::Version::HTTP_11;

    let request: reqwest::Request = http::Request::from_parts(parts, Vec::new()).try_into()?;

    let response = client.execute(request).await?;

    let mut builder = http::Response::builder()
        .status(response.status())
        .version(response.version());

    builder.headers_mut().unwrap().extend(
        response
            .headers()
            .iter()
            .map(|(key, value)| (key.clone(), value.clone())),
    );

    Ok(builder.body(response.bytes().await?.to_vec())?)
}

async fn fetch_index_config(
    index: &crates_index::SparseIndex,
    client: &ClientWithMiddleware,
) -> Result<crates_index::IndexConfig> {
    match index.index_config() {
        // Local `config.json` exists: use it without a request.
        Ok(config) => Ok(config),

        // It is absent: fetch the live config and save it locally.
        Err(crates_index::Error::Io(error)) if error.kind() == io::ErrorKind::NotFound => {
            let response =
                send_sparse_request(client, index.make_config_request()?.body(())?).await?;

            // `true` writes config.json into Cargo's sparse-index directory.
            Ok(index.parse_config_response(response, true)?)
        }

        // Do not hide corrupt JSON or permission errors by fetching anew.
        Err(error) => Err(error.into()),
    }
}

/// Client for registry data.
///
/// Release metadata is read from the sparse index, while endpoints not represented in the index,
/// such as owners and search, are requested from the API URL advertised by the index.
#[derive(Debug)]
pub struct RegistryApi {
    index_config: crates_index::IndexConfig,
    api_base: Url,
    pub(crate) sparse_index: crates_index::SparseIndex,
    client: ClientWithMiddleware,
}

impl RegistryApi {
    /// Create a client using the configured sparse-index URL and retry policy.
    ///
    /// The index's `config.json` determines the API and download URLs.
    pub async fn from_config(config: &Config) -> Result<Self> {
        Self::new(
            config.sparse_index_host.clone(),
            config.crates_io_api_call_retries,
            None,
        )
        .await
    }

    /// Create a client from a sparse-index URL.
    ///
    /// The index's `config.json` determines the API and download URLs. When `cargo_home` is
    /// provided, it is used as the sparse-index cache location; otherwise Cargo's normal cache
    /// location is used.
    pub(crate) async fn new(
        sparse_base: Url,
        max_retries: u32,
        cargo_home: Option<&Path>,
    ) -> Result<Self> {
        let client = ClientBuilder::new(
            reqwest::Client::builder()
                .user_agent(APP_USER_AGENT)
                .gzip(true)
                .build()?,
        )
        .with(RetryTransientMiddleware::new_with_policy(
            ExponentialBackoff::builder().build_with_max_retries(max_retries),
        ))
        .build();

        let sparse_index = if let Some(cargo_home) = cargo_home {
            crates_index::SparseIndex::with_path(cargo_home, sparse_base.as_str())?
        } else {
            // uses default cargo home on the system.
            crates_index::SparseIndex::from_url(sparse_base.as_str())?
        };
        let index_config = fetch_index_config(&sparse_index, &client).await?;

        Ok(Self {
            api_base: index_config
                .api
                .as_deref()
                // we only care about package indexes with api.
                .ok_or(Error::InvalidApiUrl)?
                .parse()
                .map_err(|_| Error::InvalidApiUrl)?,
            index_config,
            sparse_index,
            client,
        })
    }

    /// Return the download URL for a crate version according to the index configuration.
    pub fn download_url(&self, name: &KrateName, version: &Version) -> Result<Url> {
        self.index_config
            .download_url(name.as_str(), &version.to_string())
            .ok_or_else(|| {
                crates_index::Error::Url(format!(
                    "can't create download URL for {} {}",
                    name, version
                ))
            })?
            .parse::<Url>()
            .map_err(|err| {
                crates_index::Error::Url(format!(
                    "invalid download URL for {} {}: {:?}",
                    name, version, err
                ))
            })
            .map_err(Into::into)
    }

    /// Fetch all published versions of a crate from the sparse index.
    ///
    /// Returns `None` when the index has no entry for `name`.
    #[instrument(skip(self))]
    pub async fn get_crate_from_index(
        &self,
        name: &KrateName,
    ) -> Result<Option<crates_index::Crate>> {
        let response = send_sparse_request(
            &self.client,
            self.sparse_index
                .make_cache_request(name.as_str())?
                .body(())?,
        )
        .await?;

        Ok(self
            .sparse_index
            .parse_cache_response(name.as_str(), response, true)?)
    }

    /// Fetch a specific crate version from the sparse index.
    ///
    /// Returns `None` when either the crate or the requested version is absent.
    #[instrument(skip(self))]
    pub async fn get_version_from_index(
        &self,
        name: &KrateName,
        version: &Version,
    ) -> Result<Option<crates_index::Version>> {
        let Some(krate) = self.get_crate_from_index(name).await? else {
            return Ok(None);
        };

        let version = version.to_string();

        Ok(krate
            .versions()
            .iter()
            .find(|v| v.version() == version)
            .cloned())
    }

    /// Make a request to crates.io, parse the response as JSON.
    ///
    /// We retry on
    /// * server-error responses (5xx)
    /// * other connection errors from reqwest
    ///
    /// We don't retry on all other status codes, as they are likely to be successful, or
    /// client errors (4xx), or other unexpected responses that won't succeed on retry.
    /// For debugging we include the response body in errors, either plain text or parsed
    /// when the response has the crates.io error format.
    ///
    /// We treat 5xx errors just as text, not knowing where they were raised.
    /// For 4xx errors we try to parse the the JSON error description.
    async fn api_request<T>(&self, url: impl reqwest::IntoUrl) -> Result<T>
    where
        T: DeserializeOwned,
    {
        let response = self
            .client
            .get(url)
            .header(ACCEPT, mime::APPLICATION_JSON.as_ref())
            .send()
            .await?;
        let status = response.status();

        if status.is_success() {
            Ok(response.json().await?)
        } else if status.is_server_error() {
            // this just to let reqwest generate us its "standard" error
            let err = response.error_for_status_ref().unwrap_err();
            let text = response.text().await.unwrap_or_default();
            Err(Error::HttpError(err.into(), text))
        } else {
            let text = response.text().await.unwrap_or_default();

            if let Ok(api_errors) = serde_json::from_str::<ApiErrors>(&text) {
                Err(Error::CrateIoApiError(status, api_errors))
            } else {
                Err(Error::CrateIoError(status, text))
            }
        }
    }

    /// Fetch crate-level metadata that is not present in the sparse index.
    ///
    /// At present, this contains its owners from the registry API.
    #[instrument(skip(self))]
    pub async fn get_crate_data(&self, name: &KrateName) -> Result<CrateData> {
        Ok(CrateData {
            owners: self.get_owners(name).await?,
        })
    }

    /// Fetch release metadata from the sparse index.
    ///
    /// Returns `None` when either the crate or the requested version is absent.
    #[instrument(skip(self))]
    pub async fn get_release_data(
        &self,
        name: &KrateName,
        version: &Version,
    ) -> Result<Option<ReleaseData>> {
        let Some(version) = self.get_version_from_index(name, version).await? else {
            return Ok(None);
        };

        Ok(Some(ReleaseData {
            release_time: version
                .pubtime()
                .map(|pt| {
                    pt.parse()
                        .context("invalid datetime format in package index")
                })
                .transpose()?,
            yanked: version.is_yanked(),
        }))
    }

    /// Fetch owners from the registry API.
    async fn get_owners(&self, name: &KrateName) -> Result<Vec<CrateOwner>> {
        let url = {
            let mut url = self.api_base.clone();
            url.path_segments_mut()
                .map_err(|()| Error::InvalidApiUrl)?
                .extend(&["api", "v1", "crates", name.as_str(), "owners"]);
            url
        };

        #[derive(Deserialize)]
        struct Response {
            users: Vec<OwnerData>,
        }

        #[derive(Deserialize)]
        struct OwnerData {
            #[serde(default)]
            avatar: Option<String>,
            #[serde(default)]
            login: Option<String>,
            #[serde(default)]
            kind: Option<OwnerKind>,
        }

        let response: Response = self.api_request(url).await?;

        let result = response
            .users
            .into_iter()
            .filter(|data| data.login.as_ref().is_some_and(|login| !login.is_empty()))
            .map(|data| CrateOwner {
                avatar: data.avatar.unwrap_or_default(),
                login: data.login.unwrap_or_default(),
                kind: data.kind.unwrap_or(OwnerKind::User),
            })
            .collect();

        Ok(result)
    }

    /// Run a search with a crates.io generated cursor for fetching next/previous pages.
    #[instrument(skip(self))]
    pub async fn search<C>(&self, cursor: C) -> Result<Search>
    where
        C: Into<SearchCursor> + fmt::Debug,
    {
        let cursor = cursor.into();

        let query_params = cursor.query_for_url();

        let url = {
            let mut url = self.api_base.clone();
            url.path_segments_mut()
                .map_err(|()| Error::InvalidApiUrl)?
                .extend(&["api", "v1", "crates"]);
            url.set_query(Some(query_params));
            url
        };

        let response: SearchResponse = self.api_request(url).await?;

        Ok(Search {
            crates: response.crates.ok_or(Error::MissingReleases)?,
            meta: response.meta.ok_or(Error::MissingMetadata)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        models::{ApiError, SearchCrate, SearchMeta, SearchQuery, SearchSort},
        testing::TestRegistry,
    };
    use chrono::{DateTime, Utc};
    use crates_index::IndexConfig;
    use docs_rs_types::testing::{KRATE, V1, V2};
    use reqwest::{StatusCode, header::CONTENT_TYPE};
    use serde::Serialize;
    use test_case::test_case;

    const CHECKSUM: &str = "0000000000000000000000000000000000000000000000000000000000000000";

    fn sparse_entry(version: &Version, pubtime: Option<&str>, yanked: bool) -> serde_json::Value {
        serde_json::json!({
            "name": KRATE.as_str(),
            "vers": version.to_string(),
            "pubtime": pubtime,
            "deps": [],
            "cksum": CHECKSUM,
            "features": {},
            "yanked": yanked,
        })
    }

    async fn test_search(body: impl Serialize) -> Result<Search> {
        let env = TestRegistry::new().await?;

        env.create_api_mock("/api/v1/crates?q=foo", move |mock| {
            mock.with_status(StatusCode::OK.as_u16().into())
                .with_header(CONTENT_TYPE, mime::APPLICATION_JSON.as_ref())
                .with_body(serde_json::to_vec(&body).unwrap())
        })
        .await;

        env.api().search(&SearchQuery::from("foo")).await
    }

    async fn test_get_release<L>(
        payload: impl IntoIterator<Item = L>,
        version: &Version,
    ) -> Result<Option<ReleaseData>>
    where
        L: Serialize,
    {
        let env = TestRegistry::new().await?;
        env.mock_index_response(&KRATE, payload).await;

        env.api().get_release_data(&KRATE, version).await
    }

    #[tokio::test]
    async fn test_search_ok() -> Result<()> {
        let env = TestRegistry::new().await?;
        let next_page = SearchCursor::builder().custom_arg("next", "").build();
        let prev_page = SearchCursor::builder().custom_arg("prev", "").build();

        let query = SearchQuery::from("foo");

        env.mock_search(query.clone())
            .crate_names(["foo", "bar"])
            .next_page(next_page.clone())
            .prev_page(prev_page.clone())
            .create()
            .await;

        let result = env.api().search(&query).await?;

        assert_eq!(
            result.crates,
            vec![
                SearchCrate { name: "foo".into() },
                SearchCrate { name: "bar".into() },
            ]
        );
        assert_eq!(
            result.meta,
            SearchMeta {
                next_page: Some(next_page),
                prev_page: Some(prev_page),
            }
        );

        Ok(())
    }

    #[tokio::test]
    async fn test_search_passes_query_params_and_returns_pagination() -> Result<()> {
        let env = TestRegistry::new().await?;
        let this_cursor = SearchCursor::builder()
            .query("some_random_crate")
            .page(2)
            .per_page(30)
            .sort_by(SearchSort::RecentUpdates)
            .build();
        let crates = vec![SearchCrate {
            name: "some_random_crate".into(),
        }];
        let meta = SearchMeta {
            next_page: Some(this_cursor.clone().adapt().page(3).build()),
            prev_page: Some(this_cursor.clone().adapt().page(1).build()),
        };

        env.mock_search(&this_cursor)
            .crate_names(["some_random_crate"])
            .maybe_next_page(meta.next_page().cloned())
            .maybe_prev_page(meta.prev_page().cloned())
            .create()
            .await;

        let result = env.api().search(&this_cursor).await?;

        assert_eq!(result.crates, crates);
        assert_eq!(result.meta, meta);

        Ok(())
    }

    #[tokio::test]
    async fn test_search_encodes_initial_search_params() -> Result<()> {
        let env = TestRegistry::new().await?;
        let query = SearchQuery::builder("some random crate")
            .sort_by(SearchSort::RecentUpdates)
            .per_page(30)
            .build();

        env.mock_search(query.clone())
            .crate_names(["some_random_crate"])
            .create()
            .await;

        let result = env.api().search(&query).await?;
        assert_eq!(result.crates[0].name, "some_random_crate");

        Ok(())
    }

    #[tokio::test]
    async fn test_search_follows_pagination() -> Result<()> {
        let env = TestRegistry::new().await?;
        let query = SearchQuery::builder("some random crate")
            .sort_by(SearchSort::RecentUpdates)
            .per_page(30)
            .build();

        let next_page: SearchCursor = query.clone().into();
        let next_page = next_page.adapt().page(2).build();

        env.mock_search(query.clone())
            .crate_names(["first_page"])
            .next_page(next_page.clone())
            .create()
            .await;

        let first_page = env.api().search(&query).await?;
        let cursor = first_page.meta.next_page().expect("next page cursor");

        env.mock_search(next_page)
            .crate_names(["second_page"])
            .create()
            .await;

        let second_page = env.api().search(cursor).await?;
        assert_eq!(second_page.crates[0].name, "second_page");

        Ok(())
    }

    #[tokio::test]
    async fn test_search_passes_opaque_pagination_params() -> Result<()> {
        let env = TestRegistry::new().await?;
        let crates = vec![SearchCrate {
            name: "some_random_crate".into(),
        }];
        let meta = SearchMeta {
            next_page: None,
            prev_page: None,
        };

        let cursor = SearchCursor::builder()
            .custom_arg("some", "dummy")
            .custom_arg("pagination", "parameters")
            .build();

        env.mock_search(&cursor)
            .crate_names(["some_random_crate"])
            .create()
            .await;

        let result = env.api().search(&cursor).await?;

        assert_eq!(result.crates, crates);
        assert_eq!(result.meta, meta);

        Ok(())
    }

    #[tokio::test]
    async fn test_search_crates_missing() -> Result<()> {
        let meta = SearchMeta {
            next_page: Some("?next".parse().unwrap()),
            prev_page: Some("?prev".parse().unwrap()),
        };

        assert!(matches!(
            test_search(SearchResponse {
                crates: None,
                meta: Some(meta.clone()),
            })
            .await
            .unwrap_err(),
            Error::MissingReleases
        ));

        Ok(())
    }

    #[tokio::test]
    async fn test_search_meta_missing() -> Result<()> {
        let crates = vec![
            SearchCrate { name: "foo".into() },
            SearchCrate { name: "bar".into() },
        ];

        assert!(matches!(
            test_search(SearchResponse {
                crates: Some(crates.clone()),
                meta: None,
            })
            .await
            .unwrap_err(),
            Error::MissingMetadata
        ));

        Ok(())
    }

    #[tokio::test]
    #[test_case(StatusCode::BAD_REQUEST)]
    #[test_case(StatusCode::UNAUTHORIZED)]
    async fn test_search_new_style_api_errors(status: StatusCode) -> Result<()> {
        let env = TestRegistry::new().await?;
        let query = SearchQuery::from("foo");
        let response = ApiErrors {
            errors: vec![
                ApiError {
                    detail: Some("error 1".into()),
                },
                ApiError {
                    detail: Some("error 2".into()),
                },
            ],
        };

        env.mock_search_error(query.clone())
            .client_error(status)
            .api_errors(response.clone())
            .create()
            .await;

        assert!(matches!(
            env.api().search(&query).await.unwrap_err(),
            Error::CrateIoApiError(_status, errors) if errors == response
        ));

        Ok(())
    }

    #[tokio::test]
    async fn test_search_not_found_is_a_plain_api_error() -> Result<()> {
        let env = TestRegistry::new().await?;
        let query = SearchQuery::from("foo");

        env.mock_search_error(query.clone())
            .client_error(StatusCode::NOT_FOUND)
            .create()
            .await;

        assert!(matches!(
            env.api().search(&query).await.unwrap_err(),
            Error::CrateIoError(status, _) if status == StatusCode::NOT_FOUND
        ));

        Ok(())
    }

    #[tokio::test]
    #[test_case(StatusCode::INTERNAL_SERVER_ERROR)]
    #[test_case(StatusCode::BAD_GATEWAY)]
    async fn test_search_server_errors(status: StatusCode) -> Result<()> {
        let env = TestRegistry::new().await?;
        let query = SearchQuery::from("foo");
        let msg = "some error message";

        env.mock_search_error(query.clone())
            .server_error(status)
            .error_text(msg)
            .create()
            .await;

        let err = env.api().search(&query).await.unwrap_err();
        assert!(err.to_string().contains(msg));
        assert_eq!(err.status(), Some(status));

        let Error::HttpError(req_err, body) = err else {
            panic!("Expected HttpError");
        };

        assert_eq!(req_err.status(), Some(status));
        assert!(body.contains(msg));

        Ok(())
    }

    #[tokio::test]
    async fn test_search_retries_server_errors() -> Result<()> {
        const RETRIES: u32 = 2;

        let env = TestRegistry::builder().retries(RETRIES).build().await?;
        env.create_api_mock("/api/v1/crates?q=foo", |mock| {
            mock.with_status(StatusCode::INTERNAL_SERVER_ERROR.as_u16().into())
                .expect((RETRIES + 1) as usize)
        })
        .await;

        assert!(matches!(
            env.api().search(&SearchQuery::from("foo")).await.unwrap_err(),
            Error::HttpError(error, _) if error.status() == Some(StatusCode::INTERNAL_SERVER_ERROR)
        ));
        env.assert_mocks().await;

        Ok(())
    }

    #[tokio::test]
    async fn test_get_release_ok() -> Result<()> {
        let release_data = test_get_release(
            [
                sparse_entry(&V1, Some("2024-01-01T00:00:00Z"), false),
                sparse_entry(&V2, Some("2024-01-02T00:00:00Z"), true),
            ],
            &V1,
        )
        .await?
        .expect("found version");

        assert_eq!(
            release_data,
            ReleaseData {
                release_time: Some(
                    DateTime::parse_from_rfc3339("2024-01-01T00:00:00Z")
                        .unwrap()
                        .with_timezone(&Utc)
                ),
                yanked: false,
            }
        );

        Ok(())
    }

    #[tokio::test]
    async fn test_get_release_without_pubtime() -> Result<()> {
        let release_data = test_get_release(
            [
                sparse_entry(&V1, None, false),
                sparse_entry(&V2, Some("2024-01-02T00:00:00Z"), true),
            ],
            &V1,
        )
        .await?
        .expect("found version");

        assert_eq!(
            release_data,
            ReleaseData {
                release_time: None,
                yanked: false,
            }
        );

        Ok(())
    }

    #[tokio::test]
    async fn test_get_release_not_found_other_version() -> Result<()> {
        assert!(
            test_get_release(
                [sparse_entry(&V1, Some("2024-01-01T00:00:00Z"), false)],
                &V2,
            )
            .await?
            .is_none()
        );

        Ok(())
    }

    #[tokio::test]
    async fn test_get_release_not_found_defaults_to_404() -> Result<()> {
        let env = TestRegistry::new().await?;

        assert!(env.api().get_release_data(&KRATE, &V1).await?.is_none());

        Ok(())
    }

    #[tokio::test]
    async fn test_get_crate_and_version() -> Result<()> {
        let env = TestRegistry::new().await?;
        env.mock_index_response(
            &KRATE,
            [sparse_entry(&V1, Some("2024-01-01T00:00:00Z"), false)],
        )
        .await;

        assert_eq!(
            env.api()
                .get_crate_from_index(&KRATE)
                .await?
                .unwrap()
                .versions()
                .len(),
            1
        );
        assert_eq!(
            env.api()
                .get_version_from_index(&KRATE, &V1)
                .await?
                .unwrap()
                .version(),
            V1.to_string()
        );
        assert!(
            env.api()
                .get_version_from_index(&KRATE, &V2)
                .await?
                .is_none()
        );
        Ok(())
    }

    #[tokio::test]
    async fn test_get_crate_data_normalizes_owners() -> Result<()> {
        let env = TestRegistry::new().await?;
        env.create_api_mock("/api/v1/crates/krate/owners", move |mock| {
            mock.with_status(StatusCode::OK.as_u16().into())
                .with_header(CONTENT_TYPE, mime::APPLICATION_JSON.as_ref())
                .with_body(
                    serde_json::to_vec(&serde_json::json!({
                        "users": [
                            {"login": "team", "avatar": "avatar", "kind": "team"},
                            {"login": "user"},
                            {"login": ""},
                            {}
                        ]
                    }))
                    .unwrap(),
                )
        })
        .await;

        let data = env.api().get_crate_data(&KRATE).await?;
        assert_eq!(data.owners.len(), 2);
        assert_eq!(data.owners[0].login, "team");
        assert_eq!(data.owners[0].kind, OwnerKind::Team);
        assert_eq!(data.owners[1].avatar, "");
        assert_eq!(data.owners[1].kind, OwnerKind::User);
        Ok(())
    }

    #[tokio::test]
    async fn test_get_release_yanked_and_invalid_pubtime() -> Result<()> {
        let data = test_get_release([sparse_entry(&V1, Some("invalid"), true)], &V1).await;
        assert!(matches!(data.unwrap_err(), Error::Other(_)));

        let data = test_get_release([sparse_entry(&V1, Some("2024-01-01T00:00:00Z"), true)], &V1)
            .await?
            .unwrap();
        assert!(data.yanked);
        Ok(())
    }

    #[tokio::test]
    async fn test_invalid_index_config_is_an_error() -> Result<()> {
        for config in [
            IndexConfig {
                dl: "http://127.0.0.1:1".into(),
                api: None,
            },
            IndexConfig {
                dl: "http://127.0.0.1:1".into(),
                api: Some("not a url".into()),
            },
        ] {
            let err = TestRegistry::builder()
                .index_config(config)
                .build()
                .await
                .err()
                .expect("invalid config");
            assert!(matches!(
                err.downcast_ref::<Error>(),
                Some(Error::InvalidApiUrl)
            ));
        }
        Ok(())
    }

    #[tokio::test]
    async fn test_invalid_sparse_url_and_download_url_are_errors() -> Result<()> {
        let cargo_home = tempfile::tempdir().unwrap();
        let err = RegistryApi::new(
            "https://index.example".parse().unwrap(),
            0,
            Some(cargo_home.path()),
        )
        .await
        .expect_err("invalid sparse URL");
        assert!(matches!(err, Error::SparseIndexError(_)));

        let env = TestRegistry::builder()
            .index_config(IndexConfig {
                dl: "not a URL".into(),
                api: Some("http://127.0.0.1:1".into()),
            })
            .build()
            .await?;
        assert!(env.api().download_url(&KRATE, &V1).is_err());
        Ok(())
    }

    #[tokio::test]
    async fn test_download_url() -> Result<()> {
        let env = TestRegistry::new().await?;

        assert!(
            env.api()
                .download_url(&KRATE, &V1)?
                .to_string()
                .ends_with("/crates/krate/1.0.0/download")
        );

        Ok(())
    }
}
