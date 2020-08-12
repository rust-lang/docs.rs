use crate::db::Pool;
use crate::BuildQueue;
use crate::Metrics;
use iron::headers::ContentType;
use iron::prelude::*;
use iron::status::Status;
use prometheus::{Encoder, HistogramVec, TextEncoder};
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
#[allow(dead_code)]
pub static OPEN_FILE_DESCRIPTORS: Lazy<IntGauge> = Lazy::new(|| {
    register_int_gauge!(
        "docsrs_open_file_descriptors",
        "The number of currently opened file descriptors"
    )
    .unwrap()
});

#[cfg(not(windows))]
#[allow(dead_code)]
pub static CURRENTLY_RUNNING_THREADS: Lazy<IntGauge> = Lazy::new(|| {
    register_int_gauge!(
        "docsrs_running_threads",
        "The number of threads being used by docs.rs"
    )
    .unwrap()
});

pub static HTML_REWRITE_OOMS: Lazy<IntGauge> = Lazy::new(|| {
    register_int_gauge!(
        "docsrs_html_rewrite_ooms",
        "The number of attempted files that failed due to a memory limit"
    )
    .unwrap()
});

pub fn metrics_handler(req: &mut Request) -> IronResult<Response> {
    let metrics = extension!(req, Metrics);
    let pool = extension!(req, Pool);
    let queue = extension!(req, BuildQueue);

    let mut buffer = Vec::new();
    let families = ctry!(req, metrics.gather(pool, &*queue));
    ctry!(req, TextEncoder::new().encode(&families, &mut buffer));

    let mut resp = Response::with(buffer);
    resp.status = Some(Status::Ok);
    resp.headers.set(ContentType::plaintext());

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

        let metrics = extension!(request, Metrics);
        metrics
            .routes_visited
            .with_label_values(&[&self.route_name])
            .inc();
        metrics
            .response_time
            .with_label_values(&[&self.route_name])
            .observe(resp_time);

        result
    }
}

struct RenderingTime {
    start: Instant,
    step: &'static str,
}

pub(crate) struct RenderingTimesRecorder<'a> {
    metric: &'a HistogramVec,
    current: Option<RenderingTime>,
}

impl<'a> RenderingTimesRecorder<'a> {
    pub(crate) fn new(metric: &'a HistogramVec) -> Self {
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

impl Drop for RenderingTimesRecorder<'_> {
    fn drop(&mut self) {
        self.record_current();
    }
}

#[cfg(test)]
mod tests {
    use crate::test::{assert_success, wrapper};
    use std::collections::HashMap;

    #[test]
    fn test_response_times_count_being_collected() {
        const ROUTES: &[(&str, &str)] = &[
            ("", "/"),
            ("/", "/"),
            ("/crate/hexponent/0.2.0", "/crate/:name/:version"),
            ("/crate/rcc/0.0.0", "/crate/:name/:version"),
            ("/index.js", "static resource"),
            ("/menu.js", "static resource"),
            ("/opensearch.xml", "static resource"),
            ("/releases", "/releases"),
            ("/releases/feed", "static resource"),
            ("/releases/queue", "/releases/queue"),
            ("/releases/recent-failures", "/releases/recent-failures"),
            (
                "/releases/recent-failures/1",
                "/releases/recent-failures/:page",
            ),
            ("/releases/recent/1", "/releases/recent/:page"),
            ("/robots.txt", "static resource"),
            ("/sitemap.xml", "static resource"),
            ("/style.css", "static resource"),
        ];

        wrapper(|env| {
            env.fake_release()
                .name("rcc")
                .version("0.0.0")
                .repo("https://github.com/jyn514/rcc")
                .create()?;
            env.fake_release()
                .name("rcc")
                .version("1.0.0")
                .build_result_successful(false)
                .create()?;
            env.fake_release()
                .name("hexponent")
                .version("0.2.0")
                .create()?;

            let frontend = env.frontend();
            let metrics = env.metrics();

            for (route, _) in ROUTES.iter() {
                frontend.get(route).send()?;
                frontend.get(route).send()?;
            }

            let mut expected = HashMap::new();
            for (_, correct) in ROUTES.iter() {
                let entry = expected.entry(*correct).or_insert(0);
                *entry += 2;
            }

            for (label, count) in expected.iter() {
                assert_eq!(
                    metrics.routes_visited.with_label_values(&[*label]).get(),
                    *count
                );
                assert_eq!(
                    metrics
                        .response_time
                        .with_label_values(&[*label])
                        .get_sample_count(),
                    *count as u64
                );
            }

            Ok(())
        })
    }

    #[test]
    fn test_metrics_page_success() {
        wrapper(|env| {
            let web = env.frontend();
            assert_success("/about/metrics", web)
        })
    }
}
