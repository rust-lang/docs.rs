use docs_rs_opentelemetry::AnyMeterProvider;
use opentelemetry::metrics::Counter;

#[derive(Debug)]
pub struct BuildQueueMetrics {
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
}
