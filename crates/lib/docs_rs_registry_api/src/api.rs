use crate::{
    Config,
    error::{Error, Result},
    models::{ApiErrors, CrateData, CrateOwner, OwnerKind, ReleaseData, Search, SearchResponse},
};
use anyhow::{Context, anyhow};
use chrono::{DateTime, Utc};
use docs_rs_types::{KrateName, Version};
use docs_rs_utils::{APP_USER_AGENT, retry_async};
use reqwest::header::{ACCEPT, HeaderValue, USER_AGENT};
use serde::{Deserialize, de::DeserializeOwned};
use tracing::instrument;
use url::Url;

#[derive(Debug)]
pub struct RegistryApi {
    api_base: Url,
    max_retries: u32,
    client: reqwest::Client,
}

impl RegistryApi {
    pub fn from_config(config: &Config) -> Result<Self> {
        Self::new(
            config.registry_api_host.clone(),
            config.crates_io_api_call_retries,
        )
    }

    pub fn new(api_base: Url, max_retries: u32) -> Result<Self> {
        let headers = vec![
            (USER_AGENT, HeaderValue::from_static(APP_USER_AGENT)),
            (ACCEPT, HeaderValue::from_static("application/json")),
        ]
        .into_iter()
        .collect();

        let client = reqwest::Client::builder()
            .default_headers(headers)
            .build()?;

        Ok(Self {
            api_base,
            client,
            max_retries,
        })
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
    async fn request<T>(&self, url: &Url) -> Result<T>
    where
        T: DeserializeOwned,
    {
        let response = retry_async(
            || async {
                // Make the request.
                // This would error on connection errors etc.
                let response = self.client.get(url.clone()).send().await?;

                if response.status().is_server_error() {
                    // this just to let reqwest generate us its "standard" error
                    let err = response.error_for_status_ref().unwrap_err();
                    let text = response.text().await.unwrap_or_default();
                    // we only want to retry on 5xx errors.
                    // for client errors we assume that trying again is not worth it.
                    Err(Error::HttpError(err, text))
                } else {
                    Ok::<_, Error>(response)
                }
            },
            self.max_retries,
        )
        .await?;

        let status = response.status();

        if status.is_success() {
            Ok(response.json().await?)
        } else {
            let text = response.text().await.unwrap_or_default();

            if let Ok(api_errors) = serde_json::from_str::<ApiErrors>(&text) {
                Err(Error::CrateIoApiError(status, api_errors))
            } else {
                Err(Error::CrateIoError(status, text))
            }
        }
    }

    #[instrument(skip(self))]
    pub async fn get_crate_data(&self, name: &KrateName) -> Result<CrateData> {
        Ok(CrateData {
            owners: self.get_owners(name).await?,
        })
    }

    #[instrument(skip(self))]
    pub async fn get_release_data(
        &self,
        name: &KrateName,
        version: &Version,
    ) -> Result<ReleaseData> {
        let url = {
            let mut url = self.api_base.clone();
            url.path_segments_mut()
                .map_err(|_| Error::InvalidApiUrl)?
                .extend(&["api", "v1", "crates", name.as_str(), "versions"]);
            url
        };

        #[derive(Deserialize)]
        struct Response {
            versions: Vec<VersionData>,
        }

        #[derive(Deserialize)]
        struct VersionData {
            num: Version,
            #[serde(default = "Utc::now")]
            created_at: DateTime<Utc>,
            #[serde(default)]
            yanked: bool,
            #[serde(default)]
            downloads: i32,
        }

        let response: Response = self.request(&url).await?;

        let version = response
            .versions
            .into_iter()
            .find(|data| data.num == *version)
            .with_context(|| anyhow!("Could not find version in response"))?;

        Ok(ReleaseData {
            release_time: version.created_at,
            yanked: version.yanked,
            downloads: version.downloads,
        })
    }

    /// Fetch owners from the registry's API
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

        let response: Response = self.request(&url).await?;

        let result = response
            .users
            .into_iter()
            .filter(|data| {
                !data
                    .login
                    .as_ref()
                    .map(|login| login.is_empty())
                    .unwrap_or_default()
            })
            .map(|data| CrateOwner {
                avatar: data.avatar.unwrap_or_default(),
                login: data.login.unwrap_or_default(),
                kind: data.kind.unwrap_or(OwnerKind::User),
            })
            .collect();

        Ok(result)
    }

