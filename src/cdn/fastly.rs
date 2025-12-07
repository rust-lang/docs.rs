use crate::{
    cdn::CdnMetrics,
    config::Config,
    web::headers::{SURROGATE_KEY, SurrogateKey, SurrogateKeys},
};
use anyhow::{Result, anyhow, bail};
use chrono::{DateTime, TimeZone as _, Utc};
use docs_rs_utils::APP_USER_AGENT;
use http::{
    HeaderMap, HeaderName, HeaderValue,
    header::{ACCEPT, USER_AGENT},
};
use itertools::Itertools as _;
use opentelemetry::KeyValue;
use std::sync::OnceLock;
use tracing::error;

const FASTLY_KEY: HeaderName = HeaderName::from_static("fastly-key");

// https://www.fastly.com/documentation/reference/api/#rate-limiting
const FASTLY_RATELIMIT_REMAINING: HeaderName =
    HeaderName::from_static("fastly-ratelimit-remaining");
const FASTLY_RATELIMIT_RESET: HeaderName = HeaderName::from_static("fastyly-ratelimit-reset");

static CLIENT: OnceLock<Result<reqwest::Client>> = OnceLock::new();

fn fastly_client(api_token: impl AsRef<str>) -> anyhow::Result<&'static reqwest::Client> {
    CLIENT
        .get_or_init(|| -> Result<_> {
            let mut headers = HeaderMap::new();
            headers.insert(USER_AGENT, HeaderValue::from_static(APP_USER_AGENT));
            headers.insert(ACCEPT, HeaderValue::from_static("application/json"));
            headers.insert(FASTLY_KEY, HeaderValue::from_str(api_token.as_ref())?);

            Ok(reqwest::Client::builder()
                .default_headers(headers)
                .build()?)
        })
        .as_ref()
        .map_err(|err| anyhow!("reqwest Client init failed: {}", err))
}

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

