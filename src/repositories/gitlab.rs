use crate::error::Result;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use reqwest::{
    Client as HttpClient,
    header::{ACCEPT, AUTHORIZATION, HeaderMap, HeaderValue, USER_AGENT},
};
use serde::Deserialize;
use std::collections::HashSet;
use std::str::FromStr;
use tracing::warn;

use crate::{
    APP_USER_AGENT,
    repositories::{
        FetchRepositoriesResult, RateLimitReached, Repository, RepositoryForge, RepositoryName,
    },
};

const GRAPHQL_UPDATE: &str = "query($ids: [ID!]!) {
    projects(ids: $ids) {
        nodes {
            id
            fullPath
            lastActivityAt
            description
            starCount
            forksCount
            openIssuesCount
        }
    }
}";

const GRAPHQL_SINGLE: &str = "query($fullPath: ID!) {
    project(fullPath: $fullPath) {
        id
        fullPath
        lastActivityAt
        description
        starCount
        forksCount
        openIssuesCount
    }
}";

pub struct GitLab {
    client: HttpClient,
    host: &'static str,
    endpoint: String,
}

impl GitLab {
    pub fn new(host: &'static str, access_token: &Option<String>) -> Result<Self> {
        Self::with_custom_endpoint(host, access_token, format!("https://{host}/api/graphql"))
    }

    pub fn with_custom_endpoint<E: AsRef<str>>(
        host: &'static str,
        access_token: &Option<String>,
        endpoint: E,
    ) -> Result<Self> {
        let mut headers = HeaderMap::new();
        headers.insert(USER_AGENT, HeaderValue::from_static(APP_USER_AGENT));
        headers.insert(ACCEPT, HeaderValue::from_static("application/json"));

        if let Some(token) = access_token {
            headers.insert(
                AUTHORIZATION,
                HeaderValue::from_str(&format!("Bearer {token}"))?,
            );
        } else {
            warn!(
                "will try to retrieve `{}` stats without token since none was provided",
                host
            );
        }

        let client = HttpClient::builder().default_headers(headers).build()?;
        Ok(GitLab {
            client,
            host,
            endpoint: endpoint.as_ref().to_string(),
        })
    }
}

#[async_trait]
impl RepositoryForge for GitLab {
    fn host(&self) -> &'static str {
        self.host
    }

    fn icon(&self) -> &'static str {
        "gitlab"
    }

    fn chunk_size(&self) -> usize {
        100
    }

    async fn fetch_repository(&self, name: &RepositoryName) -> Result<Option<Repository>> {
        let project_path = format!("{}/{}", name.owner, name.repo);
        // Fetch the latest information from the GitLab API.
        let response: (GraphResponse<GraphProjectNode>, Option<usize>) = self
            .graphql(
                GRAPHQL_SINGLE,
                serde_json::json!({
                    "fullPath": &project_path,
                }),
            )
            .await?;
        let (response, rate_limit) = response;
        if let Some(repo) = response.data.and_then(|d| d.project) {
            Ok(Some(Repository {
                id: repo.id,
                name_with_owner: repo.full_path,
                description: repo.description,
                last_activity_at: repo.last_activity_at,
                stars: repo.star_count,
                forks: repo.forks_count,
                issues: repo.open_issues_count.unwrap_or(0),
            }))
        } else if rate_limit.map(|x| x < 1).unwrap_or(false) {
            Err(RateLimitReached.into())
        } else {
            Ok(None)
        }
    }

    async fn fetch_repositories(&self, ids: &[String]) -> Result<FetchRepositoriesResult> {
        let response: (
            GraphResponse<GraphProjects<Option<GraphProject>>>,
            Option<usize>,
        ) = self
            .graphql(
                GRAPHQL_UPDATE,
                serde_json::json!({
                    "ids": ids,
                }),
            )
            .await?;
        let (response, rate_limit) = response;
        let mut ret = FetchRepositoriesResult::default();
        // When gitlab doesn't find an ID, it simply doesn't list it. So we need to actually check
        // which nodes remain at the end to delete their DB entry.
        let mut node_ids: HashSet<&String> = ids.iter().collect();

        if let Some(data) = response.data {
            if !response.errors.is_empty() {
                anyhow::bail!("error updating repositories: {:?}", response.errors);
            }
            for node in data.projects.nodes.into_iter().flatten() {
                let repo = Repository {
                    id: node.id,
                    name_with_owner: node.full_path,
                    description: node.description,
                    last_activity_at: node.last_activity_at,
                    stars: node.star_count,
                    forks: node.forks_count,
                    issues: node.open_issues_count.unwrap_or(0),
                };
                let id = repo.id.clone();
                node_ids.remove(&id);
                ret.present.insert(id, repo);
            }

            if ret.present.is_empty() && rate_limit.map(|x| x < 1).unwrap_or(false) {
                return Err(RateLimitReached.into());
            }

            // Those nodes were not returned by gitlab, meaning they don't exist (anymore?).
            ret.missing = node_ids.into_iter().map(|s| s.to_owned()).collect();

            Ok(ret)
        } else if rate_limit.map(|x| x < 1).unwrap_or(false) {
            Err(RateLimitReached.into())
        } else {
            anyhow::bail!("no data")
        }
    }
}

