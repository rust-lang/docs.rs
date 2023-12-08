use crate::error::Result;
use crate::Config;
use axum::async_trait;
use chrono::{DateTime, Utc};
use reqwest::{
    header::{HeaderMap, HeaderValue, ACCEPT, AUTHORIZATION, USER_AGENT},
    Client as HttpClient,
};
use serde::Deserialize;
use tracing::{trace, warn};

use crate::repositories::{
    FetchRepositoriesResult, RateLimitReached, Repository, RepositoryForge, RepositoryName,
    APP_USER_AGENT,
};

const GRAPHQL_UPDATE: &str = "query($ids: [ID!]!) {
    nodes(ids: $ids) {
        ... on Repository {
            id
            nameWithOwner
            pushedAt
            description
            stargazerCount
            forkCount
            issues(states: [OPEN]) { totalCount }
        }
    }
    rateLimit {
        remaining
    }
}";

const GRAPHQL_SINGLE: &str = "query($owner: String!, $repo: String!) {
    repository(owner: $owner, name: $repo) {
        id
        nameWithOwner
        pushedAt
        description
        stargazerCount
        forkCount
        issues(states: [OPEN]) { totalCount }
    }
}";

pub struct GitHub {
    endpoint: String,
    client: HttpClient,
    github_updater_min_rate_limit: u32,
}

impl GitHub {
    /// Returns `Err` if the access token has invalid syntax (but *not* if it isn't authorized).
    /// Returns `Ok(None)` if there is no access token.
    pub fn new(config: &Config) -> Result<Option<Self>> {
        Self::with_custom_endpoint(config, "https://api.github.com/graphql")
    }

    pub fn with_custom_endpoint<E: AsRef<str>>(
        config: &Config,
        endpoint: E,
    ) -> Result<Option<Self>> {
        let mut headers = HeaderMap::new();
        headers.insert(USER_AGENT, HeaderValue::from_static(APP_USER_AGENT));
        headers.insert(ACCEPT, HeaderValue::from_static("application/json"));

        if let Some(ref token) = config.github_accesstoken {
            headers.insert(
                AUTHORIZATION,
                HeaderValue::from_str(&format!("token {token}"))?,
            );
        } else {
            warn!("did not collect `github.com` stats as no token was provided");
            return Ok(None);
        }

        let client = HttpClient::builder().default_headers(headers).build()?;

        Ok(Some(GitHub {
            client,
            endpoint: endpoint.as_ref().to_owned(),
            github_updater_min_rate_limit: config.github_updater_min_rate_limit,
        }))
    }
}

#[async_trait]
impl RepositoryForge for GitHub {
    fn host(&self) -> &'static str {
        "github.com"
    }

    fn icon(&self) -> &'static str {
        "github"
    }

    /// How many repositories to update in a single chunk. Values over 100 are probably going to be
    /// rejected by the GraphQL API.
    fn chunk_size(&self) -> usize {
        100
    }

    async fn fetch_repository(&self, name: &RepositoryName) -> Result<Option<Repository>> {
        // Fetch the latest information from the GitHub API.
        let response: GraphResponse<GraphRepositoryNode> = self
            .graphql(
                GRAPHQL_SINGLE,
                serde_json::json!({
                    "owner": name.owner,
                    "repo": name.repo,
                }),
            )
            .await?;

        Ok(response
            .data
            .and_then(|data| data.repository)
            .map(|repo| Repository {
                id: repo.id,
                name_with_owner: repo.name_with_owner,
                description: repo.description,
                last_activity_at: repo.pushed_at,
                stars: repo.stargazer_count,
                forks: repo.fork_count,
                issues: repo.issues.total_count,
            }))
    }

    async fn fetch_repositories(&self, node_ids: &[String]) -> Result<FetchRepositoriesResult> {
        let response: GraphResponse<GraphNodes<Option<GraphRepository>>> = self
            .graphql(
                GRAPHQL_UPDATE,
                serde_json::json!({
                    "ids": node_ids,
                }),
            )
            .await?;

        // The error is returned *before* we reach the rate limit, to ensure we always have an
        // amount of API calls we can make at any time.
        if let Some(ref data) = response.data {
            trace!(
                "GitHub GraphQL rate limit remaining: {}",
                data.rate_limit.remaining
            );
            if data.rate_limit.remaining < self.github_updater_min_rate_limit {
                return Err(RateLimitReached.into());
            }
        }

        let mut ret = FetchRepositoriesResult::default();

        for error in &response.errors {
            use GraphErrorPath::*;
            match (error.error_type.as_str(), error.path.as_slice()) {
                ("NOT_FOUND", [Segment(nodes), Index(idx)]) if nodes == "nodes" => {
                    ret.missing.push(node_ids[*idx as usize].clone());
                }
                ("RATE_LIMITED", []) => {
                    return Err(RateLimitReached.into());
                }
                _ => anyhow::bail!("error updating repositories: {}", error.message),
            }
        }

        if let Some(data) = response.data {
            // When a node is missing (for example if the repository was deleted or made private) the
            // GraphQL API will return *both* a `null` instead of the data in the nodes list and a
            // `NOT_FOUND` error in the errors list.
            for node in data.nodes.into_iter().flatten() {
                let repo = Repository {
                    id: node.id,
                    name_with_owner: node.name_with_owner,
                    description: node.description,
                    last_activity_at: node.pushed_at,
                    stars: node.stargazer_count,
                    forks: node.fork_count,
                    issues: node.issues.total_count,
                };
                ret.present.insert(repo.id.clone(), repo);
            }
        }

        Ok(ret)
    }
}

