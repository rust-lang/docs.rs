use crate::{error::Result, utils::retry};
use anyhow::{anyhow, Context};
use chrono::{DateTime, Utc};
use reqwest::header::{HeaderValue, ACCEPT, USER_AGENT};
use semver::Version;
use serde::Deserialize;
use url::Url;

const APP_USER_AGENT: &str = concat!(
    env!("CARGO_PKG_NAME"),
    " ",
    include_str!(concat!(env!("OUT_DIR"), "/git_version"))
);

#[derive(Debug)]
pub struct Api {
    api_base: Option<Url>,
    client: reqwest::blocking::Client,
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
}

impl Api {
    pub(super) fn new(api_base: Option<Url>) -> Result<Self> {
        let headers = vec![
            (USER_AGENT, HeaderValue::from_static(APP_USER_AGENT)),
            (ACCEPT, HeaderValue::from_static("application/json")),
        ]
        .into_iter()
        .collect();

        let client = reqwest::blocking::Client::builder()
            .default_headers(headers)
            .build()?;

        Ok(Self { api_base, client })
    }

    fn api_base(&self) -> Result<Url> {
        self.api_base
            .clone()
            .with_context(|| anyhow!("index is missing an api base url"))
    }

    pub fn get_crate_data(&self, name: &str) -> Result<CrateData> {
        let owners = self
            .get_owners(name)
            .context(format!("Failed to get owners for {name}"))?;

        Ok(CrateData { owners })
    }

    pub(crate) fn get_release_data(&self, name: &str, version: &str) -> Result<ReleaseData> {
        let (release_time, yanked, downloads) = self
            .get_release_time_yanked_downloads(name, version)
            .context(format!("Failed to get crate data for {name}-{version}"))?;

        Ok(ReleaseData {
            release_time,
            yanked,
            downloads,
        })
    }

    /// Get release_time, yanked and downloads from the registry's API
    fn get_release_time_yanked_downloads(
        &self,
        name: &str,
        version: &str,
    ) -> Result<(DateTime<Utc>, bool, i32)> {
        let url = {
            let mut url = self.api_base()?;
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

        let response: Response = retry(
            || Ok(self.client.get(url.clone()).send()?.error_for_status()?),
            3,
        )?
        .json()?;

        let version = Version::parse(version)?;
        let version = response
            .versions
            .into_iter()
            .find(|data| data.num == version)
            .with_context(|| anyhow!("Could not find version in response"))?;

        Ok((version.created_at, version.yanked, version.downloads))
    }

    /// Fetch owners from the registry's API
    fn get_owners(&self, name: &str) -> Result<Vec<CrateOwner>> {
        let url = {
            let mut url = self.api_base()?;
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
        }

        let response: Response = retry(
            || Ok(self.client.get(url.clone()).send()?.error_for_status()?),
            3,
        )?
        .json()?;

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
            })
            .collect();

        Ok(result)
    }
}
