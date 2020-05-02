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

            #[cfg(test)]
            tests::record_tests(route);
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::test::wrapper;
    use std::{
        collections::HashMap,
        sync::{
            atomic::{AtomicUsize, Ordering},
            Mutex,
        },
    };

    static ROUTES_VISITED: AtomicUsize = AtomicUsize::new(0);
    lazy_static::lazy_static! {
        static ref RESPONSE_TIMES: Mutex<HashMap<String, usize>> = Mutex::new(HashMap::new());
    }

    pub fn record_tests(route: &str) {
        ROUTES_VISITED.fetch_add(1, Ordering::SeqCst);

        let mut times = RESPONSE_TIMES.lock().unwrap();
        if let Some(requests) = times.get_mut(route) {
            *requests += 1;
        } else {
            times.insert(route.to_owned(), 1);
        }
    }

    fn reset_records() {
        ROUTES_VISITED.store(0, Ordering::SeqCst);
        RESPONSE_TIMES.lock().unwrap().clear();
    }

    #[test]
    fn home_page() {
        wrapper(|env| {
            let frontend = env.frontend();

            reset_records();

            frontend.get("/").send()?;
            frontend.get("/").send()?;
            assert_eq!(ROUTES_VISITED.load(Ordering::SeqCst), 2);
            assert_eq!(RESPONSE_TIMES.lock().unwrap().get("home (found)"), Some(&2));

            reset_records();

            frontend.get("").send()?;
            frontend.get("").send()?;
            assert_eq!(ROUTES_VISITED.load(Ordering::SeqCst), 2);
            assert_eq!(RESPONSE_TIMES.lock().unwrap().get("home (found)"), Some(&2));

            Ok(())
        })
    }

    #[test]
    fn resources() {
        wrapper(|env| {
            let frontend = env.frontend();

            let routes = [
                "/style.css",
                "/index.js",
                "/menu.js",
                "/sitemap.xml",
                "/opensearch.xml",
            ];

            for route in routes.iter() {
                reset_records();

                frontend.get(route).send()?;
                frontend.get(route).send()?;

                assert_eq!(ROUTES_VISITED.load(Ordering::SeqCst), 2);
                assert_eq!(
                    RESPONSE_TIMES.lock().unwrap().get("resources (found)"),
                    Some(&2)
                );
            }

            reset_records();

            frontend.get("/robots.txt").send()?;
            frontend.get("/robots.txt").send()?;

            assert_eq!(ROUTES_VISITED.load(Ordering::SeqCst), 2);
            assert_eq!(RESPONSE_TIMES.lock().unwrap().get("resources"), Some(&2));

            Ok(())
        })
    }

    #[test]
    fn releases() {
        wrapper(|env| {
            env.db()
                .fake_release()
                .name("rcc")
                .version("0.0.0")
                .repo("https://github.com/jyn514/rcc")
                .create()?;
            env.db()
                .fake_release()
                .name("rcc")
                .version("1.0.0")
                .build_result_successful(false)
                .create()?;

            let frontend = env.frontend();

            let routes = [
                ("/releases", "recent releases (found)"),
                // ("/releases/recent", "recent releases (found)"),
                ("/releases/recent/1", "recent releases (found)"),
                ("/releases/feed", "release feed (found)"),
                ("/releases/queue", "release queue (found)"),
                // ("/releases/search", "search releases (found)"),
                // ("/releases/stars", "release stars (found)"),
                // ("/releases/stars/1", "release stars (found)"),
                // ("/releases/activity", "release activity (found)"),
                // ("/releases/failures", "build failures (found)"),
                // ("/releases/failures/1", "build failures (found)"),
                ("/releases/recent-failures", "recent failures (found)"),
                ("/releases/recent-failures/1", "recent failures (found)"),
            ];

            for (route, correct) in routes.iter() {
                reset_records();

                frontend.get(route).send()?;
                frontend.get(route).send()?;

                assert_eq!(ROUTES_VISITED.load(Ordering::SeqCst), 2);
                assert_eq!(RESPONSE_TIMES.lock().unwrap().get(*correct), Some(&2));
            }

            Ok(())
        })
    }

    #[test]
    fn crates() {
        wrapper(|env| {
            env.db()
                .fake_release()
                .name("rcc")
                .version("0.0.0")
                .create()?;
            env.db()
                .fake_release()
                .name("hexponent")
                .version("0.2.0")
                .create()?;

            let frontend = env.frontend();

            let routes = [
                ("/crate/rcc", "crate details (found)"),
                ("/crate/rcc/", "crate details (found)"),
                ("/crate/hexponent", "crate details (found)"),
                ("/crate/hexponent/", "crate details (found)"),
                ("/crate/rcc/0.0.0", "crate details (found)"),
                ("/crate/rcc/0.0.0/", "crate details (found)"),
                ("/crate/hexponent/0.2.0", "crate details (found)"),
                ("/crate/hexponent/0.2.0/", "crate details (found)"),
                ("/crate/i_dont_exist", "crate details (404)"),
                ("/crate/i_dont_exist/", "crate details (404)"),
                ("/crate/i_dont_exist/4.0.4", "crate details (404)"),
                ("/crate/i_dont_exist/4.0.4/", "crate details (404)"),
            ];

            for (route, correct) in routes.iter() {
                reset_records();

                frontend.get(route).send()?;
                frontend.get(route).send()?;

                assert_eq!(ROUTES_VISITED.load(Ordering::SeqCst), 2);
                assert_eq!(RESPONSE_TIMES.lock().unwrap().get(*correct), Some(&2));
            }

            Ok(())
        })
    }
}
