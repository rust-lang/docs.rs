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
    time::Instant,
};

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
        // Include fast acquisitions as well as the configured 30-second timeout.
        let boundaries = vec![
            0.0001, 0.0005, 0.001, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0,
            15.0, 30.0, 60.0, 120.0,
        ];
        Self {
            acquire_duration: meter
                .f64_histogram(format!("{PREFIX}.acquire_duration"))
                .with_description("Time to acquire a connection, including creation and validation")
                .with_unit("s")
                .with_boundaries(boundaries.clone())
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
                .with_boundaries(boundaries)
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
