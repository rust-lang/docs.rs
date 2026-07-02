use docs_rs_opentelemetry::AnyMeterProvider;
use opentelemetry::metrics::Counter;

#[derive(Debug)]
pub(crate) struct WatcherMetrics {
    dummy: Counter<u64>,
}

impl WatcherMetrics {
    pub(crate) fn new(meter_provider: &AnyMeterProvider) -> Self {
        let meter = meter_provider.meter("watcher");
        const PREFIX: &str = "docsrs.watcher";
        Self {
            dummy: meter
                .u64_counter(format!("{PREFIX}.dummy"))
                .with_unit("1")
                .build(),
        }
    }
}
