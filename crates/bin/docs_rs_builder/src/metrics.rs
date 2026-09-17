use docs_rs_opentelemetry::AnyMeterProvider;
use docs_rs_types::ByteSize;
use opentelemetry::{
    KeyValue,
    metrics::{Counter, Histogram},
};
use std::time::Duration;

/// Buckets for documentation size in bytes, doubling from 64 KiB to 32 GiB.
/// Base for some estimates:
/// * `itertools` docs is an 8.2 MB archive with 144 MB of docs
/// * the biggest doc archive know of (`stm32ral`) is an 1.8 GiB archive,
///   which would be an estimated 32 GiB of docs based on the compression
///   ratio above.
/// * we don't know the distribution of these doc sizes yet.
pub const DOCUMENTATION_SIZE_BUCKETS: &[ByteSize; 20] = &[
    ByteSize::kib(64),
    ByteSize::kib(128),
    ByteSize::kib(256),
    ByteSize::kib(512),
    ByteSize::mib(1),
    ByteSize::mib(2),
    ByteSize::mib(4),
    ByteSize::mib(8),
    ByteSize::mib(16),
    ByteSize::mib(32),
    ByteSize::mib(64),
    ByteSize::mib(128),
    ByteSize::mib(256),
    ByteSize::mib(512),
    ByteSize::gib(1),
    ByteSize::gib(2),
    ByteSize::gib(4),
    ByteSize::gib(8),
    ByteSize::gib(16),
    ByteSize::gib(32),
];

/// the measured times of building crates will be put into these buckets
pub const BUILD_TIME_HISTOGRAM_BUCKETS: &[Duration] = &[
    Duration::from_secs(5),
    Duration::from_secs(10),
    Duration::from_secs(15),
    Duration::from_secs(20),
    Duration::from_secs(25),
    Duration::from_secs(30),
    Duration::from_secs(45),
    Duration::from_mins(1),
    Duration::from_secs(90),
    Duration::from_mins(2),
    Duration::from_secs(150),
    Duration::from_mins(3),
    Duration::from_secs(210),
    Duration::from_mins(4),
    Duration::from_secs(270),
    Duration::from_mins(5),
    Duration::from_mins(7),
    Duration::from_mins(10),
    Duration::from_mins(15),
    Duration::from_mins(20),
    Duration::from_mins(30),
    Duration::from_mins(60),
];

#[derive(Debug, Clone, Copy)]
pub enum BuildResult {
    Success,
    Failed,
    Error,
}

impl BuildResult {
    pub fn as_str(&self) -> &'static str {
        match self {
            BuildResult::Success => "success",
            BuildResult::Failed => "failed",
            BuildResult::Error => "error",
        }
    }
}

#[derive(Debug)]
pub struct BuilderMetrics {
    pub total_builds: Counter<u64>,
    build_time: Histogram<f64>,
    pub successful_builds: Counter<u64>,
    pub failed_builds: Counter<u64>,
    pub non_library_builds: Counter<u64>,
    documentation_size: Histogram<u64>,
}

impl BuilderMetrics {
    pub fn new(meter_provider: &AnyMeterProvider) -> Self {
        let meter = meter_provider.meter("builder");
        const PREFIX: &str = "docsrs.builder";
        Self {
            failed_builds: meter
                .u64_counter(format!("{PREFIX}.failed_builds"))
                .with_unit("1")
                .build(),
            build_time: meter
                .f64_histogram(format!("{PREFIX}.build_time"))
                .with_boundaries(
                    BUILD_TIME_HISTOGRAM_BUCKETS
                        .iter()
                        .map(Duration::as_secs_f64)
                        .collect(),
                )
                .with_unit("s")
                .build(),
            total_builds: meter
                .u64_counter(format!("{PREFIX}.total_builds"))
                .with_unit("1")
                .build(),
            successful_builds: meter
                .u64_counter(format!("{PREFIX}.successful_builds"))
                .with_unit("1")
                .build(),
            non_library_builds: meter
                .u64_counter(format!("{PREFIX}.non_library_builds"))
                .with_unit("1")
                .build(),
            documentation_size: meter
                .u64_histogram(format!("{PREFIX}.documentation_size"))
                .with_boundaries(
                    DOCUMENTATION_SIZE_BUCKETS
                        .iter()
                        .map(|size| size.as_u64() as f64)
                        .collect(),
                )
                .with_unit("By")
                .with_description("size of the generated documentation in bytes")
                .build(),
        }
    }

    pub fn record_documentation_size(&self, size: ByteSize) {
        self.documentation_size.record(size.as_u64(), &[])
    }

    pub fn record_build_time(&self, elapsed: Duration, build_result: BuildResult) {
        self.build_time.record(
            elapsed.as_secs_f64(),
            &[KeyValue::new("result", build_result.as_str())],
        );
    }
}
