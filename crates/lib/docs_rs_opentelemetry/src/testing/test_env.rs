use crate::{AnyMeterProvider, testing::collected_metrics::CollectedMetrics};
use opentelemetry_sdk::metrics::{InMemoryMetricExporter, PeriodicReader};
use std::sync::Arc;

/// A test metrics environment that collects metrics in memory.
pub struct TestMetrics {
    exporter: InMemoryMetricExporter,
    provider: AnyMeterProvider,
}

impl TestMetrics {
    pub fn new() -> Self {
        let metric_exporter = InMemoryMetricExporter::default();

        Self {
            exporter: metric_exporter.clone(),
            provider: Arc::new(
                opentelemetry_sdk::metrics::SdkMeterProvider::builder()
                    .with_reader(PeriodicReader::builder(metric_exporter.clone()).build())
                    .build(),
            ),
        }
    }

    pub fn collected_metrics(&self) -> CollectedMetrics {
        self.provider.force_flush().unwrap();
        CollectedMetrics(self.exporter.get_finished_metrics().unwrap())
    }

    pub fn provider(&self) -> &AnyMeterProvider {
        &self.provider
    }
}

impl Default for TestMetrics {
    fn default() -> Self {
        Self::new()
    }
}
