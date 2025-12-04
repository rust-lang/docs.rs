pub(crate) mod otel;
pub(crate) mod service;

/// the measured times from cdn invalidations, meaning:
/// * how long an invalidation took, or
/// * how long the invalidation was queued
///
/// will be put into these buckets (seconds,
/// each entry is the upper bound).
/// Prometheus only gets the counts per bucket in a certain
/// time range, no exact durations.
pub const CDN_INVALIDATION_HISTOGRAM_BUCKETS: &[f64; 11] = &[
    60.0,    // 1
    120.0,   // 2
    300.0,   // 5
    600.0,   // 10
    900.0,   // 15
    1200.0,  // 20
    1800.0,  // 30
    2700.0,  // 45
    6000.0,  // 100
    12000.0, // 200
    24000.0, // 400
];

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

/// response time histogram buckets from the opentelemetry semantiv conventions
/// https://opentelemetry.io/docs/specs/semconv/http/http-metrics/#metric-httpserverrequestduration
///
/// These are the default prometheus bucket sizes,
/// https://docs.rs/prometheus/0.14.0/src/prometheus/histogram.rs.html#25-27
/// tailored to broadly measure the response time (in seconds) of a network service.
///
/// Otel default buckets are not suited for that.
pub const RESPONSE_TIME_HISTOGRAM_BUCKETS: &[f64] = &[
    0.005, 0.01, 0.025, 0.05, 0.075, 0.1, 0.25, 0.5, 0.75, 1.0, 2.5, 5.0, 7.5, 10.0,
];