impl GitLab {
    async fn graphql<T: serde::de::DeserializeOwned + std::fmt::Debug>(
        &self,
        query: &str,
        variables: impl serde::Serialize,
    ) -> Result<(GraphResponse<T>, Option<usize>)> {
        let res = self
            .client
            .post(&self.endpoint)
            .json(&serde_json::json!({
                "query": query,
                "variables": variables,
            }))
            .send()
            .await?
            .error_for_status()?;
        // There are a few other header values that might interesting so keeping them here:
        // * RateLimit-Observed: '1'
        // * RateLimit-Remaining: '1999'
        // * RateLimit-ResetTime: 'Wed, 10 Feb 2021 21:31:42 GMT'
        // * RateLimit-Limit: '2000'
        let rate_limit = res
            .headers()
            .get("RateLimit-Remaining")
            .and_then(|x| usize::from_str(x.to_str().ok()?).ok());
        Ok((res.json().await?, rate_limit))
    }
}

#[derive(Debug, Deserialize)]
struct GraphProjects<T> {
    projects: GraphNodes<T>,
}

#[derive(Debug, Deserialize)]
struct GraphResponse<T> {
    data: Option<T>,
    #[serde(default)]
    errors: Vec<GraphError>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)] // used by anyhow for error reporting; apparently the compiler isn't smart enough to tell
struct GraphError {
    message: String,
    locations: Vec<GraphErrorLocation>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct GraphErrorLocation {
    line: u32,
    column: u32,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct GraphRateLimit {
    remaining: u32,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GraphNodes<T> {
    nodes: Vec<T>,
}

#[derive(Debug, Deserialize)]
struct GraphProjectNode {
    project: Option<GraphProject>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GraphProject {
    id: String,
    full_path: String,
    last_activity_at: Option<DateTime<Utc>>,
    description: Option<String>,
    star_count: i64,
    forks_count: i64,
    open_issues_count: Option<i64>,
}

#[cfg(test)]
mod tests {
    use super::GitLab;
    use crate::repositories::RateLimitReached;
    use crate::repositories::updater::{RepositoryForge, repository_name};
    use anyhow::Result;

    async fn mock_server_and_gitlab() -> (mockito::ServerGuard, GitLab) {
        let server = mockito::Server::new_async().await;
        let updater = GitLab::with_custom_endpoint(
            "gitlab.com",
            &None,
            format!("{}/api/graphql", server.url()),
        )
        .expect("GitLab::new failed");

        (server, updater)
    }

    #[tokio::test]
    async fn test_rate_limit() -> Result<()> {
        let (mut server, updater) = mock_server_and_gitlab().await;

        let _m1 = server
            .mock("POST", "/api/graphql")
            .with_header("content-type", "application/json")
            .with_header("RateLimit-Remaining", "0")
            .with_body("{}")
            .create();

        match updater
            .fetch_repository(
                &repository_name("https://gitlab.com/foo/bar").expect("repository_name failed"),
            )
            .await
        {
            Err(e) if e.downcast_ref::<RateLimitReached>().is_some() => {}
            x => panic!("Expected Err(RateLimitReached), found: {x:?}"),
        }
        match updater.fetch_repositories(&[String::new()]).await {
            Err(e) if e.downcast_ref::<RateLimitReached>().is_some() => {}
            x => panic!("Expected Err(RateLimitReached), found: {x:?}"),
        }
        Ok(())
    }

    #[tokio::test]
    async fn not_found() -> Result<()> {
        let (mut server, updater) = mock_server_and_gitlab().await;

        let _m1 = server
            .mock("POST", "/api/graphql")
            .with_header("content-type", "application/json")
            .with_body(r#"{"data": {"projects": {"nodes": []}}}"#)
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
        let (mut server, updater) = mock_server_and_gitlab().await;

        let _m1 = server
            .mock("POST", "/api/graphql")
            .with_header("content-type", "application/json")
            .with_body(
                r#"{"data": {"project": {"id": "hello", "fullPath": "foo/bar",
                "description": "this is", "starCount": 10, "forksCount": 11,
                "openIssuesCount": 12}}}"#,
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
    }
}
