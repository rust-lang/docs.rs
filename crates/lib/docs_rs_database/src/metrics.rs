use docs_rs_opentelemetry::{AnyMeterProvider, RESPONSE_TIME_HISTOGRAM_BUCKETS};
use opentelemetry::metrics::{Counter, Histogram, ObservableGauge};
use std::time::Duration;

#[derive(Debug)]
pub(crate) struct PoolMetrics {
    pub(crate) failed_connections: Counter<u64>,
    pub(crate) acquire_duration: Histogram<f64>,
    _idle_connections: ObservableGauge<u64>,
    _used_connections: ObservableGauge<u64>,
    _max_connections: ObservableGauge<u64>,
}

impl PoolMetrics {
    pub(crate) fn new(pool: sqlx::PgPool, meter_provider: &AnyMeterProvider) -> Self {
        let meter = meter_provider.meter("pool");
        const PREFIX: &str = "docsrs.db.pool";
        Self {
            acquire_duration: meter
                .f64_histogram(format!("{PREFIX}.acquire_duration"))
                .with_description(
                    "Time to acquire a database connection, including failed attempts",
                )
                .with_boundaries(
                    // Resolve fast acquisitions while retaining the existing buckets for long waits.
                    [
                        Duration::from_micros(50),
                        Duration::from_micros(100),
                        Duration::from_micros(250),
                        Duration::from_micros(500),
                        Duration::from_millis(1),
                        Duration::from_micros(2500),
                    ]
                    .into_iter()
                    .chain(RESPONSE_TIME_HISTOGRAM_BUCKETS.iter().copied())
                    .map(|duration| duration.as_secs_f64())
                    .collect(),
                )
                .with_unit("s")
                .build(),
            failed_connections: meter
                .u64_counter(format!("{PREFIX}.failed_connections"))
                .with_unit("1")
                .build(),
            _idle_connections: meter
                .u64_observable_gauge(format!("{PREFIX}.idle_connections"))
                .with_unit("1")
                .with_callback({
                    let pool = pool.clone();
                    move |observer| {
                        observer.observe(pool.num_idle() as u64, &[]);
                    }
                })
                .build(),
            _used_connections: meter
                .u64_observable_gauge(format!("{PREFIX}.used_connections"))
                .with_unit("1")
                .with_callback({
                    let pool = pool.clone();
                    move |observer| {
                        let used = pool.size() as u64 - pool.num_idle() as u64;
                        observer.observe(used, &[]);
                    }
                })
                .build(),
            _max_connections: meter
                .u64_observable_gauge(format!("{PREFIX}.max_connections"))
                .with_unit("1")
                .with_callback({
                    let pool = pool.clone();
                    move |observer| {
                        observer.observe(pool.size() as u64, &[]);
                    }
                })
                .build(),
        }
    }
}
