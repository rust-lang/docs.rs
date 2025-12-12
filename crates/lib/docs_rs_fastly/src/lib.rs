mod config;
mod metrics;

use std::sync::Arc;

pub use config::Config;

use anyhow::{Result, bail};
use chrono::{DateTime, TimeZone as _, Utc};
use docs_rs_database::types::krate_name::KrateName;
use docs_rs_headers::{SURROGATE_KEY, SurrogateKey, SurrogateKeys};
use docs_rs_opentelemetry::AnyMeterProvider;
use docs_rs_utils::APP_USER_AGENT;
use http::{
    HeaderMap, HeaderName, HeaderValue,
    header::{ACCEPT, USER_AGENT},
};
use itertools::Itertools as _;
use opentelemetry::KeyValue;
use tracing::error;

const FASTLY_KEY: HeaderName = HeaderName::from_static("fastly-key");

// https://www.fastly.com/documentation/reference/api/#rate-limiting
const FASTLY_RATELIMIT_REMAINING: HeaderName =
    HeaderName::from_static("fastly-ratelimit-remaining");
const FASTLY_RATELIMIT_RESET: HeaderName = HeaderName::from_static("fastly-ratelimit-reset");

fn fetch_rate_limit_state(headers: &HeaderMap) -> (Option<u64>, Option<DateTime<Utc>>) {
    // https://www.fastly.com/documentation/reference/api/#rate-limiting
    (
        headers
            .get(FASTLY_RATELIMIT_REMAINING)
            .and_then(|hv| hv.to_str().ok())
            .and_then(|s| s.parse().ok()),
        headers
            .get(FASTLY_RATELIMIT_RESET)
            .and_then(|hv| hv.to_str().ok())
            .and_then(|s| s.parse::<i64>().ok())
            .and_then(|ts| Utc.timestamp_opt(ts, 0).single()),
    )
}

pub struct Cdn {
    client: reqwest::Client,
    config: Arc<config::Config>,
    metrics: metrics::CdnMetrics,
}

impl Cdn {
    pub fn from_config(
        config: Arc<config::Config>,
        meter_provider: &AnyMeterProvider,
    ) -> Result<Cdn> {
        let Some(ref api_token) = config.api_token else {
            bail!("Fastly API token not configured");
        };

        let mut headers = HeaderMap::new();
        headers.insert(USER_AGENT, HeaderValue::from_static(APP_USER_AGENT));
        headers.insert(ACCEPT, HeaderValue::from_static("application/json"));
        headers.insert(FASTLY_KEY, HeaderValue::from_str(api_token)?);

        Ok(Self {
            client: reqwest::Client::builder()
                .default_headers(headers)
                .build()?,
            config,
            metrics: metrics::CdnMetrics::new(&meter_provider),
        })
    }

