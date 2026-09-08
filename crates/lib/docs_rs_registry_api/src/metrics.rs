use docs_rs_opentelemetry::AnyMeterProvider;
use opentelemetry::{
    KeyValue,
    metrics::{Counter, Histogram},
};
use std::{future::Future, time::Instant};

#[derive(Debug, Clone, Copy)]
pub(crate) enum Operation {
    IndexConfig,
    IndexCrate,
    ApiSearch,
    ApiOwners,
    Download,
}

impl Operation {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::IndexConfig => "index_config",
            Self::IndexCrate => "index_crate",
            Self::ApiSearch => "api_search",
            Self::ApiOwners => "api_owners",
            Self::Download => "download",
        }
    }
}

#[derive(Debug)]
pub struct RegistryApiMetrics {
    requests: Counter<u64>,
    request_duration: Histogram<f64>,
}

impl RegistryApiMetrics {
    pub fn new(meter_provider: &AnyMeterProvider) -> Self {
        let meter = meter_provider.meter("registry_api");
        const PREFIX: &str = "docsrs.registry_api";
        Self {
            requests: meter
                .u64_counter(format!("{PREFIX}.requests"))
                .with_description("Completed logical HTTP requests, including retries")
                .with_unit("1")
                .build(),
            request_duration: meter
                .f64_histogram(format!("{PREFIX}.request_duration"))
                .with_description(
                    "Time to response headers or transport failure, including retries",
                )
                .with_boundaries(vec![
                    0.005, 0.01, 0.025, 0.05, 0.075, 0.1, 0.25, 0.5, 0.75, 1.0, 1.5, 2.5, 3.5, 5.0,
                    7.5, 10.0, 15.0, 20.0, 30.0, 45.0, 60.0, 90.0, 120.0,
                ])
                .with_unit("s")
                .build(),
        }
    }

    /// Record one logical request, regardless of the number of middleware retries.
    /// Body consumption and parsing are outside this measurement.
    pub(crate) async fn record_request(
        &self,
        operation: Operation,
        request: impl Future<Output = reqwest_middleware::Result<reqwest::Response>>,
    ) -> reqwest_middleware::Result<reqwest::Response> {
        let start = Instant::now();
        let result = request.await;
        let duration = start.elapsed().as_secs_f64();
        let status = match &result {
            Ok(response) => response.status().as_str().to_owned(),
            Err(_) => "transport_error".to_owned(),
        };
        let operation = KeyValue::new("operation", operation.as_str());
        self.requests
            .add(1, &[operation.clone(), KeyValue::new("status", status)]);
        self.request_duration.record(duration, &[operation]);
        result
    }
}
