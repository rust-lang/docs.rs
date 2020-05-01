use super::pool::Pool;
use iron::headers::ContentType;
use iron::prelude::*;
use iron::status::Status;
use prometheus::{
    opts, register_counter, register_int_counter, register_int_gauge, Encoder, IntCounter,
    IntGauge, TextEncoder, __register_gauge, register_int_counter_vec, IntCounterVec,
    __register_counter_vec, histogram_opts, register_histogram_vec, HistogramVec,
};
use std::time::{Duration, Instant};

lazy_static::lazy_static! {
    static ref QUEUED_CRATES_COUNT: IntGauge = register_int_gauge!(
        "docsrs_queued_crates_count",
        "Number of crates in the build queue"
    )
    .unwrap();

    static ref FAILED_CRATES_COUNT: IntGauge = register_int_gauge!(
        "docsrs_failed_crates_count",
        "Number of crates that failed to build"
    )
    .unwrap();

    pub static ref TOTAL_BUILDS: IntCounter = register_int_counter!(
        "docsrs_total_builds",
        "Number of crates built"
    )
    .unwrap();

    pub static ref SUCCESSFUL_BUILDS: IntCounter = register_int_counter!(
        "docsrs_successful_builds",
        "Number of builds that successfully generated docs"
    )
    .unwrap();

    pub static ref FAILED_BUILDS: IntCounter = register_int_counter!(
        "docsrs_failed_builds",
        "Number of builds that generated a compile error"
    )
    .unwrap();

    pub static ref NON_LIBRARY_BUILDS: IntCounter = register_int_counter!(
        "docsrs_non_library_builds",
        "Number of builds that did not complete due to not being a library"
    )
    .unwrap();

    pub static ref UPLOADED_FILES_TOTAL: IntCounter = register_int_counter!(
        "docsrs_uploaded_files_total",
        "Number of files uploaded to S3 or stored in the database"
    )
    .unwrap();

    pub static ref ROUTES_VISITED: IntCounterVec = register_int_counter_vec!(
        "docsrs_routes_visited",
        "The traffic of various docs.rs routes",
        &["route"]
    )
    .unwrap();

    pub static ref RESPONSE_TIMES: HistogramVec = register_histogram_vec!(
        "docsrs_response_time",
        "The response times of various docs.rs routes",
        &["route"]
    )
    .unwrap();
}

pub fn metrics_handler(req: &mut Request) -> IronResult<Response> {
    let conn = extension!(req, Pool).get();

    QUEUED_CRATES_COUNT.set(
        ctry!(conn.query("SELECT COUNT(*) FROM queue WHERE attempt < 5;", &[]))
            .get(0)
            .get(0),
    );
    FAILED_CRATES_COUNT.set(
        ctry!(conn.query("SELECT COUNT(*) FROM queue WHERE attempt >= 5;", &[]))
            .get(0)
            .get(0),
    );

    let mut buffer = Vec::new();
    let families = prometheus::gather();
    ctry!(TextEncoder::new().encode(&families, &mut buffer));

    let mut resp = Response::with(buffer);
    resp.status = Some(Status::Ok);
    resp.headers
        .set(ContentType("text/plain; version=0.0.4".parse().unwrap()));
    Ok(resp)
}

/// Converts a `Duration` to seconds, used by prometheus internally
#[inline]
fn duration_to_seconds(d: Duration) -> f64 {
    let nanos = f64::from(d.subsec_nanos()) / 1e9;
    d.as_secs() as f64 + nanos
}

#[derive(Debug, Clone)]
pub struct ResponseRecorder {
    start_time: Instant,
    route: Option<String>,
}

impl ResponseRecorder {
    #[inline]
    pub fn new() -> Self {
        Self {
            start_time: Instant::now(),
            route: None,
        }
    }

    #[inline]
    pub fn route(&mut self, route: impl Into<String>) {
        self.route = Some(route.into());
    }
}

impl Drop for ResponseRecorder {
    fn drop(&mut self) {
        if let Some(route) = &self.route {
            ROUTES_VISITED.with_label_values(&[route]).inc();

            let response_time = duration_to_seconds(self.start_time.elapsed());
            RESPONSE_TIMES
                .with_label_values(&[route])
                .observe(response_time);
        }
    }
}
