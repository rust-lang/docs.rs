use docs_rs_opentelemetry::AnyMeterProvider;
use opentelemetry::metrics::{Counter, Histogram};

/// buckets for documentation size, in MiB
/// Base for some estimates:
/// * `itertools` docs is an 8.2 MB archive with 144 MB of docs
/// * the biggest doc archive know of (`stm32ral`) is an 1.8 GiB archive,
///   which would be an estimated 32 GiB of docs based on the compression
///   ratio above.
/// * we don't know the distribution of these doc sizes yet.
pub const DOCUMENTATION_SIZE_BUCKETS: &[f64; 16] = &[
    1.0, 2.0, 4.0, 8.0, 16.0, 32.0, 64.0, 128.0, 256.0, 512.0, 1024.0, 2048.0, 4096.0, 8192.0,
    16384.0, 32768.0,
];

/// the measured times of building crates will be put into these buckets
pub const BUILD_TIME_HISTOGRAM_BUCKETS: &[f64] = &[
    5.0,    // 5s
    10.0,   // 10s
    15.0,   // 15s
    20.0,   // 20s
    25.0,   // 25s
    30.0,   // 30s
    45.0,   // 45s
    60.0,   // 1 min
    90.0,   // 1.5 min
    120.0,  // 2 min
    150.0,  // 2.5 min
    180.0,  // 3 min
    210.0,  // 3.5 min
    240.0,  // 4 min
    270.0,  // 4.5 min
    300.0,  // 5 min
    420.0,  // 7 min
    600.0,  // 10 min
    900.0,  // 15 min
    1200.0, // 20 min
    1800.0, // 30 min
    3600.0, // 60 min
];

#[derive(Debug)]
pub struct BuilderMetrics {
    pub total_builds: Counter<u64>,
    pub build_time: Histogram<f64>,
    pub successful_builds: Counter<u64>,
    pub failed_builds: Counter<u64>,
    pub non_library_builds: Counter<u64>,
    pub documentation_size: Histogram<u64>,
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
                .with_boundaries(BUILD_TIME_HISTOGRAM_BUCKETS.to_vec())
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
                .with_boundaries(DOCUMENTATION_SIZE_BUCKETS.to_vec())
                .with_unit("bytes")
                .with_description("size of the generated documentation in bytes")
                .build(),
        }
    }
}
