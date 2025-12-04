use crate::{APP_USER_AGENT, db::types::version::Version, error::Result, utils::retry_async};
use anyhow::{Context, anyhow, bail};
use chrono::{DateTime, Utc};
use reqwest::header::{ACCEPT, HeaderValue, USER_AGENT};
use serde::{Deserialize, Serialize};
use std::fmt;
use tracing::instrument;
use url::Url;

#[derive(Debug)]
pub struct RegistryApi {
    api_base: Url,
    max_retries: u32,
    client: reqwest::Client,
}

#[derive(Debug)]
pub struct CrateData {
    pub(crate) owners: Vec<CrateOwner>,
}

#[derive(Debug)]
pub(crate) struct ReleaseData {
    pub(crate) release_time: DateTime<Utc>,
    pub(crate) yanked: bool,
    pub(crate) downloads: i32,
}

impl Default for ReleaseData {
    fn default() -> ReleaseData {
        ReleaseData {
            release_time: Utc::now(),
            yanked: false,
            downloads: 0,
        }
    }
}

#[derive(Debug, Clone)]
pub struct CrateOwner {
    pub(crate) avatar: String,
    pub(crate) login: String,
    pub(crate) kind: OwnerKind,
}

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Serialize,
    Deserialize,
    sqlx::Type,
    bincode::Encode,
)]
#[sqlx(type_name = "owner_kind", rename_all = "lowercase")]
#[serde(rename_all = "lowercase")]
pub enum OwnerKind {
    User,
    Team,
}

impl fmt::Display for OwnerKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::User => f.write_str("user"),
            Self::Team => f.write_str("team"),
        }
    }
}

#[derive(Deserialize, Debug)]

pub(crate) struct SearchCrate {
    pub(crate) name: String,
}

#[derive(Deserialize, Debug)]

pub(crate) struct SearchMeta {
    pub(crate) next_page: Option<String>,
    pub(crate) prev_page: Option<String>,
}

#[derive(Deserialize, Debug)]
pub(crate) struct Search {
    pub(crate) crates: Vec<SearchCrate>,
    pub(crate) meta: SearchMeta,
}

impl RegistryApi {
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

    #[instrument(skip(self))]
    pub async fn get_crate_data(&self, name: &str) -> Result<CrateData> {
        let owners = self
            .get_owners(name)
            .await
            .context(format!("Failed to get owners for {name}"))?;

        Ok(CrateData { owners })
    }

    #[instrument(skip(self))]
    pub(crate) async fn get_release_data(
        &self,
        name: &str,
        version: &Version,
    ) -> Result<ReleaseData> {
        let (release_time, yanked, downloads) = self
            .get_release_time_yanked_downloads(name, version)
            .await
            .context(format!("Failed to get crate data for {name}-{version}"))?;

        Ok(ReleaseData {
            release_time,
            yanked,
            downloads,
        })
    }

    /// Get release_time, yanked and downloads from the registry's API
    async fn get_release_time_yanked_downloads(
        &self,
        name: &str,
        version: &Version,
    ) -> Result<(DateTime<Utc>, bool, i32)> {
        let url = {
            let mut url = self.api_base.clone();
            url.path_segments_mut()
                .map_err(|()| anyhow!("Invalid API url"))?
                .extend(&["api", "v1", "crates", name, "versions"]);
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

        let response: Response = retry_async(
            || async {
                Ok(self
                    .client
                    .get(url.clone())
                    .send()
                    .await?
                    .error_for_status()?)
            },
            self.max_retries,
        )
        .await?
        .json()
        .await?;

        let version = response
            .versions
            .into_iter()
            .find(|data| data.num == *version)
            .with_context(|| anyhow!("Could not find version in response"))?;

        Ok((version.created_at, version.yanked, version.downloads))
    }

    /// Fetch owners from the registry's API
    async fn get_owners(&self, name: &str) -> Result<Vec<CrateOwner>> {
        let url = {
            let mut url = self.api_base.clone();
            url.path_segments_mut()
                .map_err(|()| anyhow!("Invalid API url"))?
                .extend(&["api", "v1", "crates", name, "owners"]);
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

        let response: Response = retry_async(
            || async {
                Ok(self
                    .client
                    .get(url.clone())
                    .send()
                    .await?
                    .error_for_status()?)
            },
            self.max_retries,
        )
        .await?
        .json()
        .await?;

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

    /// Fetch crates from the registry's API
    pub(crate) async fn search(&self, query_params: &str) -> Result<Search> {
        #[derive(Deserialize, Debug)]
        struct SearchError {
            detail: String,
        }

        #[derive(Deserialize, Debug)]
        struct SearchResponse {
            crates: Option<Vec<SearchCrate>>,
            meta: Option<SearchMeta>,
            errors: Option<Vec<SearchError>>,
        }

        let url = {
            let mut url = self.api_base.clone();
            url.path_segments_mut()
                .map_err(|()| anyhow!("Invalid API url"))?
                .extend(&["api", "v1", "crates"]);
            url.set_query(Some(query_params));
            url
        };

        let response: SearchResponse = retry_async(
            || async {
                Ok(self
                    .client
                    .get(url.clone())
                    .send()
                    .await?
                    .error_for_status()?)
            },
            self.max_retries,
        )
        .await?
        .json()
        .await?;

        if let Some(errors) = response.errors {
            let messages: Vec<_> = errors.into_iter().map(|e| e.detail).collect();
            bail!("got error from crates.io: {}", messages.join("\n"));
        }

        let Some(crates) = response.crates else {
            bail!("missing releases in crates.io response");
        };

        let Some(meta) = response.meta else {
            bail!("missing metadata in crates.io response");
        };

        Ok(Search { crates, meta })
    }
}
