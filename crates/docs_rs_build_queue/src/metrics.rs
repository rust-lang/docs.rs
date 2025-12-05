use docs_rs_opentelemetry::AnyMeterProvider;
use opentelemetry::metrics::Counter;

#[derive(Debug)]
pub struct BuildQueueMetrics {
    queued_builds: Counter<u64>,
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
        }
    }
}
