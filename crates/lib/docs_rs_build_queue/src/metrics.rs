use docs_rs_opentelemetry::AnyMeterProvider;
use docs_rs_types::Duration;
use opentelemetry::{
    KeyValue,
    metrics::{Counter, Histogram},
};

/// Queue times from one second through four weeks
const QUEUE_TIME_BUCKETS: &[Duration; 23] = &[
    Duration::from_secs(1),
    Duration::from_secs(5),
    Duration::from_secs(10),
    Duration::from_secs(30),
    Duration::from_mins(1),
    Duration::from_mins(2),
    Duration::from_mins(5),
    Duration::from_mins(10),
    Duration::from_mins(30),
    Duration::from_hours(1),
    Duration::from_hours(2),
    Duration::from_hours(6),
    Duration::from_hours(12),
    Duration::from_days(1),
    Duration::from_days(2),
    Duration::from_days(3),
    Duration::from_days(4),
    Duration::from_days(5),
    Duration::from_days(6),
    Duration::from_weeks(1),
    Duration::from_weeks(2),
    Duration::from_weeks(3),
    Duration::from_weeks(4),
];

#[derive(Debug)]
pub struct BuildQueueMetrics {
    queue_time: Histogram<f64>,
    pub(crate) queued_builds: Counter<u64>,
    /// hard errors (= Result::Err from the builder).
    /// Not the same as "normal" build failures = rustc failed compiling.
    pub(crate) failed_crates_count: Counter<u64>,
}

impl BuildQueueMetrics {
    pub fn new(meter_provider: &AnyMeterProvider) -> Self {
        let meter = meter_provider.meter("build_queue");
        const PREFIX: &str = "docsrs.build_queue";
        Self {
            queue_time: meter
                .f64_histogram(format!("{PREFIX}.queue_time"))
                .with_unit("s")
                .with_boundaries(
                    QUEUE_TIME_BUCKETS
                        .iter()
                        .map(|duration| duration.as_secs_f64())
                        .collect(),
                )
                .with_description("Time from enqueue to first build attempt")
                .build(),
            queued_builds: meter
                .u64_counter(format!("{PREFIX}.queued_builds"))
                .with_unit("1")
                .build(),
            failed_crates_count: meter
                .u64_counter(format!("{PREFIX}.failed_crates_count"))
                .with_unit("1")
                .build(),
        }
    }

    pub(crate) fn record_queue_time(&self, elapsed: Duration, priority: i32) {
        self.queue_time.record(
            elapsed.as_secs_f64(),
            &[KeyValue::new("priority", priority.to_string())],
        );
    }
}
