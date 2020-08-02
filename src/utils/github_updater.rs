use crate::error::Result;
use crate::{db::Pool, Config};
use chrono::{DateTime, Utc};
use failure::err_msg;
use log::{debug, warn};
use postgres::Client;
use regex::Regex;
use reqwest::header::{HeaderValue, ACCEPT, AUTHORIZATION, USER_AGENT};
use serde::Deserialize;

const APP_USER_AGENT: &str = concat!(
    env!("CARGO_PKG_NAME"),
    " ",
    include_str!(concat!(env!("OUT_DIR"), "/git_version"))
);

/// Fields we need use in cratesfyi
#[derive(Debug)]
struct GitHubFields {
    description: String,
    stars: i64,
    forks: i64,
    issues: i64,
    last_commit: DateTime<Utc>,
}

pub struct GithubUpdater {
    client: reqwest::blocking::Client,
    pool: Pool,
}

impl GithubUpdater {
    pub fn new(config: &Config, pool: Pool) -> Result<Self> {
        let mut headers = vec![
            (USER_AGENT, HeaderValue::from_static(APP_USER_AGENT)),
            (ACCEPT, HeaderValue::from_static("application/json")),
        ];

        if let Some((username, accesstoken)) = config.github_auth() {
            let basicauth = format!(
                "Basic {}",
                base64::encode(format!("{}:{}", username, accesstoken))
            );
            headers.push((AUTHORIZATION, HeaderValue::from_str(&basicauth).unwrap()));
        } else {
            warn!("No GitHub authorization specified, will be working with very low rate limits");
        }

        let client = reqwest::blocking::Client::builder()
            .default_headers(headers.into_iter().collect())
            .build()?;

        Ok(GithubUpdater { client, pool })
    }

    /// Updates github fields in crates table
    pub fn update_all_crates(&self) -> Result<()> {
        debug!("Starting update of all crates");

        if self.is_rate_limited()? {
            warn!("Skipping update because of rate limit");
            return Ok(());
        }

        let mut conn = self.pool.get()?;
        // TODO: This query assumes repository field in Cargo.toml is
        //       always the same across all versions of a crate
        let rows = conn.query(
            "SELECT DISTINCT ON (crates.name)
                    crates.name,
                    crates.id,
                    releases.repository_url
             FROM crates
             INNER JOIN releases ON releases.crate_id = crates.id
             WHERE releases.repository_url ~ '^https?://github.com' AND
                   (crates.github_last_update < NOW() - INTERVAL '1 day' OR
                    crates.github_last_update IS NULL)
             ORDER BY crates.name, releases.release_time DESC",
            &[],
        )?;

        for row in &rows {
            let crate_name: String = row.get(0);
            let crate_id: i32 = row.get(1);
            let repository_url: String = row.get(2);

            debug!("Updating {}", crate_name);
            if let Err(err) = self.update_crate(&mut conn, crate_id, &repository_url) {
                if self.is_rate_limited()? {
                    warn!("Skipping remaining updates because of rate limit");
                    return Ok(());
                }
                warn!("Failed to update {}: {}", crate_name, err);
            }
        }

        debug!("Completed all updates");
        Ok(())
    }

    fn is_rate_limited(&self) -> Result<bool> {
        #[derive(Deserialize)]
        struct Response {
            resources: Resources,
        }

        #[derive(Deserialize)]
        struct Resources {
            core: Resource,
        }

        #[derive(Deserialize)]
        struct Resource {
            remaining: u64,
        }

        let url = "https://api.github.com/rate_limit";
        let response: Response = self.client.get(url).send()?.error_for_status()?.json()?;

        Ok(response.resources.core.remaining == 0)
    }

    fn update_crate(&self, conn: &mut Client, crate_id: i32, repository_url: &str) -> Result<()> {
        let path =
            get_github_path(repository_url).ok_or_else(|| err_msg("Failed to get github path"))?;
        let fields = self.get_github_fields(&path)?;

        conn.execute(
            "UPDATE crates
             SET github_description = $1,
                 github_stars = $2, github_forks = $3,
                 github_issues = $4, github_last_commit = $5,
                 github_last_update = NOW()
             WHERE id = $6",
            &[
                &fields.description,
                &(fields.stars as i32),
                &(fields.forks as i32),
                &(fields.issues as i32),
                &fields.last_commit.naive_utc(),
                &crate_id,
            ],
        )?;

        Ok(())
    }

    fn get_github_fields(&self, path: &str) -> Result<GitHubFields> {
        #[derive(Deserialize)]
        struct Response {
            #[serde(default)]
            description: Option<String>,
            #[serde(default)]
            stargazers_count: i64,
            #[serde(default)]
            forks_count: i64,
            #[serde(default)]
            open_issues: i64,
            #[serde(default = "Utc::now")]
            pushed_at: DateTime<Utc>,
        }

        let url = format!("https://api.github.com/repos/{}", path);
        let response: Response = self.client.get(&url).send()?.error_for_status()?.json()?;

        Ok(GitHubFields {
            description: response.description.unwrap_or_default(),
            stars: response.stargazers_count,
            forks: response.forks_count,
            issues: response.open_issues,
            last_commit: response.pushed_at,
        })
    }
}

fn get_github_path(url: &str) -> Option<String> {
    let re = Regex::new(r"https?://github\.com/([\w\._-]+)/([\w\._-]+)").unwrap();
    match re.captures(url) {
        Some(cap) => {
            let username = cap.get(1).unwrap().as_str();
            let reponame = cap.get(2).unwrap().as_str();

            let reponame = if reponame.ends_with(".git") {
                reponame.split(".git").next().unwrap()
            } else {
                reponame
            };

            Some(format!("{}/{}", username, reponame))
        }

        None => None,
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_get_github_path() {
        assert_eq!(
            get_github_path("https://github.com/onur/cratesfyi"),
            Some("onur/cratesfyi".to_string())
        );
        assert_eq!(
            get_github_path("http://github.com/onur/cratesfyi"),
            Some("onur/cratesfyi".to_string())
        );
        assert_eq!(
            get_github_path("https://github.com/onur/cratesfyi.git"),
            Some("onur/cratesfyi".to_string())
        );
        assert_eq!(
            get_github_path("https://github.com/onur23cmD_M_R_L_/crates_fy-i"),
            Some("onur23cmD_M_R_L_/crates_fy-i".to_string())
        );
        assert_eq!(
            get_github_path("https://github.com/docopt/docopt.rs"),
            Some("docopt/docopt.rs".to_string())
        );
    }
}
