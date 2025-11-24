use crate::{
    cdn::CdnMetrics,
    config::Config,
    web::headers::{SurrogateKey, SurrogateKeys},
};
use anyhow::{Result, bail};
use chrono::{DateTime, Utc};
use fastly_api::apis::{
    configuration::{ApiKey, Configuration},
    purge_api::{BulkPurgeTagParams, bulk_purge_tag},
};
use itertools::Itertools as _;
use opentelemetry::KeyValue;
use tracing::error;

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

    let mut cfg = Configuration {
        api_key: Some(ApiKey {
            prefix: None,
            key: api_token.to_owned(),
        }),
        ..Default::default()
    };

    // the `bulk_purge_tag` supports up to 256 surrogate keys in its list,
    // but I believe we also have to respect the length limits for the full
    // surrogate key header.
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
        for sid in config
            .fastly_service_sid_web
            .iter()
            .chain(config.fastly_service_sid_static.iter())
        {
            // NOTE: we start with just calling the API, and logging an error if they happen.
            // We can then see if we need retries or escalation to full purges.

            let kv = [KeyValue::new("service_sid", sid.clone())];
            match bulk_purge_tag(
                &mut cfg,
                BulkPurgeTagParams {
                    service_id: sid.to_owned(),
                    // TODO: investigate how they could help & test
                    // soft purge. But later, after the initial migration.
                    fastly_soft_purge: None,
                    surrogate_key: Some(encoded_surrogate_keys.to_string()),
                    ..Default::default()
                },
            )
            .await
            {
                Ok(_) => {
                    metrics.fastly_batch_purges_with_surrogate.add(1, &kv);
                    metrics
                        .fastly_purge_surrogate_keys
                        .add(encoded_surrogate_keys.key_count() as u64, &kv);
                }
                Err(err) => {
                    metrics.fastly_batch_purge_errors.add(1, &kv);
                    let rate_limit_reset =
                        DateTime::<Utc>::from_timestamp(cfg.rate_limit_reset as i64, 0)
                            .map(|dt| dt.to_rfc3339());
                    error!(
                        sid,
                        ?err,
                        %encoded_surrogate_keys,
                        rate_limit_remaining=cfg.rate_limit_remaining,
                        rate_limit_reset,
                        "Failed to purge Fastly surrogate keys for service"
                    );
                }
            }
        }
    }

    metrics
        .fastly_rate_limit_remaining
        .record(cfg.rate_limit_remaining, &[]);
    metrics.fastly_time_until_rate_limit_reset.record(
        cfg.rate_limit_reset
            .saturating_sub(Utc::now().timestamp() as u64),
        &[],
    );

    Ok(())
}