    /// Purge the given surrogate keys from all configured fastly services.
    ///
    /// Accepts any number of surrogate keys, and splits them into appropriately sized
    /// batches for the Fastly API.
    pub(crate) async fn purge_surrogate_keys<I>(&self, keys: I) -> Result<()>
    where
        I: IntoIterator<Item = SurrogateKey>,
    {
        let record_rate_limit_metrics =
            |limit_remaining: Option<u64>, limit_reset: Option<DateTime<Utc>>| {
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
            };

        // the `bulk_purge_tag` supports up to 256 surrogate keys in its list,
        // but I believe we also have to respect the length limits for the full
        // surrogate key header we send in this purge request.
        // see https://www.fastly.com/documentation/reference/api/purging/
        for encoded_surrogate_keys in keys.into_iter().batching(|it| {
            const MAX_SURROGATE_KEYS_IN_BATCH_PURGE: usize = 256;

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
            if let Some(ref sid) = self.config.service_sid {
                // NOTE: we start with just calling the API, and logging an error if they happen.
                // We can then see if we need retries or escalation to full purges.

                let kv = [KeyValue::new("service_sid", sid.clone())];

                // https://www.fastly.com/documentation/reference/api/purging/
                // TODO: investigate how they could help & test
                // soft purge. But later, after the initial migration.
                match self
                    .client
                    .post(
                        self.config
                            .api_host
                            .join(&format!("/service/{}/purge", sid))?,
                    )
                    .header(&SURROGATE_KEY, encoded_surrogate_keys.to_string())
                    .send()
                    .await
                {
                    Ok(response) if response.status().is_success() => {
                        self.metrics.batch_purges_with_surrogate.add(1, &kv);
                        self.metrics
                            .purge_surrogate_keys
                            .add(encoded_surrogate_keys.key_count() as u64, &kv);

                        let (limit_remaining, limit_reset) =
                            fetch_rate_limit_state(response.headers());
                        record_rate_limit_metrics(limit_remaining, limit_reset);
                    }
                    Ok(error_response) => {
                        self.metrics.batch_purge_errors.add(1, &kv);

                        let (limit_remaining, limit_reset) =
                            fetch_rate_limit_state(error_response.headers());
                        record_rate_limit_metrics(limit_remaining, limit_reset);

                        let limit_reset = limit_reset.map(|dt| dt.to_rfc3339());

                        let status = error_response.status();
                        let content = error_response.text().await.unwrap_or_default();
                        error!(
                            sid,
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
                        self.metrics.batch_purge_errors.add(1, &kv);
                        error!(
                            sid,
                            ?err,
                            %encoded_surrogate_keys,
                            "Failed to purge Fastly surrogate keys for service"
                        );
                    }
                };
            }
        }

        Ok(())
    }

    pub async fn queue_crate_invalidation(&self, krate_name: &KrateName) -> Result<()> {
        if let Err(err) = self
            .purge_surrogate_keys(std::iter::once(SurrogateKey::from(krate_name.clone())))
            .await
        {
            // TODO: for now just consume & report the error, I want to see how often that happens.
            // We can then decide if we need more protection mechanisms (like retries or queuing).
            error!(%krate_name, ?err, "error purging Fastly surrogate keys");
        }

        Ok(())
    }
}

// #[cfg(test)]
// mod tests {
//     use super::*;
//     use crate::test::{TestEnvironment, setup_test_meter_provider};
//     use chrono::TimeZone;
//     use std::str::FromStr as _;

//     #[test]
//     fn test_read_rate_limit() {
//         // https://www.fastly.com/documentation/reference/api/#rate-limiting
//         let mut hm = HeaderMap::new();
//         hm.insert(FASTLY_RATELIMIT_REMAINING, HeaderValue::from_static("999"));
//         hm.insert(
//             FASTLY_RATELIMIT_RESET,
//             HeaderValue::from_static("1452032384"),
//         );

//         let (remaining, reset) = fetch_rate_limit_state(&hm);
//         assert_eq!(remaining, Some(999));
//         assert_eq!(
//             reset,
//             Some(Utc.timestamp_opt(1452032384, 0).single().unwrap())
//         );
//     }

//     #[tokio::test]
//     async fn test_purge() -> Result<()> {
//         let mut fastly_api = mockito::Server::new_async().await;

//         let config = TestEnvironment::base_config()
//             .fastly_api_host(fastly_api.url().parse().unwrap())
//             .fastly_api_token(Some("test-token".into()))
//             .fastly_service_sid(Some("test-sid-1".into()))
//             .build()?;

//         let m = fastly_api
//             .mock("POST", "/service/test-sid-1/purge")
//             .match_header(FASTLY_KEY, "test-token")
//             .match_header(&SURROGATE_KEY, "crate-foo crate-bar")
//             .with_status(200)
//             .create_async()
//             .await;

//         let (_exporter, meter_provider) = setup_test_meter_provider();
//         let metrics = CdnMetrics::new(&meter_provider);

//         purge_surrogate_keys(
//             &config,
//             &metrics,
//             vec![
//                 SurrogateKey::from_str("crate-foo").unwrap(),
//                 SurrogateKey::from_str("crate-bar").unwrap(),
//             ],
//         )
//         .await?;

//         m.assert_async().await;

//         Ok(())
//     }

//     #[tokio::test]
//     async fn test_purge_err_doesnt_err() -> Result<()> {
//         let mut fastly_api = mockito::Server::new_async().await;

//         let config = TestEnvironment::base_config()
//             .fastly_api_host(fastly_api.url().parse().unwrap())
//             .fastly_api_token(Some("test-token".into()))
//             .fastly_service_sid(Some("test-sid-1".into()))
//             .build()?;

//         let m = fastly_api
//             .mock("POST", "/service/test-sid-1/purge")
//             .match_header(FASTLY_KEY, "test-token")
//             .match_header(&SURROGATE_KEY, "crate-foo crate-bar")
//             .with_status(500)
//             .create_async()
//             .await;

//         let (_exporter, meter_provider) = setup_test_meter_provider();
//         let metrics = CdnMetrics::new(&meter_provider);

//         assert!(
//             purge_surrogate_keys(
//                 &config,
//                 &metrics,
//                 vec![
//                     SurrogateKey::from_str("crate-foo").unwrap(),
//                     SurrogateKey::from_str("crate-bar").unwrap(),
//                 ],
//             )
//             .await
//             .is_ok()
//         );

//         m.assert_async().await;

//         Ok(())
//     }

//     #[tokio::test]
//     async fn test_purge_split_requests() -> Result<()> {
//         let mut fastly_api = mockito::Server::new_async().await;

//         let config = TestEnvironment::base_config()
//             .fastly_api_host(fastly_api.url().parse().unwrap())
//             .fastly_api_token(Some("test-token".into()))
//             .fastly_service_sid(Some("test-sid-1".into()))
//             .build()?;

//         let m = fastly_api
//             .mock("POST", "/service/test-sid-1/purge")
//             .match_header(FASTLY_KEY, "test-token")
//             .match_request(|request| {
//                 let [surrogate_keys] = request.header(&SURROGATE_KEY)[..] else {
//                     panic!("expected one SURROGATE_KEY header");
//                 };
//                 let surrogate_keys: SurrogateKeys =
//                     surrogate_keys.to_str().unwrap().parse().unwrap();

//                 assert!(
//                     // first request
//                     surrogate_keys.key_count() == 256 ||
//                     // second request
//                     surrogate_keys.key_count() == 94
//                 );

//                 true
//             })
//             .expect(2) // 300 keys below
//             .with_status(200)
//             .create_async()
//             .await;

//         let (_exporter, meter_provider) = setup_test_meter_provider();
//         let metrics = CdnMetrics::new(&meter_provider);

//         let keys: Vec<_> = (0..350)
//             .map(|n| SurrogateKey::from_str(&format!("crate-foo-{n}")).unwrap())
//             .collect();

//         purge_surrogate_keys(&config, &metrics, keys).await?;

//         m.assert_async().await;

//         Ok(())
//     }
// }
