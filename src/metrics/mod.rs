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
