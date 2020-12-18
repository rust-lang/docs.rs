use crate::error::Result;
use crate::{db::Pool, Config};
use chrono::{DateTime, Utc};
use log::{info, trace, warn};
use postgres::Client;
use regex::Regex;
use reqwest::{
    blocking::Client as HttpClient,
    header::{HeaderMap, HeaderValue, ACCEPT, AUTHORIZATION, USER_AGENT},
};
use serde::Deserialize;
use std::sync::Arc;

const APP_USER_AGENT: &str = concat!(
    env!("CARGO_PKG_NAME"),
    " ",
    include_str!(concat!(env!("OUT_DIR"), "/git_version"))
);

const GRAPHQL_UPDATE: &str = "query($ids: [ID!]!) {
    nodes(ids: $ids) {
        ... on Repository {
            id
            nameWithOwner
            pushedAt
            description
            stargazerCount
            forkCount
            issues { totalCount }
        }
    }
    rateLimit {
        remaining
    }
}";

/// How many repositories to update in a single chunk. Values over 100 are probably going to be
/// rejected by the GraphQL API.
const UPDATE_CHUNK_SIZE: usize = 100;

pub struct GithubUpdater {
    client: HttpClient,
    pool: Pool,
    config: Arc<Config>,
}

impl GithubUpdater {
    pub fn new(config: Arc<Config>, pool: Pool) -> Result<Self> {
        let mut headers = HeaderMap::new();
        headers.insert(USER_AGENT, HeaderValue::from_static(APP_USER_AGENT));
        headers.insert(ACCEPT, HeaderValue::from_static("application/json"));

        if let Some(token) = &config.github_accesstoken {
            headers.insert(
                AUTHORIZATION,
                HeaderValue::from_str(&format!("token {}", token))?,
            );
        } else {
            warn!("No GitHub authorization specified, will be working with very low rate limits");
        }

        let client = HttpClient::builder().default_headers(headers).build()?;

        Ok(GithubUpdater {
            client,
            pool,
            config,
        })
    }

    /// Updates github fields in crates table
    pub fn update_all_crates(&self) -> Result<()> {
        info!("started updating GitHub repository stats");

        let mut conn = self.pool.get()?;
        let needs_update = conn
            .query(
                "SELECT id FROM github_repos WHERE updated_at < NOW() - INTERVAL '1 day';",
                &[],
            )?
            .into_iter()
            .map(|row| row.get(0))
            .collect::<Vec<String>>();

        if needs_update.is_empty() {
            info!("no GitHub repository stats needed to be updated");
            return Ok(());
        }

        for chunk in needs_update.chunks(UPDATE_CHUNK_SIZE) {
            if let Err(err) = self.update_repositories(&mut conn, &chunk) {
                if err.downcast_ref::<RateLimitReached>().is_some() {
                    warn!("rate limit reached, blocked the GitHub repository stats updater");
                    return Ok(());
                }
                return Err(err);
            }
        }

        info!("finished updating GitHub repository stats");
        Ok(())
    }

    fn update_repositories(&self, conn: &mut Client, node_ids: &[String]) -> Result<()> {
        let response: GraphResponse<GraphNodes<Option<GraphRepository>>> = self
            .client
            .post("https://api.github.com/graphql")
            .json(&serde_json::json!({
                "query": GRAPHQL_UPDATE,
                "variables": {
                    "ids": node_ids,
                },
            }))
            .send()?
            .error_for_status()?
            .json()?;

        // The error is returned *before* we reach the rate limit, to ensure we always have an
        // amount of API calls we can make at any time.
        trace!(
            "GitHub GraphQL rate limit remaining: {}",
            response.data.rate_limit.remaining
        );
        if response.data.rate_limit.remaining < self.config.github_updater_min_rate_limit {
            return Err(RateLimitReached.into());
        }

        // When a node is missing (for example if the repository was deleted or made private) the
        // GraphQL API will return *both* a `null` instead of the data in the nodes list and a
        // `NOT_FOUND` error in the errors list.
        for node in &response.data.nodes {
            if let Some(node) = node {
                self.store_repository(conn, &node)?;
            }
        }
        for error in &response.errors {
            use GraphErrorPath::*;
            match (error.error_type.as_str(), error.path.as_slice()) {
                ("NOT_FOUND", [Segment(nodes), Index(idx)]) if nodes == "nodes" => {
                    self.delete_repository(conn, &node_ids[*idx as usize])?;
                }
                _ => failure::bail!("error updating repositories: {}", error.message),
            }
        }

        Ok(())
    }

    fn store_repository(&self, conn: &mut Client, repo: &GraphRepository) -> Result<()> {
        trace!(
            "storing GitHub repository stats for {}",
            repo.name_with_owner
        );
        conn.execute(
            "INSERT INTO github_repos (
                 id, name, description, last_commit, stars, forks, issues, updated_at
             ) VALUES ($1, $2, $3, $4, $5, $6, $7, NOW())
             ON CONFLICT (id) DO
             UPDATE SET
                 name = $2,
                 description = $3,
                 last_commit = $4,
                 stars = $5,
                 forks = $6,
                 issues = $7,
                 updated_at = NOW();",
            &[
                &repo.id,
                &repo.name_with_owner,
                &repo.description,
                &repo.pushed_at.naive_utc(),
                &(repo.stargazer_count as i32),
                &(repo.fork_count as i32),
                &(repo.issues.total_count as i32),
            ],
        )?;
        Ok(())
    }

    fn delete_repository(&self, conn: &mut Client, id: &str) -> Result<()> {
        trace!("removing GitHub repository stats for ID {}", id);
        conn.execute("DELETE FROM github_repos WHERE id = $1;", &[&id])?;
        Ok(())
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

#[derive(Debug, failure::Fail)]
#[fail(display = "rate limit reached")]
struct RateLimitReached;

#[derive(Debug, Deserialize)]
struct GraphResponse<T> {
    data: T,
    #[serde(default)]
    errors: Vec<GraphError>,
}

#[derive(Debug, Deserialize)]
struct GraphError {
    #[serde(rename = "type")]
    error_type: String,
    path: Vec<GraphErrorPath>,
    message: String,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum GraphErrorPath {
    Segment(String),
    Index(i64),
}

#[derive(Debug, Deserialize)]
struct GraphRateLimit {
    remaining: u32,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GraphNodes<T> {
    nodes: Vec<T>,
    rate_limit: GraphRateLimit,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GraphRepository {
    id: String,
    name_with_owner: String,
    pushed_at: DateTime<Utc>,
    description: String,
    stargazer_count: i64,
    fork_count: i64,
    issues: GraphIssues,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GraphIssues {
    total_count: i64,
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
