use docs_rs_opentelemetry::AnyMeterProvider;
use opentelemetry::{
    KeyValue,
    metrics::{Counter, Histogram, ObservableGauge},
};
use std::{
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

/// Pool timing buckets covering fast acquisitions and long connection holds.
const POOL_TIME_HISTOGRAM_BUCKETS: &[Duration; 18] = &[
    Duration::from_micros(100),
    Duration::from_micros(500),
    Duration::from_millis(1),
    Duration::from_millis(5),
    Duration::from_millis(10),
    Duration::from_millis(25),
    Duration::from_millis(50),
    Duration::from_millis(100),
    Duration::from_millis(250),
    Duration::from_millis(500),
    Duration::from_secs(1),
    Duration::from_millis(2500),
    Duration::from_secs(5),
    Duration::from_secs(10),
    Duration::from_secs(15),
    Duration::from_secs(30),
    Duration::from_secs(60),
    Duration::from_secs(120),
];

#[derive(Debug)]
pub(crate) struct PoolMetrics {
    pub(crate) failed_connections: Counter<u64>,
    acquire_duration: Histogram<f64>,
    acquires: Counter<u64>,
    pub(crate) connection_hold_duration: Histogram<f64>,
    pending_acquires: Arc<AtomicU64>,
    _pending_acquires: ObservableGauge<u64>,
    _idle_connections: ObservableGauge<u64>,
    _used_connections: ObservableGauge<u64>,
    _max_connections: ObservableGauge<u64>,
}

impl PoolMetrics {
    pub(crate) fn new(pool: sqlx::PgPool, meter_provider: &AnyMeterProvider) -> Self {
        let meter = meter_provider.meter("pool");
        const PREFIX: &str = "docsrs.db.pool";
        let pending_acquires = Arc::new(AtomicU64::new(0));
        Self {
            acquire_duration: meter
                .f64_histogram(format!("{PREFIX}.acquire_duration"))
                .with_description("Time to acquire a connection, including creation and validation")
                .with_unit("s")
                .with_boundaries(
                    POOL_TIME_HISTOGRAM_BUCKETS
                        .iter()
                        .map(Duration::as_secs_f64)
                        .collect(),
                )
                .build(),
            acquires: meter
                .u64_counter(format!("{PREFIX}.acquires"))
                .with_description("Acquisition attempts by outcome, including cancellation")
                .with_unit("1")
                .build(),
            connection_hold_duration: meter
                .f64_histogram(format!("{PREFIX}.connection_hold_duration"))
                .with_description("Time a connection is held by a caller, excluding pool return")
                .with_unit("s")
                .with_boundaries(
                    POOL_TIME_HISTOGRAM_BUCKETS
                        .iter()
                        .map(Duration::as_secs_f64)
                        .collect(),
                )
                .build(),
            _pending_acquires: meter
                .u64_observable_gauge(format!("{PREFIX}.pending_acquires"))
                .with_description("Acquisition attempts currently in progress")
                .with_unit("1")
                .with_callback({
                    let pending_acquires = pending_acquires.clone();
                    move |observer| observer.observe(pending_acquires.load(Ordering::Relaxed), &[])
                })
                .build(),
            pending_acquires,
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

impl PoolMetrics {
    pub(crate) fn start_acquire(self: &Arc<Self>) -> AcquireMetricsGuard {
        self.pending_acquires.fetch_add(1, Ordering::Relaxed);
        AcquireMetricsGuard {
            metrics: self.clone(),
            started: Instant::now(),
            outcome: "cancelled",
        }
    }
}

/// Dropping an unfinished acquisition still decrements pending and records cancellation.
pub(crate) struct AcquireMetricsGuard {
    metrics: Arc<PoolMetrics>,
    started: Instant,
    outcome: &'static str,
}

impl AcquireMetricsGuard {
    pub(crate) fn finish(mut self, outcome: &'static str) {
        self.outcome = outcome;
    }
}

impl Drop for AcquireMetricsGuard {
    fn drop(&mut self) {
        let attrs = [KeyValue::new("outcome", self.outcome)];
        self.metrics
            .acquire_duration
            .record(self.started.elapsed().as_secs_f64(), &attrs);
        self.metrics.acquires.add(1, &attrs);
        self.metrics
            .pending_acquires
            .fetch_sub(1, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use docs_rs_opentelemetry::testing::TestMetrics;

    #[tokio::test]
    async fn acquisition_outcomes_clear_pending() {
        for outcome in ["success", "timeout", "error", "cancelled"] {
            let telemetry = TestMetrics::new();
            let pool = sqlx::PgPool::connect_lazy("postgres://localhost/test").unwrap();
            let metrics = Arc::new(PoolMetrics::new(pool, telemetry.provider()));
            let guard = metrics.start_acquire();
            assert_eq!(metrics.pending_acquires.load(Ordering::Relaxed), 1);
            if outcome == "cancelled" {
                drop(guard);
            } else {
                guard.finish(outcome);
            }
            assert_eq!(metrics.pending_acquires.load(Ordering::Relaxed), 0);
            let collected = telemetry.collected_metrics();
            let counter = collected
                .get_metric("pool", "docsrs.db.pool.acquires")
                .unwrap();
            let point = counter.get_u64_counter();
            assert_eq!(point.value(), 1);
            assert!(
                point
                    .attributes()
                    .any(|a| a == &KeyValue::new("outcome", outcome))
            );
            assert_eq!(
                collected
                    .get_metric("pool", "docsrs.db.pool.acquire_duration")
                    .unwrap()
                    .get_f64_histogram()
                    .count(),
                1
            );
        }
    }
}
