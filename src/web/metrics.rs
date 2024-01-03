use crate::{
    db::Pool, metrics::duration_to_seconds, utils::spawn_blocking, web::error::AxumResult,
    BuildQueue, Config, InstanceMetrics, ServiceMetrics,
};
use anyhow::{Context as _, Result};
use axum::{
    extract::{Extension, MatchedPath, Request as AxumRequest},
    http::{header::CONTENT_TYPE, StatusCode},
    middleware::Next,
    response::IntoResponse,
};
use prometheus::{proto::MetricFamily, Encoder, HistogramVec, TextEncoder};
use std::{borrow::Cow, sync::Arc, time::Instant};
#[cfg(test)]
use tracing::debug;

async fn fetch_and_render_metrics(
    fetch_metrics: impl Fn() -> Result<Vec<MetricFamily>> + Send + 'static,
) -> AxumResult<impl IntoResponse> {
    let buffer = spawn_blocking(move || {
        let metrics_families = fetch_metrics()?;
        let mut buffer = Vec::new();
        TextEncoder::new()
            .encode(&metrics_families, &mut buffer)
            .context("error encoding metrics")?;
        Ok(buffer)
    })
    .await?;

    Ok((
        StatusCode::OK,
        [(CONTENT_TYPE, mime::TEXT_PLAIN.as_ref())],
        buffer,
    ))
}

pub(super) async fn metrics_handler(
    Extension(pool): Extension<Pool>,
    Extension(config): Extension<Arc<Config>>,
    Extension(instance_metrics): Extension<Arc<InstanceMetrics>>,
    Extension(service_metrics): Extension<Arc<ServiceMetrics>>,
    Extension(queue): Extension<Arc<BuildQueue>>,
) -> AxumResult<impl IntoResponse> {
    fetch_and_render_metrics(move || {
        let mut families = Vec::new();
        families.extend_from_slice(&instance_metrics.gather(&pool)?);
        families.extend_from_slice(&service_metrics.gather(&pool, &queue, &config)?);
        Ok(families)
    })
    .await
}

pub(super) async fn service_metrics_handler(
    Extension(pool): Extension<Pool>,
    Extension(config): Extension<Arc<Config>>,
    Extension(metrics): Extension<Arc<ServiceMetrics>>,
    Extension(queue): Extension<Arc<BuildQueue>>,
) -> AxumResult<impl IntoResponse> {
    fetch_and_render_metrics(move || metrics.gather(&pool, &queue, &config)).await
}

pub(super) async fn instance_metrics_handler(
    Extension(pool): Extension<Pool>,
    Extension(metrics): Extension<Arc<InstanceMetrics>>,
) -> AxumResult<impl IntoResponse> {
    fetch_and_render_metrics(move || metrics.gather(&pool)).await
}

/// Request recorder middleware
///
/// Looks similar, but *is not* a usable middleware / layer
/// since we need the route-name.
///
/// Can be used like:
/// ```ignore
/// get(handler).route_layer(middleware::from_fn(|request, next| async {
///     request_recorder(request, next, Some("static resource")).await
/// }))
/// ```
pub(crate) async fn request_recorder(
    request: AxumRequest,
    next: Next,
    route_name: Option<&str>,
) -> impl IntoResponse {
    let route_name = if let Some(rn) = route_name {
        Cow::Borrowed(rn)
    } else if let Some(path) = request.extensions().get::<MatchedPath>() {
        Cow::Owned(path.as_str().to_string())
    } else {
        Cow::Owned(request.uri().path().to_string())
    };

    let metrics = request
        .extensions()
        .get::<Arc<InstanceMetrics>>()
        .expect("metrics missing in request extensions")
        .clone();

    let start = Instant::now();
    let result = next.run(request).await;
    let resp_time = duration_to_seconds(start.elapsed());

    metrics
        .routes_visited
        .with_label_values(&[&route_name])
        .inc();
    metrics
        .response_time
        .with_label_values(&[&route_name])
        .observe(resp_time);

    result
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
            debug!(
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
    use crate::test::wrapper;
    use crate::Context;
    use std::collections::HashMap;

    #[test]
    fn test_response_times_count_being_collected() {
        const ROUTES: &[(&str, &str)] = &[
            ("", "/"),
            ("/", "/"),
            ("/crate/hexponent/0.2.0", "/crate/:name/:version"),
            ("/crate/rcc/0.0.0", "/crate/:name/:version"),
            (
                "/crate/rcc/0.0.0/builds.json",
                "/crate/:name/:version/builds.json",
            ),
            (
                "/crate/rcc/0.0.0/status.json",
                "/crate/:name/:version/status.json",
            ),
            ("/-/static/index.js", "static resource"),
            ("/-/static/menu.js", "static resource"),
            ("/-/static/keyboard.js", "static resource"),
            ("/-/static/source.js", "static resource"),
            ("/-/static/opensearch.xml", "static resource"),
            ("/releases", "/releases"),
            ("/releases/feed", "/releases/feed"),
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
            let metrics = env.instance_metrics();

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
            let metrics_serialized = metrics.gather(&env.pool()?)?;
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
                    format!("{route}: {count}")
                })
                .collect();
            println!("routes: {routes_visited_pretty:?}");

            for (label, count) in expected.iter() {
                assert_eq!(
                    metrics.routes_visited.with_label_values(&[*label]).get(),
                    *count,
                    "routes_visited metrics for {label} are incorrect",
                );
                assert_eq!(
                    metrics
                        .response_time
                        .with_label_values(&[*label])
                        .get_sample_count(),
                    *count,
                    "response_time metrics for {label} are incorrect",
                );
            }

            Ok(())
        })
    }

    #[test]
    fn test_metrics_page_success() {
        wrapper(|env| {
            let response = env.frontend().get("/about/metrics").send()?;
            assert!(response.status().is_success());

            let body = response.text()?;
            assert!(body.contains("docsrs_failed_builds"), "{}", body);
            assert!(body.contains("queued_crates_count"), "{}", body);
            Ok(())
        })
    }

    #[test]
    fn test_service_metrics_page_success() {
        wrapper(|env| {
            let response = env.frontend().get("/about/metrics/service").send()?;
            assert!(response.status().is_success());

            let body = response.text()?;
            assert!(!body.contains("docsrs_failed_builds"), "{}", body);
            assert!(body.contains("queued_crates_count"), "{}", body);
            Ok(())
        })
    }

    #[test]
    fn test_instance_metrics_page_success() {
        wrapper(|env| {
            let response = env.frontend().get("/about/metrics/instance").send()?;
            assert!(response.status().is_success());

            let body = response.text()?;
            assert!(body.contains("docsrs_failed_builds"), "{}", body);
            assert!(!body.contains("queued_crates_count"), "{}", body);
            Ok(())
        })
    }
}
