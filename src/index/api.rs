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

pub struct Api {
    api_base: Option<Url>,
    max_retries: u32,
    client: reqwest::blocking::Client,
}

#[derive(Debug)]
pub struct CrateData {
    pub(crate) owners: Vec<GithubUser>,
}

#[derive(Debug, PartialEq)]
pub(crate) struct ReleaseData {
    pub(crate) release_time: DateTime<Utc>,
    pub(crate) yanked: bool,
    pub(crate) downloads: i32,
    pub(crate) published_by: Option<GithubUser>,
}

impl Default for ReleaseData {
    fn default() -> ReleaseData {
        ReleaseData {
            release_time: Utc::now(),
            yanked: false,
            downloads: 0,
            published_by: None,
        }
    }
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
    #[serde(default)]
    published_by: Option<GithubUser>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Default)]
pub struct GithubUser {
    #[serde(default)]
    pub(crate) id: u64,
    #[serde(default)]
    pub(crate) avatar: String,
    #[serde(default)]
    pub(crate) login: String,
}

impl Api {
    pub(super) fn new(api_base: Option<Url>, max_retries: u32) -> Result<Self> {
        let headers = vec![
            (USER_AGENT, HeaderValue::from_static(APP_USER_AGENT)),
            (ACCEPT, HeaderValue::from_static("application/json")),
        ]
        .into_iter()
        .collect();

        let client = reqwest::blocking::Client::builder()
            .default_headers(headers)
            .build()?;

        Ok(Self {
            api_base,
            client,
            max_retries,
        })
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
        let version = Version::parse(version)?;
        let data = self
            .get_versions(name)
            .context(format!("Failed to get crate data for {name}-{version}"))?
            .into_iter()
            .find(|data| data.num == version)
            .with_context(|| anyhow!("Could not find version in response"))?;

        Ok(ReleaseData {
            release_time: data.created_at,
            yanked: data.yanked,
            downloads: data.downloads,
            published_by: data.published_by,
        })
    }

    /// Get release_time, yanked and downloads from the registry's API
    fn get_versions(&self, name: &str) -> Result<Vec<VersionData>> {
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

        let response: Response = retry(
            || Ok(self.client.get(url.clone()).send()?.error_for_status()?),
            self.max_retries,
        )?
        .json()?;

        Ok(response.versions)
    }

    /// Fetch owners from the registry's API
    fn get_owners(&self, name: &str) -> Result<Vec<GithubUser>> {
        let url = {
            let mut url = self.api_base()?;
            url.path_segments_mut()
                .map_err(|()| anyhow!("Invalid API url"))?
                .extend(&["api", "v1", "crates", name, "owners"]);
            url
        };

        #[derive(Deserialize)]
        struct Response {
            users: Vec<GithubUser>,
        }

        let response: Response = retry(
            || Ok(self.client.get(url.clone()).send()?.error_for_status()?),
            self.max_retries,
        )?
        .json()?;

        Ok(response
            .users
            .into_iter()
            .filter(|data| !data.login.is_empty())
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mockito::mock;
    use serde_json::{self, json};

    fn api() -> Api {
        Api::new(Some(Url::parse(&mockito::server_url()).unwrap()), 1).unwrap()
    }

    #[test]
    fn get_owners() {
        let _m = mock("GET", "/api/v1/crates/krate/owners")
            .with_header("content-type", "application/json")
            .with_body(
                serde_json::to_string(&json!({
                "users": [
                    {
                        "id": 1,
                        "login": "the_first_owner",
                        "name": "name",
                        "avatar": "http://something",
                        "kind": "user",
                        "url": "https://github.com/the_second_owner"
                    },
                    {
                        "id": 2,
                        "login": "the_second_owner",
                        "name": "another name",
                        "avatar": "http://anotherthing",
                        "kind": "user",
                        "url": "https://github.com/the_second_owner"
                    }
                ]}))
                .unwrap(),
            )
            .create();

        assert_eq!(
            api().get_owners("krate").unwrap(),
            vec![
                GithubUser {
                    id: 1,
                    avatar: "http://something".into(),
                    login: "the_first_owner".into()
                },
                GithubUser {
                    id: 2,
                    avatar: "http://anotherthing".into(),
                    login: "the_second_owner".into()
                }
            ]
        );
    }

    #[test]
    fn get_release_info() {
        let created = Utc::now();
        let _m = mock("GET", "/api/v1/crates/krate/versions")
            .with_header("content-type", "application/json")
            .with_body(
                serde_json::to_string(&json!({
                "versions": [
                    {
                        "num": "1.2.3",
                        "created_at": created.to_rfc3339(),
                        "yanked": true,
                        "downloads": 223,
                        "license": "MIT",
                        "published_by": {
                            "id": 2,
                            "login": "the_second_owner",
                            "name": "another name",
                            "avatar": "http://anotherthing"
                        }
                    },
                    {
                        "num": "2.2.3",
                        "created_at": Utc::now().to_rfc3339(),
                        "yanked": false,
                        "downloads": 333,
                        "license": "MIT",
                        "published_by": {
                            "id": 1,
                            "login": "owner",
                            "name": "name",
                        }
                    }
                ]}))
                .unwrap(),
            )
            .create();

        assert_eq!(
            api().get_release_data("krate", "1.2.3").unwrap(),
            ReleaseData {
                release_time: created,
                yanked: true,
                downloads: 223,
                published_by: Some(GithubUser {
                    id: 2,
                    avatar: "http://anotherthing".into(),
                    login: "the_second_owner".into(),
                })
            }
        );
    }

    #[test]
    fn get_release_info_without_publisher() {
        let created = Utc::now();
        let _m = mock("GET", "/api/v1/crates/krate/versions")
            .with_header("content-type", "application/json")
            .with_body(
                serde_json::to_string(&json!({
                "versions": [
                    {
                        "num": "1.2.3",
                        "created_at": created.to_rfc3339(),
                        "yanked": true,
                        "downloads": 223,
                        "published_by": null
                    },
                ]}))
                .unwrap(),
            )
            .create();

        assert_eq!(
            api().get_release_data("krate", "1.2.3").unwrap(),
            ReleaseData {
                release_time: created,
                yanked: true,
                downloads: 223,
                published_by: None,
            }
        );
    }
}
