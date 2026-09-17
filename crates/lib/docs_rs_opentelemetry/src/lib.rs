mod config;
#[cfg(any(test, feature = "testing"))]
pub mod testing;
pub use config::Config;

use anyhow::Result;
use opentelemetry::{
    InstrumentationScope,
    metrics::{InstrumentProvider, Meter, MeterProvider},
};
use opentelemetry_otlp::{Protocol, WithExportConfig as _};
use opentelemetry_resource_detectors::{OsResourceDetector, ProcessResourceDetector};
use opentelemetry_sdk::{Resource, error::OTelSdkResult};
use std::{sync::Arc, time::Duration};
use tracing::info;

/// response time histogram buckets from the opentelemetry semantiv conventions
/// https://opentelemetry.io/docs/specs/semconv/http/http-metrics/#metric-httpserverrequestduration
///
/// These are the default prometheus bucket sizes,
/// https://docs.rs/prometheus/0.14.0/src/prometheus/histogram.rs.html#25-27
/// tailored to broadly measure the response time (in seconds) of a network service.
///
/// Otel default buckets are not suited for that.
pub const RESPONSE_TIME_HISTOGRAM_BUCKETS: &[Duration] = &[
    Duration::from_millis(5),
    Duration::from_millis(10),
    Duration::from_millis(25),
    Duration::from_millis(50),
    Duration::from_millis(75),
    Duration::from_millis(100),
    Duration::from_millis(250),
    Duration::from_millis(500),
    Duration::from_millis(750),
    Duration::from_secs(1),
    Duration::from_millis(1500),
    Duration::from_millis(2500),
    Duration::from_millis(3500),
    Duration::from_secs(5),
    Duration::from_millis(7500),
    Duration::from_secs(10),
    Duration::from_secs(15),
    Duration::from_secs(20),
    Duration::from_secs(30),
    Duration::from_secs(45),
    Duration::from_mins(1),
    Duration::from_secs(90),
    Duration::from_mins(2),
];

/// extend the `MeterProvider` trait so we also expose
/// the `force_flush` method for tests.
pub trait MeterProviderWithExt: MeterProvider {
    fn force_flush(&self) -> OTelSdkResult;
}

pub type AnyMeterProvider = Arc<dyn MeterProviderWithExt + Send + Sync>;

impl MeterProviderWithExt for opentelemetry_sdk::metrics::SdkMeterProvider {
    fn force_flush(&self) -> OTelSdkResult {
        self.force_flush()
    }
}

/// opentelemetry metric provider setup,
/// if no endpoint is configured, use a no-op provider
pub fn get_meter_provider(config: &config::Config) -> Result<AnyMeterProvider> {
    if let Some(ref endpoint) = config.endpoint {
        let endpoint = endpoint.to_string();
        info!(endpoint, "setting up OpenTelemetry metrics exporter");

        let exporter = opentelemetry_otlp::MetricExporter::builder()
            .with_tonic()
            .with_endpoint(endpoint.to_string())
            .with_protocol(Protocol::Grpc)
            .with_timeout(Duration::from_secs(3))
            .with_temporality(opentelemetry_sdk::metrics::Temporality::Delta)
            .build()?;

        let provider = opentelemetry_sdk::metrics::SdkMeterProvider::builder()
            .with_periodic_exporter(exporter)
            .with_resource(
                Resource::builder()
                    .with_detector(Box::new(OsResourceDetector))
                    .with_detector(Box::new(ProcessResourceDetector))
                    .build(),
            )
            .build();

        Ok(Arc::new(provider))
    } else {
        Ok(Arc::new(NoopMeterProvider::new()))
    }
}

/// A no-op instance of a `MetricProvider`, so we can avoid conditional
/// logic across the whole codebase.
///
/// For now, copy/paste from opentelemetry-sdk, see
/// https://github.com/open-telemetry/opentelemetry-rust/pull/3111
#[derive(Debug, Default)]
pub struct NoopMeterProvider {
    _private: (),
}

impl NoopMeterProvider {
    /// Create a new no-op meter provider.
    pub fn new() -> Self {
        NoopMeterProvider { _private: () }
    }
}

impl MeterProvider for NoopMeterProvider {
    fn meter_with_scope(&self, _scope: InstrumentationScope) -> Meter {
        Meter::new(Arc::new(NoopMeter::new()))
    }
}

impl MeterProviderWithExt for NoopMeterProvider {
    fn force_flush(&self) -> OTelSdkResult {
        Ok(())
    }
}

/// A no-op instance of a `Meter`
#[derive(Debug, Default)]
pub struct NoopMeter {
    _private: (),
}

impl NoopMeter {
    /// Create a new no-op meter core.
    pub fn new() -> Self {
        NoopMeter { _private: () }
    }
}

impl InstrumentProvider for NoopMeter {}
