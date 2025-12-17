use docs_rs_opentelemetry::AnyMeterProvider;
use opentelemetry::metrics::Counter;

#[derive(Debug)]
pub(crate) struct StorageMetrics {
    pub(crate) uploaded_files: Counter<u64>,
}

impl StorageMetrics {
    pub(crate) fn new(meter_provider: &AnyMeterProvider) -> Self {
        let meter = meter_provider.meter("storage");
        const PREFIX: &str = "docsrs.storage";
        Self {
            uploaded_files: meter
                .u64_counter(format!("{PREFIX}.uploaded_files"))
                .with_unit("1")
                .build(),
        }
    }
}
