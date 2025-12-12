use docs_rs_opentelemetry::AnyMeterProvider;
use opentelemetry::metrics::{Counter, Gauge};

#[derive(Debug)]
pub struct CdnMetrics {
    pub(crate) batch_purges_with_surrogate: Counter<u64>,
    pub(crate) batch_purge_errors: Counter<u64>,
    pub(crate) purge_surrogate_keys: Counter<u64>,
    pub(crate) rate_limit_remaining: Gauge<u64>,
    pub(crate) time_until_rate_limit_reset: Gauge<u64>,
}

impl CdnMetrics {
    pub fn new(meter_provider: &AnyMeterProvider) -> Self {
        let meter = meter_provider.meter("cdn");
        const PREFIX: &str = "docsrs.cdn";
        Self {
            batch_purges_with_surrogate: meter
                .u64_counter(format!("{PREFIX}.fastly_batch_purges_with_surrogate"))
                .with_unit("1")
                .build(),
            batch_purge_errors: meter
                .u64_counter(format!("{PREFIX}.fastly_batch_purge_errors"))
                .with_unit("1")
                .build(),
            purge_surrogate_keys: meter
                .u64_counter(format!("{PREFIX}.fastly_purge_surrogate_keys"))
                .with_unit("1")
                .build(),
            rate_limit_remaining: meter
                .u64_gauge(format!("{PREFIX}.fasty_rate_limit_remaining"))
                .with_unit("1")
                .build(),
            time_until_rate_limit_reset: meter
                .u64_gauge(format!("{PREFIX}.fastly_time_until_rate_limit_reset"))
                .with_unit("s")
                .build(),
        }
    }
}
