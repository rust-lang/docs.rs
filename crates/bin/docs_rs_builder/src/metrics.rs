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
    30.0,   // 0.5
    60.0,   // 1
    120.0,  // 2
    180.0,  // 3
    240.0,  // 4
    300.0,  // 5
    360.0,  // 6
    420.0,  // 7
    480.0,  // 8
    540.0,  // 9
    600.0,  // 10
    660.0,  // 11
    720.0,  // 12
    780.0,  // 13
    840.0,  // 14
    900.0,  // 15
    1200.0, // 20
    1800.0, // 30
    2400.0, // 40
    3000.0, // 50
    3600.0, // 60
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
