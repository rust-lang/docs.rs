use crate::{
    RateLimitReached,
    config::Config,
    updater::{FetchRepositoriesResult, Repository, RepositoryForge, RepositoryName},
};
use anyhow::{Result, anyhow, bail};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use docs_rs_utils::APP_USER_AGENT;
use reqwest::{
    Client as HttpClient, StatusCode,
    header::{ACCEPT, AUTHORIZATION, HeaderMap, HeaderValue, USER_AGENT},
};
use serde::{Deserialize, Serialize};
use tracing::{trace, warn};

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
                HeaderValue::from_str(&format!("Bearer {token}"))?,
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
                _ => bail!("error updating repositories: {}", error.message),
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
        let response = self
            .client
            .post(&self.endpoint)
            .json(&serde_json::json!({
                "query": query,
                "variables": variables,
            }))
            .send()
            .await?;

        let status = response.status();
        let body = response.text().await?;

        if status == StatusCode::TOO_MANY_REQUESTS {
            Err(RateLimitReached.into())
        } else if status == StatusCode::FORBIDDEN
            && let Ok(api_error) = serde_json::from_str::<ApiError>(&body)
            && (api_error
                .documentation_url
                .contains("secondary-rate-limits")
                || api_error.message.contains("secondary rate limit"))
        {
            Err(RateLimitReached.into())
        } else if status.is_client_error() || status.is_server_error() {
            Err(anyhow!(
                "GitHub GraphQL response status: {}\n{}",
                status,
                body
            ))
        } else {
            Ok(serde_json::from_str(&body)?)
        }
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

#[derive(Debug, Serialize, Deserialize)]
struct ApiError {
    documentation_url: String,
    message: String,
}

#[cfg(test)]
mod tests {
    use crate::{
        Config, GitHub, RateLimitReached,
        github::ApiError,
        updater::{RepositoryForge, repository_name},
    };
    use anyhow::Result;
    use docs_rs_config::AppConfig as _;
    use reqwest::header::AUTHORIZATION;

    const TEST_TOKEN: &str = "qsjdnfqdq";

    fn github_config() -> anyhow::Result<Config> {
        let mut cfg = Config::from_environment()?;
        cfg.github_accesstoken = Some(TEST_TOKEN.to_owned());
        Ok(cfg)
    }

    async fn mock_server_and_github(config: &Config) -> (mockito::ServerGuard, GitHub) {
        let server = mockito::Server::new_async().await;
        let updater = GitHub::with_custom_endpoint(config, format!("{}/graphql", server.url()))
            .expect("GitHub::new failed")
            .unwrap();

        (server, updater)
    }

    #[tokio::test]
    async fn test_rate_limit_fail() -> Result<()> {
        let config = github_config()?;
        let (mut server, updater) = mock_server_and_github(&config).await;

        let _m1 = server
            .mock("POST", "/graphql")
            .with_header("content-type", "application/json")
            .with_body(
                r#"{"errors":[{"type":"RATE_LIMITED","message":"API rate limit exceeded"}]}"#,
            )
            .match_header(AUTHORIZATION, format!("Bearer {TEST_TOKEN}").as_str())
            .create();

        match updater.fetch_repositories(&[String::new()]).await {
            Err(e) if e.downcast_ref::<RateLimitReached>().is_some() => {}
            x => panic!("Expected Err(RateLimitReached), found: {x:?}"),
        }
        Ok(())
    }