    /// Fetch crates from the registry's API.
    pub async fn search(&self, query_params: &str) -> Result<Search> {
        let url = {
            let mut url = self.api_base.clone();
            url.path_segments_mut()
                .map_err(|()| Error::InvalidApiUrl)?
                .extend(&["api", "v1", "crates"]);
            url.set_query(Some(query_params));
            url
        };

        let response: SearchResponse = self.request(&url).await?;

        Ok(Search {
            crates: response.crates.ok_or(Error::MissingReleases)?,
            meta: response.meta.ok_or(Error::MissingMetadata)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{ApiError, SearchCrate, SearchMeta};
    use reqwest::{StatusCode, header::CONTENT_TYPE};
    use serde::Serialize;
    use test_case::test_case;

    async fn test_search(status: StatusCode, body: impl Serialize) -> Result<Search> {
        let mut crates_io_api = mockito::Server::new_async().await;

        let _m = crates_io_api
            .mock("GET", "/api/v1/crates?q=foo")
            .with_status(status.as_u16().into())
            .with_header(CONTENT_TYPE, mime::APPLICATION_JSON.as_ref())
            .with_body(serde_json::to_vec(&body).unwrap())
            .create_async()
            .await;

        let api = RegistryApi::new(crates_io_api.url().parse().unwrap(), 0)?;
        api.search("q=foo").await
    }

    #[test]
    fn test_error_without_status() {
        for err in [
            Error::InvalidApiUrl,
            Error::MissingReleases,
            Error::MissingMetadata,
            Error::Other(anyhow!("some error")),
        ] {
            assert!(err.status().is_none());
        }
    }

    #[test]
    fn test_error_with_included_status() {
        let status = StatusCode::INTERNAL_SERVER_ERROR;

        assert!(Error::CrateIoApiError(status, ApiErrors::default()).status() == Some(status));

        assert!(Error::CrateIoError(status, "".into()).status() == Some(status));
    }

    #[tokio::test]
    async fn test_error_reqwest_error_status() -> Result<()> {
        let status = StatusCode::INTERNAL_SERVER_ERROR;

        let mut srv = mockito::Server::new_async().await;
        let _m = srv
            .mock("GET", "/")
            .with_status(status.as_u16().into())
            .create_async()
            .await;

        let err = reqwest::get(&srv.url())
            .await?
            .error_for_status()
            .unwrap_err();

        assert_eq!(err.status(), Some(status));

        Ok(())
    }

    #[tokio::test]
    async fn test_search_ok() -> Result<()> {
        let crates = vec![
            SearchCrate { name: "foo".into() },
            SearchCrate { name: "bar".into() },
        ];
        let meta = SearchMeta {
            next_page: Some("next".into()),
            prev_page: Some("prev".into()),
        };

        let result = test_search(
            StatusCode::OK,
            SearchResponse {
                crates: Some(crates.clone()),
                meta: Some(meta.clone()),
            },
        )
        .await?;

        assert_eq!(result.crates, crates);
        assert_eq!(result.meta, meta);

        Ok(())
    }

    #[tokio::test]
    async fn test_search_crates_missing() -> Result<()> {
        let meta = SearchMeta {
            next_page: Some("next".into()),
            prev_page: Some("prev".into()),
        };

        assert!(matches!(
            test_search(
                StatusCode::OK,
                SearchResponse {
                    crates: None,
                    meta: Some(meta.clone()),
                }
            )
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
            test_search(
                StatusCode::OK,
                SearchResponse {
                    crates: Some(crates.clone()),
                    meta: None,
                }
            )
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

        assert!(matches!(
            test_search(status, response.clone()).await.unwrap_err(),
            Error::CrateIoApiError(status, errors) if errors == response
        ));

        Ok(())
    }

    #[tokio::test]
    #[test_case(StatusCode::INTERNAL_SERVER_ERROR)]
    #[test_case(StatusCode::BAD_GATEWAY)]
    async fn test_search_server_errors(status: StatusCode) -> Result<()> {
        let msg = "some error message";

        let err = test_search(status, msg).await.unwrap_err();
        assert!(err.to_string().contains(msg));
        assert_eq!(err.status(), Some(status));

        let Error::HttpError(req_err, body) = err else {
            panic!("Expected HttpError");
        };

        assert_eq!(req_err.status(), Some(status));
        assert!(body.contains(msg));

        Ok(())
    }
}
