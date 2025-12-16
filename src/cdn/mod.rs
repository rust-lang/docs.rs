use crate::Config;
use anyhow::Result;
use docs_rs_headers::SurrogateKey;
use docs_rs_opentelemetry::AnyMeterProvider;
use docs_rs_types::KrateName;
use opentelemetry::metrics::{Counter, Gauge};
use tracing::{error, info, instrument};

pub(crate) mod fastly;

#[derive(Debug)]
pub struct CdnMetrics {
    fastly_batch_purges_with_surrogate: Counter<u64>,
    fastly_batch_purge_errors: Counter<u64>,
    fastly_purge_surrogate_keys: Counter<u64>,
    fastly_rate_limit_remaining: Gauge<u64>,
    fastly_time_until_rate_limit_reset: Gauge<u64>,
}

impl CdnMetrics {
    pub fn new(meter_provider: &AnyMeterProvider) -> Self {
        let meter = meter_provider.meter("cdn");
        const PREFIX: &str = "docsrs.cdn";
        Self {
            fastly_batch_purges_with_surrogate: meter
                .u64_counter(format!("{PREFIX}.fastly_batch_purges_with_surrogate"))
                .with_unit("1")
                .build(),
            fastly_batch_purge_errors: meter
                .u64_counter(format!("{PREFIX}.fastly_batch_purge_errors"))
                .with_unit("1")
                .build(),
            fastly_purge_surrogate_keys: meter
                .u64_counter(format!("{PREFIX}.fastly_purge_surrogate_keys"))
                .with_unit("1")
                .build(),
            fastly_rate_limit_remaining: meter
                .u64_gauge(format!("{PREFIX}.fasty_rate_limit_remaining"))
                .with_unit("1")
                .build(),
            fastly_time_until_rate_limit_reset: meter
                .u64_gauge(format!("{PREFIX}.fastly_time_until_rate_limit_reset"))
                .with_unit("s")
                .build(),
        }
    }
}

#[instrument(skip(config))]
pub(crate) async fn queue_crate_invalidation(
    config: &Config,
    metrics: &CdnMetrics,
    krate_name: &KrateName,
) -> Result<()> {
    if !config.cache_invalidatable_responses {
        info!("full page cache disabled, skipping queueing invalidation");
        return Ok(());
    }

    if config.fastly_api_token.is_some()
        && let Err(err) = fastly::purge_surrogate_keys(
            config,
            metrics,
            std::iter::once(SurrogateKey::from(krate_name.clone())),
        )
        .await
    {
        // TODO: for now just consume & report the error, I want to see how often that happens.
        // We can then decide if we need more protection mechanisms (like retries or queuing).
        error!(%krate_name, ?err, "error purging Fastly surrogate keys");
    }

    Ok(())
}
