use anyhow::{Result, anyhow};
use opentelemetry_sdk::metrics::data::{
    AggregatedMetrics, HistogramDataPoint, Metric, MetricData, ResourceMetrics, SumDataPoint,
};

/// small wrapper around the collected result of the InMemoryMetricExporter.
/// For convenience in tests.
#[derive(Debug)]
pub struct CollectedMetrics(pub Vec<ResourceMetrics>);

impl CollectedMetrics {
    pub fn get_metric<'a>(
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
            .filter(|sm| sm.scope().name() == scope)
            .last()
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
                .filter(|m| m.name() == name)
                .last()
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

pub struct CollectedMetric<'a>(&'a Metric);

impl core::ops::Deref for CollectedMetric<'_> {
    type Target = Metric;

    fn deref(&self) -> &Self::Target {
        self.0
    }
}

impl<'a> CollectedMetric<'a> {
    pub fn get_u64_counter(&'a self) -> &'a SumDataPoint<u64> {
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

    pub fn get_f64_histogram(&'a self) -> &'a HistogramDataPoint<f64> {
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