impl GitHub {
    async fn graphql<T: serde::de::DeserializeOwned>(
        &self,
        query: &str,
        variables: impl serde::Serialize,
    ) -> Result<GraphResponse<T>> {
        Ok(self
            .client
            .post(&self.endpoint)
            .json(&serde_json::json!({
                "query": query,
                "variables": variables,
            }))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?)
    }
}

#[derive(Debug, Deserialize)]
struct GraphResponse<T> {
    data: Option<T>,
    #[serde(default)]
    errors: Vec<GraphError>,
}

#[derive(Debug, Deserialize)]
struct GraphError {
    #[serde(rename = "type")]
    error_type: String,
    #[serde(default)]
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
struct GraphRepositoryNode {
    repository: Option<GraphRepository>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GraphRepository {
    id: String,
    name_with_owner: String,
    pushed_at: Option<DateTime<Utc>>,
    description: Option<String>,
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
mod tests {
    use super::{Config, GitHub};
    use crate::repositories::updater::{repository_name, RepositoryForge};
    use crate::repositories::RateLimitReached;

    async fn mock_server_and_github(config: &Config) -> (mockito::ServerGuard, GitHub) {
        let server = mockito::Server::new_async().await;
        let updater = GitHub::with_custom_endpoint(config, format!("{}/graphql", server.url()))
            .expect("GitHub::new failed")
            .unwrap();

        (server, updater)
    }

    #[test]
    fn test_rate_limit_fail() {
        crate::test::async_wrapper(|env| async move {
            let mut config = env.base_config();
            config.github_accesstoken = Some("qsjdnfqdq".to_owned());
            let (mut server, updater) = mock_server_and_github(&config).await;

            let _m1 = server
                .mock("POST", "/graphql")
                .with_header("content-type", "application/json")
                .with_body(
                    r#"{"errors":[{"type":"RATE_LIMITED","message":"API rate limit exceeded"}]}"#,
                )
                .create();

            match updater.fetch_repositories(&[String::new()]).await {
                Err(e) if e.downcast_ref::<RateLimitReached>().is_some() => {}
                x => panic!("Expected Err(RateLimitReached), found: {x:?}"),
            }
            Ok(())
        });
    }

    #[test]
    fn test_rate_limit_manual() {
        crate::test::async_wrapper(|env| async move {
            let mut config = env.base_config();
            config.github_accesstoken = Some("qsjdnfqdq".to_owned());
            let (mut server, updater) = mock_server_and_github(&config).await;

            let _m1 = server
                .mock("POST", "/graphql")
                .with_header("content-type", "application/json")
                .with_body(r#"{"data": {"nodes": [], "rateLimit": {"remaining": 0}}}"#)
                .create();

            match updater.fetch_repositories(&[String::new()]).await {
                Err(e) if e.downcast_ref::<RateLimitReached>().is_some() => {}
                x => panic!("Expected Err(RateLimitReached), found: {x:?}"),
            }
            Ok(())
        });
    }

    #[test]
    fn not_found() {
        crate::test::async_wrapper(|env| async move {
            let mut config = env.base_config();
            config.github_accesstoken = Some("qsjdnfqdq".to_owned());
            let (mut server, updater) = mock_server_and_github(&config).await;

            let _m1 = server
                .mock("POST", "/graphql")
                .with_header("content-type", "application/json")
                .with_body(
                    r#"{"data": {"nodes": [], "rateLimit": {"remaining": 100000}}, "errors":
                    [{"type": "NOT_FOUND", "path": ["nodes", 0], "message": "none"}]}"#,
                )
                .create();

            match updater.fetch_repositories(&[String::new()]).await {
                Ok(res) => {
                    assert_eq!(res.missing, vec![String::new()]);
                    assert_eq!(res.present.len(), 0);
                }
                x => panic!("Failed: {x:?}"),
            }
            Ok(())
        });
    }

    #[test]
    fn get_repository_info() {
        crate::test::async_wrapper(|env| async move {
            let mut config = env.base_config();
            config.github_accesstoken = Some("qsjdnfqdq".to_owned());
            let (mut server, updater) = mock_server_and_github(&config).await;

            let _m1 = server
                .mock("POST", "/graphql")
                .with_header("content-type", "application/json")
                .with_body(
                    r#"{"data": {"repository": {"id": "hello", "nameWithOwner": "foo/bar",
                    "description": "this is", "stargazerCount": 10, "forkCount": 11,
                    "issues": {"totalCount": 12}}}}"#,
                )
                .create();

            let repo = updater
                .fetch_repository(
                    &repository_name("https://gitlab.com/foo/bar").expect("repository_name failed"),
                )
                .await
                .expect("fetch_repository failed")
                .unwrap();

            assert_eq!(repo.id, "hello");
            assert_eq!(repo.name_with_owner, "foo/bar");
            assert_eq!(repo.description, Some("this is".to_owned()));
            assert_eq!(repo.stars, 10);
            assert_eq!(repo.forks, 11);
            assert_eq!(repo.issues, 12);
            Ok(())
        });
    }
}
