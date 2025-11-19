use crate::{
    Config,
    db::types::krate_name::KrateName,
    metrics::{CDN_INVALIDATION_HISTOGRAM_BUCKETS, otel::AnyMeterProvider},
    web::headers::SurrogateKey,
};
use anyhow::{Context, Result};
use opentelemetry::metrics::{Counter, Gauge, Histogram};
use tracing::{debug, error, info, instrument};

pub(crate) mod cloudfront;
pub(crate) mod fastly;

#[derive(Debug)]
pub struct CdnMetrics {
    invalidation_time: Histogram<f64>,
    queue_time: Histogram<f64>,
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
            invalidation_time: meter
                .f64_histogram(format!("{PREFIX}.invalidation_time"))
                .with_boundaries(CDN_INVALIDATION_HISTOGRAM_BUCKETS.to_vec())
                .with_unit("s")
                .build(),
            queue_time: meter
                .f64_histogram(format!("{PREFIX}.queue_time"))
                .with_boundaries(CDN_INVALIDATION_HISTOGRAM_BUCKETS.to_vec())
                .with_unit("s")
                .build(),
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

#[instrument(skip(conn, config))]
pub(crate) async fn queue_crate_invalidation(
    conn: &mut sqlx::PgConnection,
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

    /// cloudfront needs a queue to work around a concurrency limit of just 15 parallel
    /// wildcard invalidations.
    async fn add(
        conn: &mut sqlx::PgConnection,
        name: &str,
        distribution_id: &str,
        path_patterns: &[&str],
    ) -> Result<()> {
        for pattern in path_patterns {
            debug!(distribution_id, pattern, "enqueueing web CDN invalidation");
            sqlx::query!(
                "INSERT INTO cdn_invalidation_queue (crate, cdn_distribution_id, path_pattern)
                 VALUES ($1, $2, $3)",
                name,
                distribution_id,
                pattern
            )
            .execute(&mut *conn)
            .await?;
        }
        Ok(())
    }

    if let Some(distribution_id) = config.cloudfront_distribution_id_web.as_ref() {
        add(
            conn,
            krate_name,
            distribution_id,
            &[&format!("/{krate_name}*"), &format!("/crate/{krate_name}*")],
        )
        .await
        .context("error enqueueing web CDN invalidation")?;
    }
    if let Some(distribution_id) = config.cloudfront_distribution_id_static.as_ref() {
        add(
            conn,
            krate_name,
            distribution_id,
            &[&format!("/rustdoc/{krate_name}*")],
        )
        .await
        .context("error enqueueing static CDN invalidation")?;
    }

    Ok(())
}
