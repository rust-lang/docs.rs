use std::sync::Arc;

use anyhow::{Result, anyhow};
use derive_more::Deref;
use opentelemetry_sdk::metrics::{
    InMemoryMetricExporter, PeriodicReader,
    data::{
        AggregatedMetrics, HistogramDataPoint, Metric, MetricData, ResourceMetrics, SumDataPoint,
    },
};

use crate::metrics::otel::AnyMeterProvider;

/// set up a standalone InMemoryMetricExporter and MeterProvider for testing purposes.
/// For when you want to collect metrics, and then inspect what was collected.
pub(crate) fn setup_test_meter_provider() -> (InMemoryMetricExporter, AnyMeterProvider) {
    let metric_exporter = InMemoryMetricExporter::default();

    (
        metric_exporter.clone(),
        Arc::new(
            opentelemetry_sdk::metrics::SdkMeterProvider::builder()
                .with_reader(PeriodicReader::builder(metric_exporter.clone()).build())
                .build(),
        ),
    )
}

/// small wrapper around the collected result of the InMemoryMetricExporter.
/// For convenience in tests.
#[derive(Debug)]
pub(crate) struct CollectedMetrics(pub(crate) Vec<ResourceMetrics>);

impl CollectedMetrics {
    pub(crate) fn get_metric<'a>(
        &'a self,
        scope: impl AsRef<str>,
        name: impl AsRef<str>,
    ) -> Result<CollectedMetric<'a>> {
        let scope = scope.as_ref();
        let name = name.as_ref();

        let scope_metrics = self
            .0
            .iter()
            .flat_map(|rm| rm.scope_metrics())
            .find(|sm| sm.scope().name() == scope)
            .ok_or_else(|| {
                anyhow!(
                    "Scope '{}' not found in collected metrics: {:?}",
                    scope,
                    self.0
                )
            })?;

        Ok(CollectedMetric(
            scope_metrics
                .metrics()
                .find(|m| m.name() == name)
                .ok_or_else(|| {
                    anyhow!(
                        "Metric '{}' not found in scope '{}': {:?}",
                        name,
                        scope,
                        scope_metrics,
                    )
                })?,
        ))
    }
}

#[derive(Debug, Deref)]
pub(crate) struct CollectedMetric<'a>(&'a Metric);

impl<'a> CollectedMetric<'a> {
    pub(crate) fn get_u64_counter(&'a self) -> &'a SumDataPoint<u64> {
        let AggregatedMetrics::U64(metric_data) = self.data() else {
            panic!("Expected U64 metric data, got: {:?}", self.data());
        };

        let MetricData::Sum(sum) = metric_data else {
            panic!("Expected sum metric data, got: {:?}", metric_data);
        };

        let mut data_points = sum.data_points();

        let result = data_points
            .next()
            .expect("Expected at least one data point");

        debug_assert!(data_points.next().is_none(), "Expected only one data point");

        result
    }

    pub(crate) fn get_f64_histogram(&'a self) -> &'a HistogramDataPoint<f64> {
        let AggregatedMetrics::F64(metric_data) = self.data() else {
            panic!("Expected F64 metric data, got: {:?}", self.data());
        };

        let MetricData::Histogram(histogram) = metric_data else {
            panic!("Expected Histogram metric data, got: {:?}", metric_data);
        };

        let mut data_points = histogram.data_points();

        let result = data_points
            .next()
            .expect("Expected at least one data point");

        debug_assert!(data_points.next().is_none(), "Expected only one data point");

        result
    }
}
