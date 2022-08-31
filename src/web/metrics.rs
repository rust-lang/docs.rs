use crate::db::Pool;
use crate::BuildQueue;
use crate::Metrics;
use iron::headers::ContentType;
use iron::prelude::*;
use iron::status::Status;
use prometheus::{Encoder, HistogramVec, TextEncoder};
use std::time::{Duration, Instant};

pub(super) fn metrics_handler(req: &mut Request) -> IronResult<Response> {
    let metrics = extension!(req, Metrics);
    let pool = extension!(req, Pool);
    let queue = extension!(req, BuildQueue);

    let mut buffer = Vec::new();
    let families = ctry!(req, metrics.gather(pool, queue));
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

pub(super) struct RequestRecorder {
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
            #[cfg(test)]
            log::debug!(
                "rendering time - {}: {:?}",
                current.step,
                current.start.elapsed()
            );
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
    use crate::Context;
    use std::collections::HashMap;

    #[test]
    fn test_response_times_count_being_collected() {
        const ROUTES: &[(&str, &str)] = &[
            ("", "/"),
            ("/", "/"),
            ("/crate/hexponent/0.2.0", "/crate/:name/:version"),
            ("/crate/rcc/0.0.0", "/crate/:name/:version"),
            ("/-/static/index.js", "static resource"),
            ("/-/static/menu.js", "static resource"),
            ("/-/static/keyboard.js", "static resource"),
            ("/-/static/source.js", "static resource"),
            ("/-/static/opensearch.xml", "static resource"),
            ("/releases", "/releases"),
            ("/releases/feed", "static resource"),
            ("/releases/queue", "/releases/queue"),
            ("/releases/recent-failures", "/releases/recent-failures"),
            (
                "/releases/recent-failures/1",
                "/releases/recent-failures/:page",
            ),
            ("/releases/recent/1", "/releases/recent/:page"),
            ("/-/static/robots.txt", "static resource"),
            ("/sitemap.xml", "/sitemap.xml"),
            ("/-/sitemap/a/sitemap.xml", "/-/sitemap/:letter/sitemap.xml"),
            ("/-/static/style.css", "static resource"),
            ("/-/static/vendored.css", "static resource"),
            ("/rustdoc/rcc/0.0.0/rcc/index.html", "rustdoc page"),
            ("/rustdoc/gcc/0.0.0/gcc/index.html", "rustdoc page"),
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
                .build_result_failed()
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

            // this shows what the routes were *actually* recorded as, making it easier to update ROUTES if the name changes.
            let metrics_serialized = metrics.gather(&env.pool()?, &env.build_queue())?;
            let all_routes_visited = metrics_serialized
                .iter()
                .find(|x| x.get_name() == "docsrs_routes_visited")
                .unwrap();
            let routes_visited_pretty: Vec<_> = all_routes_visited
                .get_metric()
                .iter()
                .map(|metric| {
                    let labels = metric.get_label();
                    assert_eq!(labels.len(), 1); // not sure when this would be false
                    let route = labels[0].get_value();
                    let count = metric.get_counter().get_value();
                    format!("{}: {}", route, count)
                })
                .collect();
            println!("routes: {:?}", routes_visited_pretty);

            for (label, count) in expected.iter() {
                assert_eq!(
                    metrics.routes_visited.with_label_values(&[*label]).get(),
                    *count,
                    "routes_visited metrics for {} are incorrect",
                    label,
                );
                assert_eq!(
                    metrics
                        .response_time
                        .with_label_values(&[*label])
                        .get_sample_count(),
                    *count as u64,
                    "response_time metrics for {} are incorrect",
                    label,
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
