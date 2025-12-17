use crate::{CdnMetrics, Config, cdn::CdnBehaviour, rate_limit::fetch_rate_limit_state};
use anyhow::{Result, bail};
use chrono::{DateTime, Utc};
use docs_rs_headers::{SURROGATE_KEY, SurrogateKey, SurrogateKeys};
use docs_rs_opentelemetry::AnyMeterProvider;
use docs_rs_utils::APP_USER_AGENT;
use http::{
    HeaderMap, HeaderName, HeaderValue,
    header::{ACCEPT, USER_AGENT},
};
use itertools::Itertools as _;
use opentelemetry::KeyValue;
use tracing::{error, instrument};
use url::Url;

const FASTLY_KEY: HeaderName = HeaderName::from_static("fastly-key");

// the `bulk_purge_tag` supports up to 256 surrogate keys in its list,
// but we additionally respect the length limits for the full
// surrogate key header we send in this purge request.
// see https://www.fastly.com/documentation/reference/api/purging/
const MAX_SURROGATE_KEYS_IN_BATCH_PURGE: usize = 256;

#[derive(Debug)]
pub struct RealCdn {
    client: reqwest::Client,
    api_host: Url,
    service_sid: String,
    metrics: CdnMetrics,
    metric_attributes: Vec<KeyValue>,
}

impl RealCdn {
    pub(crate) fn from_config(config: &Config, meter_provider: &AnyMeterProvider) -> Result<Self> {
        let Some(ref api_token) = config.api_token else {
            bail!("Fastly API token not configured");
        };

        let Some(ref service_sid) = config.service_sid else {
            bail!("Fastly service SID not configured");
        };

        let mut headers = HeaderMap::new();
        headers.insert(USER_AGENT, HeaderValue::from_static(APP_USER_AGENT));
        headers.insert(ACCEPT, HeaderValue::from_static("application/json"));
        headers.insert(FASTLY_KEY, HeaderValue::from_str(api_token)?);

        Ok(Self {
            client: reqwest::Client::builder()
                .default_headers(headers)
                .build()?,
            service_sid: service_sid.clone(),
            api_host: config.api_host.clone(),
            metrics: CdnMetrics::new(meter_provider),
            metric_attributes: vec![KeyValue::new("service_sid", service_sid.clone())],
        })
    }

    fn record_rate_limit_metrics(
        &self,
        limit_remaining: Option<u64>,
        limit_reset: Option<DateTime<Utc>>,
    ) {
        if let Some(limit_remaining) = limit_remaining {
            self.metrics
                .rate_limit_remaining
                .record(limit_remaining, &[]);
        }

        if let Some(limit_reset) = limit_reset {
            self.metrics
                .time_until_rate_limit_reset
                .record((limit_reset - Utc::now()).num_seconds() as u64, &[]);
        }
    }
}

