use crate::db::Pool;
use crate::BuildQueue;
use iron::headers::ContentType;
use iron::prelude::*;
use iron::status::Status;
use once_cell::sync::Lazy;
use prometheus::{
    opts, register_counter, register_int_counter, register_int_gauge, Encoder, IntCounter,
    IntGauge, TextEncoder, __register_gauge, register_int_counter_vec, IntCounterVec,
    __register_counter_vec, histogram_opts, register_histogram_vec, HistogramVec,
};
use std::time::{Duration, Instant};

static QUEUED_CRATES_COUNT: Lazy<IntGauge> = Lazy::new(|| {
    register_int_gauge!(
        "docsrs_queued_crates_count",
        "Number of crates in the build queue"
    )
    .unwrap()
});

pub static PRIORITIZED_CRATES_COUNT: Lazy<IntGauge> = Lazy::new(|| {
    register_int_gauge!(
        "docsrs_prioritized_crates_count",
        "Number of crates in the build queue that have a positive priority"
    )
    .unwrap()
});

static FAILED_CRATES_COUNT: Lazy<IntGauge> = Lazy::new(|| {
    register_int_gauge!(
        "docsrs_failed_crates_count",
        "Number of crates that failed to build"
    )
    .unwrap()
});

pub static TOTAL_BUILDS: Lazy<IntCounter> =
    Lazy::new(|| register_int_counter!("docsrs_total_builds", "Number of crates built").unwrap());

pub static SUCCESSFUL_BUILDS: Lazy<IntCounter> = Lazy::new(|| {
    register_int_counter!(
        "docsrs_successful_builds",
        "Number of builds that successfully generated docs"
    )
    .unwrap()
});

pub static FAILED_BUILDS: Lazy<IntCounter> = Lazy::new(|| {
    register_int_counter!(
        "docsrs_failed_builds",
        "Number of builds that generated a compile error"
    )
    .unwrap()
});

pub static NON_LIBRARY_BUILDS: Lazy<IntCounter> = Lazy::new(|| {
    register_int_counter!(
        "docsrs_non_library_builds",
        "Number of builds that did not complete due to not being a library"
    )
    .unwrap()
});

pub static UPLOADED_FILES_TOTAL: Lazy<IntCounter> = Lazy::new(|| {
    register_int_counter!(
        "docsrs_uploaded_files_total",
        "Number of files uploaded to S3 or stored in the database"
    )
    .unwrap()
});

pub static ROUTES_VISITED: Lazy<IntCounterVec> = Lazy::new(|| {
    register_int_counter_vec!(
        "docsrs_routes_visited",
        "The traffic of various docs.rs routes",
        &["route"]
    )
    .unwrap()
});

pub static RESPONSE_TIMES: Lazy<HistogramVec> = Lazy::new(|| {
    register_histogram_vec!(
        "docsrs_response_time",
        "The response times of various docs.rs routes",
        &["route"]
    )
    .unwrap()
});

pub static RUSTDOC_RENDERING_TIMES: Lazy<HistogramVec> = Lazy::new(|| {
    register_histogram_vec!(
        "docsrs_rustdoc_rendering_time",
        "The time it takes to render a rustdoc page",
        &["step"]
    )
    .unwrap()
});

pub static FAILED_DB_CONNECTIONS: Lazy<IntCounter> = Lazy::new(|| {
    register_int_counter!(
        "docsrs_failed_db_connections",
        "Number of attempted and failed connections to the database"
    )
    .unwrap()
});

pub static USED_DB_CONNECTIONS: Lazy<IntGauge> = Lazy::new(|| {
    register_int_gauge!(
        "docsrs_used_db_connections",
        "The number of used database connections"
    )
    .unwrap()
});

pub static IDLE_DB_CONNECTIONS: Lazy<IntGauge> = Lazy::new(|| {
    register_int_gauge!(
        "docsrs_idle_db_connections",
        "The number of idle database connections"
    )
    .unwrap()
});

pub static MAX_DB_CONNECTIONS: Lazy<IntGauge> = Lazy::new(|| {
    register_int_gauge!(
        "docsrs_max_db_connections",
        "The maximum database connections"
    )
    .unwrap()
});

#[cfg(not(windows))]
pub static OPEN_FILE_DESCRIPTORS: Lazy<IntGauge> = Lazy::new(|| {
    register_int_gauge!(
        "docsrs_open_file_descriptors",
        "The number of currently opened file descriptors"
    )
    .unwrap()
});

#[cfg(not(windows))]
pub static CURRENTLY_RUNNING_THREADS: Lazy<IntGauge> = Lazy::new(|| {
    register_int_gauge!(
        "docsrs_running_threads",
        "The number of threads being used by docs.rs"
    )
    .unwrap()
});

