use super::pool::Pool;
use iron::headers::ContentType;
use iron::prelude::*;
use iron::status::Status;
use prometheus::{Encoder, IntGauge, TextEncoder};

lazy_static! {
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
}

pub fn metrics_handler(req: &mut Request<'_, '_>) -> IronResult<Response> {
    let conn = extension!(req, Pool);

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