    #[tokio::test]
    async fn test_rate_limit_manual() -> Result<()> {
        let config = github_config()?;
        let (mut server, updater) = mock_server_and_github(&config).await;

        let _m1 = server
            .mock("POST", "/graphql")
            .with_header("content-type", "application/json")
            .with_body(r#"{"data": {"nodes": [], "rateLimit": {"remaining": 0}}}"#)
            .match_header(AUTHORIZATION, format!("Bearer {TEST_TOKEN}").as_str())
            .create();

        match updater.fetch_repositories(&[String::new()]).await {
            Err(e) if e.downcast_ref::<RateLimitReached>().is_some() => {}
            x => panic!("Expected Err(RateLimitReached), found: {x:?}"),
        }
        Ok(())
    }

    #[tokio::test]
    async fn not_found() -> Result<()> {
        let config = github_config()?;
        let (mut server, updater) = mock_server_and_github(&config).await;

        let _m1 = server
            .mock("POST", "/graphql")
            .with_header("content-type", "application/json")
            .with_body(
                r#"{"data": {"nodes": [], "rateLimit": {"remaining": 100000}}, "errors":
                    [{"type": "NOT_FOUND", "path": ["nodes", 0], "message": "none"}]}"#,
            )
            .match_header(AUTHORIZATION, format!("Bearer {TEST_TOKEN}").as_str())
            .create();

        match updater.fetch_repositories(&[String::new()]).await {
            Ok(res) => {
                assert_eq!(res.missing, vec![String::new()]);
                assert_eq!(res.present.len(), 0);
            }
            x => panic!("Failed: {x:?}"),
        }
        Ok(())
    }

    #[tokio::test]
    async fn get_repository_info() -> Result<()> {
        let config = github_config()?;
        let (mut server, updater) = mock_server_and_github(&config).await;

        let _m1 = server
            .mock("POST", "/graphql")
            .with_header("content-type", "application/json")
            .with_body(
                r#"{"data": {"repository": {"id": "hello", "nameWithOwner": "foo/bar",
                    "description": "this is", "stargazerCount": 10, "forkCount": 11,
                    "issues": {"totalCount": 12}}}}"#,
            )
            .match_header(AUTHORIZATION, format!("Bearer {TEST_TOKEN}").as_str())
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
    }

    #[tokio::test]
    async fn test_403_error_with_body() -> Result<()> {
        let config = github_config()?;
        let (mut server, updater) = mock_server_and_github(&config).await;

        let _m1 = server
            .mock("POST", "/graphql")
            .with_header("content-type", "application/json")
            .with_status(403)
            .with_body("some error text")
            .create();

        let err = updater
            .fetch_repository(
                &repository_name("https://gitlab.com/foo/bar").expect("repository_name failed"),
            )
            .await
            .unwrap_err();

        assert_eq!(
            err.to_string(),
            "GitHub GraphQL response status: 403 Forbidden\nsome error text"
        );

        Ok(())
    }

    #[tokio::test]
    async fn test_secondary_rate_limit() -> Result<()> {
        let config = github_config()?;
        let (mut server, updater) = mock_server_and_github(&config).await;

        let _m1 = server
            .mock("POST", "/graphql")
            .with_header("content-type", "application/json")
            .with_status(403)
            .with_body(&serde_json::to_string(&ApiError {
                documentation_url: "https://docs.github.com/graphql/overview/\
                    rate-limits-and-node-limits-for-the-graphql-api#secondary-rate-limits"
                    .into(),
                message: "You have exceeded a secondary rate limit.
                    Please wait a few minutes before you try again.
                    For more on scraping GitHub and how it may affect your rights,
                    please review our Terms of Service
                    (https://docs.github.com/en/site-policy/github-terms/github-terms-of-service)
                    If you reach out to GitHub Support for help, please include the request ID
                    ECEE:193CF9:5A5D684:1866A8EB:698779A9."
                    .into(),
            })?)
            .create();

        assert!(
            updater
                .fetch_repository(
                    &repository_name("https://gitlab.com/foo/bar").expect("repository_name failed"),
                )
                .await
                .unwrap_err()
                .is::<RateLimitReached>()
        );

        Ok(())
    }

    #[tokio::test]
    async fn test_429_rate_limit() -> Result<()> {
        let config = github_config()?;
        let (mut server, updater) = mock_server_and_github(&config).await;

        let _m1 = server
            .mock("POST", "/graphql")
            .with_header("content-type", "application/json")
            .with_status(429)
            .create();

        assert!(
            updater
                .fetch_repository(
                    &repository_name("https://gitlab.com/foo/bar").expect("repository_name failed"),
                )
                .await
                .unwrap_err()
                .is::<RateLimitReached>()
        );

        Ok(())
    }
}
