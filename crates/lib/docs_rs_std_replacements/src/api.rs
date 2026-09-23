use crate::{Config, ReplacementDetails, ReplacementMap};
use anyhow::Result;
use docs_rs_reqwest::{CachedResult, Client};
use docs_rs_types::KrateName;
use std::sync::Arc;
use url::Url;

/// A single snapshot, fetched lazily and refreshed on demand when its TTL expires.
#[derive(Debug)]
pub struct StdReplacements {
    client: Client<ReplacementMap>,
    url: Url,
}

impl StdReplacements {
    /// Create a client without fetching data. Fetch failures propagate to the caller.
    pub fn from_config(config: &Config) -> Result<Self> {
        Ok(Self {
            client: Client::builder()
                .max_retries(config.max_retries)
                .cache_capacity(1u64)
                .default_ttl(config.cache_default_ttl)
                .build()?,
            url: config.url.clone(),
        })
    }

    /// Return the alternative for a crate, refreshing an expired snapshot if needed.
    pub async fn get(
        &self,
        name: &KrateName,
    ) -> Result<CachedResult<Option<Arc<ReplacementDetails>>>> {
        Ok(self
            .client
            .get(&self.url)
            .await?
            .map(|map| map.and_then(|map| map.get(name).cloned())))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{StdReplacementMockServer, std_replacement};
    use docs_rs_headers::CacheControl;
    use docs_rs_types::testing::KRATE;
    use std::time::Duration;

    async fn fixture() -> Result<(StdReplacementMockServer, StdReplacements)> {
        let server = StdReplacementMockServer::new().await;
        let api = StdReplacements::from_config(&server.config().build())?;
        Ok((server, api))
    }

    #[tokio::test]
    async fn crate_lookups_share_dataset_and_preserve_ttl() -> Result<()> {
        let (mut server, api) = fixture().await?;
        server = server
            .mock()
            .replacement(KRATE, std_replacement("replacement"))
            .cache_control(CacheControl::new().with_max_age(Duration::from_mins(10)))
            .start()
            .await;

        let name = KRATE;
        let missing_name = KrateName::from_static("missing");
        let (present, missing) = tokio::try_join!(api.get(&name), api.get(&missing_name))?;
        assert_eq!(present.value.unwrap().description(), "replacement");
        assert!(missing.value.is_none());
        for ttl in [present.ttl, missing.ttl] {
            assert!(ttl <= Duration::from_secs(600));
            assert!(ttl > Duration::from_secs(590));
        }
        server.assert_async().await;
        Ok(())
    }

    #[tokio::test]
    async fn refreshed_dataset_can_remove_replacements() -> Result<()> {
        let (mut server, api) = fixture().await?;
        server = server
            .mock()
            .replacement(KRATE, std_replacement("old"))
            .cache_control(CacheControl::new().with_max_age(Duration::from_secs(1)))
            .start()
            .await;

        api.get(&KRATE).await?;
        server.remove_mock();

        server = server.mock().start().await;
        tokio::time::sleep(Duration::from_millis(1100)).await;
        assert!(api.get(&KRATE).await?.value.is_none());
        server.assert_async().await;
        Ok(())
    }

    #[tokio::test]
    async fn forwards_configured_fallback_ttl() -> Result<()> {
        let server = StdReplacementMockServer::new()
            .await
            .mock()
            .replacement(KRATE, std_replacement("replacement"))
            .start()
            .await;

        let api = StdReplacements::from_config(
            &server
                .config()
                .cache_default_ttl(Duration::from_secs(90).into())
                .build(),
        )?;

        let result = api.get(&KRATE).await?;
        assert_eq!(result.value.unwrap().description(), "replacement");
        assert!(result.ttl <= Duration::from_secs(90));
        assert!(result.ttl > Duration::from_secs(85));
        server.assert_async().await;
        Ok(())
    }
}