impl CdnBehaviour for RealCdn {
    /// Purge the given surrogate keys from all configured fastly services.
    ///
    /// Accepts any number of surrogate keys, and splits them into appropriately sized
    /// batches for the Fastly API.
    #[instrument(skip(self, keys), fields(service_sid = %self.service_sid))]
    async fn purge_surrogate_keys<I>(&self, keys: I) -> Result<()>
    where
        I: IntoIterator<Item = SurrogateKey> + Send + 'static,
        I::IntoIter: Send,
    {
        for encoded_surrogate_keys in keys.into_iter().batching(|it| {
            // SurrogateKeys::from_iter::until_full only consumes as many elements as will fit into
            // the header.
            // The rest is up to the next `batching` iteration.
            let keys =
                SurrogateKeys::from_iter_until_full(it.take(MAX_SURROGATE_KEYS_IN_BATCH_PURGE));

            if keys.key_count() > 0 {
                Some(keys)
            } else {
                None
            }
        }) {
            // NOTE: we start with just calling the API, and logging an error if they happen.
            // We can then see if we need retries or escalation to full purges.

            // https://www.fastly.com/documentation/reference/api/purging/
            // TODO: investigate how they could help & test
            // soft purge. But later, after the initial migration.
            match self
                .client
                .post(
                    self.api_host
                        .join(&format!("/service/{}/purge", self.service_sid))?,
                )
                .header(&SURROGATE_KEY, encoded_surrogate_keys.to_string())
                .send()
                .await
            {
                Ok(response) if response.status().is_success() => {
                    self.metrics
                        .batch_purges_with_surrogate
                        .add(1, &self.metric_attributes);
                    self.metrics.purge_surrogate_keys.add(
                        encoded_surrogate_keys.key_count() as u64,
                        &self.metric_attributes,
                    );

                    let (limit_remaining, limit_reset) = fetch_rate_limit_state(response.headers());
                    self.record_rate_limit_metrics(limit_remaining, limit_reset);
                }
                Ok(error_response) => {
                    self.metrics
                        .batch_purge_errors
                        .add(1, &self.metric_attributes);

                    let (limit_remaining, limit_reset) =
                        fetch_rate_limit_state(error_response.headers());
                    self.record_rate_limit_metrics(limit_remaining, limit_reset);

                    let limit_reset = limit_reset.map(|dt| dt.to_rfc3339());

                    let status = error_response.status();
                    let content = error_response.text().await.unwrap_or_default();
                    error!(
                        %status,
                        content,
                        %encoded_surrogate_keys,
                        rate_limit_remaining=limit_remaining,
                        rate_limit_reset=limit_reset,
                        "Failed to purge Fastly surrogate keys for service"
                    );
                }
                Err(err) => {
                    // connection errors or similar, where we don't have a response
                    self.metrics
                        .batch_purge_errors
                        .add(1, &self.metric_attributes);
                    error!(
                        ?err,
                        %encoded_surrogate_keys,
                        "Failed to purge Fastly surrogate keys for service"
                    );
                }
            };
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use docs_rs_opentelemetry::testing::TestMetrics;
    use std::str::FromStr as _;

    #[tokio::test]
    async fn test_purge() -> Result<()> {
        let mut fastly_api = mockito::Server::new_async().await;

        let config = Config {
            api_host: fastly_api.url().parse().unwrap(),
            api_token: Some("test-token".into()),
            service_sid: Some("test-sid-1".into()),
        };

        let m = fastly_api
            .mock("POST", "/service/test-sid-1/purge")
            .match_header(FASTLY_KEY, "test-token")
            .match_header(&SURROGATE_KEY, "crate-bar crate-foo")
            .with_status(200)
            .create_async()
            .await;

        let test_metrics = TestMetrics::new();
        let cdn = RealCdn::from_config(&config, test_metrics.provider())?;

        cdn.purge_surrogate_keys(vec![
            SurrogateKey::from_str("crate-foo").unwrap(),
            SurrogateKey::from_str("crate-bar").unwrap(),
        ])
        .await?;

        m.assert_async().await;

        Ok(())
    }

    #[tokio::test]
    async fn test_purge_err_doesnt_err() -> Result<()> {
        let mut fastly_api = mockito::Server::new_async().await;

        let config = Config {
            api_host: fastly_api.url().parse().unwrap(),
            api_token: Some("test-token".into()),
            service_sid: Some("test-sid-1".into()),
        };

        let m = fastly_api
            .mock("POST", "/service/test-sid-1/purge")
            .match_header(FASTLY_KEY, "test-token")
            .match_header(&SURROGATE_KEY, "crate-bar crate-foo")
            .with_status(500)
            .create_async()
            .await;

        let test_metrics = TestMetrics::new();
        let cdn = RealCdn::from_config(&config, test_metrics.provider())?;

        assert!(
            cdn.purge_surrogate_keys(vec![
                SurrogateKey::from_str("crate-foo").unwrap(),
                SurrogateKey::from_str("crate-bar").unwrap(),
            ],)
                .await
                .is_ok()
        );

        m.assert_async().await;

        Ok(())
    }

    #[tokio::test]
    async fn test_purge_split_requests() -> Result<()> {
        let mut fastly_api = mockito::Server::new_async().await;

        let config = Config {
            api_host: fastly_api.url().parse().unwrap(),
            api_token: Some("test-token".into()),
            service_sid: Some("test-sid-1".into()),
        };

        let m = fastly_api
            .mock("POST", "/service/test-sid-1/purge")
            .match_header(FASTLY_KEY, "test-token")
            .match_request(|request| {
                let [surrogate_keys] = request.header(&SURROGATE_KEY)[..] else {
                    panic!("expected one SURROGATE_KEY header");
                };
                let surrogate_keys: SurrogateKeys =
                    surrogate_keys.to_str().unwrap().parse().unwrap();

                assert!(
                    // first request
                    surrogate_keys.key_count() == 256 ||
                    // second request
                    surrogate_keys.key_count() == 94
                );

                true
            })
            .expect(2) // 300 keys below
            .with_status(200)
            .create_async()
            .await;

        let test_metrics = TestMetrics::new();
        let cdn = RealCdn::from_config(&config, test_metrics.provider())?;

        let keys: Vec<_> = (0..350)
            .map(|n| SurrogateKey::from_str(&format!("crate-foo-{n}")).unwrap())
            .collect();

        cdn.purge_surrogate_keys(keys).await?;

        m.assert_async().await;

        Ok(())
    }
}