/// Purge the given surrogate keys from all configured fastly services.
///
/// Accepts any number of surrogate keys, and splits them into appropriately sized
/// batches for the Fastly API.
pub(crate) async fn purge_surrogate_keys<I>(
    config: &Config,
    metrics: &CdnMetrics,
    keys: I,
) -> Result<()>
where
    I: IntoIterator<Item = SurrogateKey>,
{
    let Some(api_token) = &config.fastly_api_token else {
        bail!("Fastly API token not configured");
    };

    let client = fastly_client(api_token)?;

    let record_rate_limit_metrics =
        |limit_remaining: Option<u64>, limit_reset: Option<DateTime<Utc>>| {
            if let Some(limit_remaining) = limit_remaining {
                metrics
                    .fastly_rate_limit_remaining
                    .record(limit_remaining, &[]);
            }

            if let Some(limit_reset) = limit_reset {
                metrics
                    .fastly_time_until_rate_limit_reset
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
        let keys = SurrogateKeys::from_iter_until_full(it.take(MAX_SURROGATE_KEYS_IN_BATCH_PURGE));

        if keys.key_count() > 0 {
            Some(keys)
        } else {
            None
        }
    }) {
        if let Some(ref sid) = config.fastly_service_sid {
            // NOTE: we start with just calling the API, and logging an error if they happen.
            // We can then see if we need retries or escalation to full purges.

            let kv = [KeyValue::new("service_sid", sid.clone())];

            // https://www.fastly.com/documentation/reference/api/purging/
            // TODO: investigate how they could help & test
            // soft purge. But later, after the initial migration.
            match client
                .post(
                    config
                        .fastly_api_host
                        .join(&format!("/service/{}/purge", sid))?,
                )
                .header(&SURROGATE_KEY, encoded_surrogate_keys.to_string())
                .send()
                .await
            {
                Ok(response) if response.status().is_success() => {
                    metrics.fastly_batch_purges_with_surrogate.add(1, &kv);
                    metrics
                        .fastly_purge_surrogate_keys
                        .add(encoded_surrogate_keys.key_count() as u64, &kv);

                    let (limit_remaining, limit_reset) = fetch_rate_limit_state(response.headers());
                    record_rate_limit_metrics(limit_remaining, limit_reset);
                }
                Ok(error_response) => {
                    metrics.fastly_batch_purge_errors.add(1, &kv);

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
                    metrics.fastly_batch_purge_errors.add(1, &kv);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test::{TestEnvironment, setup_test_meter_provider};
    use chrono::TimeZone;
    use std::str::FromStr as _;

    #[test]
    fn test_read_rate_limit() {
        // https://www.fastly.com/documentation/reference/api/#rate-limiting
        let mut hm = HeaderMap::new();
        hm.insert(FASTLY_RATELIMIT_REMAINING, HeaderValue::from_static("999"));
        hm.insert(
            FASTLY_RATELIMIT_RESET,
            HeaderValue::from_static("1452032384"),
        );

        let (remaining, reset) = fetch_rate_limit_state(&hm);
        assert_eq!(remaining, Some(999));
        assert_eq!(
            reset,
            Some(Utc.timestamp_opt(1452032384, 0).single().unwrap())
        );
    }

    #[tokio::test]
    async fn test_purge() -> Result<()> {
        let mut fastly_api = mockito::Server::new_async().await;

        let config = TestEnvironment::base_config()
            .fastly_api_host(fastly_api.url().parse().unwrap())
            .fastly_api_token(Some("test-token".into()))
            .fastly_service_sid(Some("test-sid-1".into()))
            .build()?;

        let m = fastly_api
            .mock("POST", "/service/test-sid-1/purge")
            .match_header(FASTLY_KEY, "test-token")
            .match_header(&SURROGATE_KEY, "crate-foo crate-bar")
            .with_status(200)
            .create_async()
            .await;

        let (_exporter, meter_provider) = setup_test_meter_provider();
        let metrics = CdnMetrics::new(&meter_provider);

        purge_surrogate_keys(
            &config,
            &metrics,
            vec![
                SurrogateKey::from_str("crate-foo").unwrap(),
                SurrogateKey::from_str("crate-bar").unwrap(),
            ],
        )
        .await?;

        m.assert_async().await;

        Ok(())
    }

    #[tokio::test]
    async fn test_purge_err_doesnt_err() -> Result<()> {
        let mut fastly_api = mockito::Server::new_async().await;

        let config = TestEnvironment::base_config()
            .fastly_api_host(fastly_api.url().parse().unwrap())
            .fastly_api_token(Some("test-token".into()))
            .fastly_service_sid(Some("test-sid-1".into()))
            .build()?;

        let m = fastly_api
            .mock("POST", "/service/test-sid-1/purge")
            .match_header(FASTLY_KEY, "test-token")
            .match_header(&SURROGATE_KEY, "crate-foo crate-bar")
            .with_status(500)
            .create_async()
            .await;

        let (_exporter, meter_provider) = setup_test_meter_provider();
        let metrics = CdnMetrics::new(&meter_provider);

        assert!(
            purge_surrogate_keys(
                &config,
                &metrics,
                vec![
                    SurrogateKey::from_str("crate-foo").unwrap(),
                    SurrogateKey::from_str("crate-bar").unwrap(),
                ],
            )
            .await
            .is_ok()
        );

        m.assert_async().await;

        Ok(())
    }

    #[tokio::test]
    async fn test_purge_split_requests() -> Result<()> {
        let mut fastly_api = mockito::Server::new_async().await;

        let config = TestEnvironment::base_config()
            .fastly_api_host(fastly_api.url().parse().unwrap())
            .fastly_api_token(Some("test-token".into()))
            .fastly_service_sid(Some("test-sid-1".into()))
            .build()?;

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

        let (_exporter, meter_provider) = setup_test_meter_provider();
        let metrics = CdnMetrics::new(&meter_provider);

        let keys: Vec<_> = (0..350)
            .map(|n| SurrogateKey::from_str(&format!("crate-foo-{n}")).unwrap())
            .collect();

        purge_surrogate_keys(&config, &metrics, keys).await?;

        m.assert_async().await;

        Ok(())
    }
}
