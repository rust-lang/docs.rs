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
pub const DOCUMENTATION_SIZE_BUCKETS: &[ByteSize; 20] = &{
    let mut buckets = [ByteSize::kib(64); 20];
    let mut i = 1;
    while i < buckets.len() {
        buckets[i] = ByteSize::b(buckets[i - 1].as_u64() * 2);
        i += 1;
    }
    buckets
};

/// Build-time buckets with finer resolution for shorter builds.
pub const BUILD_TIME_HISTOGRAM_BUCKETS: &[Duration; 50] = &{
    let mut buckets = [Duration::ZERO; 50];
    let mut i = 0;

    // 5 seconds through 2 minutes, in 5-second steps (24 boundaries).
    let mut seconds = 5;
    while seconds <= 120 {
        buckets[i] = Duration::from_secs(seconds);
        seconds += 5;
        i += 1;
    }

    // Above 2 minutes through 10 minutes, in 30-second steps (16 boundaries).
    let mut seconds = 150;
    while seconds <= 600 {
        buckets[i] = Duration::from_secs(seconds);
        seconds += 30;
        i += 1;
    }

    // Above 10 minutes through 60 minutes, in 5-minute steps (10 boundaries).
    let mut minutes = 15;
    while minutes <= 60 {
        buckets[i] = Duration::from_mins(minutes);
        minutes += 5;
        i += 1;
    }

    buckets
};

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
