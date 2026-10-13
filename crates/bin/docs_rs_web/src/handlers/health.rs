//! Dependency-free health check for the web process.

use crate::cache::CachePolicy;
use axum::{Extension, response::IntoResponse};

pub(crate) async fn health_check() -> impl IntoResponse {
    (Extension(CachePolicy::NoStoreMustRevalidate), "OK\n")
}

#[cfg(test)]
mod tests {
    use crate::{
        cache::CachePolicy,
        testing::{
            AxumResponseTestExt, AxumRouterTestExt as _, TestEnvironment, TestEnvironmentExt as _,
        },
    };
    use anyhow::Result;

    #[tokio::test(flavor = "multi_thread")]
    async fn health_check() -> Result<()> {
        let env = TestEnvironment::new().await?;
        let web = env.web_app().await;

        let response = web.assert_success("/-/health").await?;

        response.assert_cache_control(CachePolicy::NoStoreMustRevalidate, env.config());
        assert_eq!(response.text().await?, "OK\n",);

        Ok(())
    }
}