pub fn metrics_handler(req: &mut Request) -> IronResult<Response> {
    let pool = extension!(req, Pool);
    let queue = extension!(req, BuildQueue);

    USED_DB_CONNECTIONS.set(pool.used_connections() as i64);
    IDLE_DB_CONNECTIONS.set(pool.idle_connections() as i64);

    QUEUED_CRATES_COUNT.set(ctry!(queue.pending_count()) as i64);
    PRIORITIZED_CRATES_COUNT.set(ctry!(queue.prioritized_count()) as i64);
    FAILED_CRATES_COUNT.set(ctry!(queue.failed_count()) as i64);

    #[cfg(target_os = "linux")]
    {
        use procfs::process::Process;

        let process = Process::myself().unwrap();
        OPEN_FILE_DESCRIPTORS.set(process.fd().unwrap().len() as i64);
        CURRENTLY_RUNNING_THREADS.set(process.stat().unwrap().num_threads as i64);
    }

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

pub struct RequestRecorder {
    handler: Box<dyn iron::Handler>,
    route_name: String,
}

impl RequestRecorder {
    pub fn new(handler: impl iron::Handler, route: impl Into<String>) -> Self {
        Self {
            handler: Box::new(handler),
            route_name: route.into(),
        }
    }
}

impl iron::Handler for RequestRecorder {
    fn handle(&self, request: &mut Request) -> IronResult<Response> {
        let start = Instant::now();
        let result = self.handler.handle(request);
        let resp_time = duration_to_seconds(start.elapsed());

        ROUTES_VISITED.with_label_values(&[&self.route_name]).inc();
        RESPONSE_TIMES
            .with_label_values(&[&self.route_name])
            .observe(resp_time);

        #[cfg(test)]
        tests::record_tests(&self.route_name);

        result
    }
}

struct RenderingTime {
    start: Instant,
    step: &'static str,
}

pub(crate) struct RenderingTimesRecorder {
    metric: &'static HistogramVec,
    current: Option<RenderingTime>,
}

impl RenderingTimesRecorder {
    pub(crate) fn new(metric: &'static HistogramVec) -> Self {
        Self {
            metric,
            current: None,
        }
    }

    pub(crate) fn step(&mut self, step: &'static str) {
        self.record_current();
        self.current = Some(RenderingTime {
            start: Instant::now(),
            step,
        });
    }

    fn record_current(&mut self) {
        if let Some(current) = self.current.take() {
            self.metric
                .with_label_values(&[current.step])
                .observe(duration_to_seconds(current.start.elapsed()));
        }
    }
}

impl Drop for RenderingTimesRecorder {
    fn drop(&mut self) {
        self.record_current();
    }
}

#[cfg(test)]
mod tests {
    use crate::test::{assert_success, wrapper};
    use once_cell::sync::Lazy;
    use std::{
        collections::HashMap,
        sync::{
            atomic::{AtomicUsize, Ordering},
            Mutex,
        },
    };

    static ROUTES_VISITED: AtomicUsize = AtomicUsize::new(0);
    static RESPONSE_TIMES: Lazy<Mutex<HashMap<String, usize>>> =
        Lazy::new(|| Mutex::new(HashMap::new()));

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
            assert_eq!(RESPONSE_TIMES.lock().unwrap().get("/"), Some(&2));

            reset_records();

            frontend.get("").send()?;
            frontend.get("").send()?;
            assert_eq!(ROUTES_VISITED.load(Ordering::SeqCst), 2);
            assert_eq!(RESPONSE_TIMES.lock().unwrap().get("/"), Some(&2));

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
                "/robots.txt",
            ];

            for route in routes.iter() {
                reset_records();

                frontend.get(route).send()?;
                frontend.get(route).send()?;

                assert_eq!(ROUTES_VISITED.load(Ordering::SeqCst), 2);
                assert_eq!(
                    RESPONSE_TIMES.lock().unwrap().get("static resource"),
                    Some(&2)
                );
            }

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
                ("/releases", "/releases"),
                ("/releases/recent/1", "/releases/recent/:page"),
                ("/releases/feed", "static resource"),
                ("/releases/queue", "/releases/queue"),
                ("/releases/recent-failures", "/releases/recent-failures"),
                (
                    "/releases/recent-failures/1",
                    "/releases/recent-failures/:page",
                ),
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

            let routes = ["/crate/rcc/0.0.0", "/crate/hexponent/0.2.0"];

            for route in routes.iter() {
                reset_records();

                frontend.get(route).send()?;
                frontend.get(route).send()?;

                assert_eq!(ROUTES_VISITED.load(Ordering::SeqCst), 2);
                assert_eq!(
                    RESPONSE_TIMES.lock().unwrap().get("/crate/:name/:version"),
                    Some(&2)
                );
            }

            Ok(())
        })
    }

    #[test]
    fn metrics() {
        wrapper(|env| {
            let web = env.frontend();
            assert_success("/about/metrics", web)
        })
    }
}
