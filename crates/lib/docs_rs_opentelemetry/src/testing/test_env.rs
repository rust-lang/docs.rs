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

#[cfg(test)]
mod tests {
    use super::*;
    use opentelemetry::metrics::Counter;

    const METER: &str = "meter";
    const METRIC: &str = "metric1";

    fn create_metric(meter_provider: &AnyMeterProvider) -> Counter<u64> {
        let meter = meter_provider.meter(METER);
        meter.u64_counter(METRIC).with_unit("1").build()
    }

    #[test]
    fn test_collect_once() -> anyhow::Result<()> {
        let metrics = TestMetrics::new();

        let dummy = create_metric(metrics.provider());
        dummy.add(42, &[]);
        dummy.add(24, &[]);

        assert_eq!(
            66,
            metrics
                .collected_metrics()
                .get_metric(METER, METRIC)?
                .get_u64_counter()
                .value()
        );

        Ok(())
    }

    #[test]
    fn test_collect_twice() -> anyhow::Result<()> {
        let metrics = TestMetrics::new();

        let dummy = create_metric(metrics.provider());
        dummy.add(42, &[]);

        eprintln!("first asserts");
        assert_eq!(
            42,
            metrics
                .collected_metrics()
                .get_metric(METER, METRIC)?
                .get_u64_counter()
                .value()
        );

        dummy.add(24, &[]);

        eprintln!("second asserts");
        assert_eq!(
            66,
            metrics
                .collected_metrics()
                .get_metric(METER, METRIC)?
                .get_u64_counter()
                .value()
        );

        Ok(())
    }
}
