use crate::{
    AsyncBuildQueue, Config, InstanceMetrics, ServiceMetrics, db::Pool,
    metrics::otel::AnyMeterProvider, web::error::AxumResult,
};
use anyhow::{Context as _, Result};
use axum::{
    extract::{Extension, MatchedPath, Request as AxumRequest},
    http::{StatusCode, header::CONTENT_TYPE},
    middleware::Next,
    response::IntoResponse,
};
use opentelemetry::{
    KeyValue,
    metrics::{Counter, Histogram},
};
use prometheus::{Encoder, TextEncoder, proto::MetricFamily};
use std::{borrow::Cow, future::Future, sync::Arc, time::Instant};

#[derive(Debug)]
pub(crate) struct WebMetrics {
    pub(crate) html_rewrite_ooms: Counter<u64>,
    pub(crate) im_feeling_lucky_searches: Counter<u64>,

    routes_visited: Counter<u64>,
    response_time: Histogram<f64>,
}

impl WebMetrics {
    pub(crate) fn new(meter_provider: &AnyMeterProvider) -> Self {
        let meter = meter_provider.meter("web");
        const PREFIX: &str = "docsrs.web";
        Self {
            html_rewrite_ooms: meter
                .u64_counter(format!("{PREFIX}.html_rewrite_ooms"))
                .with_unit("1")
                .build(),
            im_feeling_lucky_searches: meter
                .u64_counter(format!("{PREFIX}.im_feeling_lucky_searches"))
                .with_unit("1")
                .build(),
            routes_visited: meter
                .u64_counter(format!("{PREFIX}.routes_visited"))
                .with_unit("1")
                .build(),
            response_time: meter
                .f64_histogram(format!("{PREFIX}.response_time"))
                .with_unit("s")
                .build(),
        }
    }
}

async fn fetch_and_render_metrics<Fut>(fetch_metrics: Fut) -> AxumResult<impl IntoResponse>
where
    Fut: Future<Output = Result<Vec<MetricFamily>>> + Send + 'static,
{
    let metrics_families = fetch_metrics.await?;

    let mut buffer = Vec::new();
    TextEncoder::new()
        .encode(&metrics_families, &mut buffer)
        .context("error encoding metrics")?;

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
    Extension(queue): Extension<Arc<AsyncBuildQueue>>,
) -> AxumResult<impl IntoResponse> {
    fetch_and_render_metrics(async move {
        let mut families = Vec::new();
        families.extend_from_slice(&instance_metrics.gather(&pool)?);
        families.extend_from_slice(&service_metrics.gather(&pool, &queue, &config).await?);
        Ok(families)
    })
    .await
}

pub(super) async fn service_metrics_handler(
    Extension(pool): Extension<Pool>,
    Extension(config): Extension<Arc<Config>>,
    Extension(metrics): Extension<Arc<ServiceMetrics>>,
    Extension(queue): Extension<Arc<AsyncBuildQueue>>,
) -> AxumResult<impl IntoResponse> {
    fetch_and_render_metrics(async move { metrics.gather(&pool, &queue, &config).await }).await
}

pub(super) async fn instance_metrics_handler(
    Extension(pool): Extension<Pool>,
    Extension(metrics): Extension<Arc<InstanceMetrics>>,
) -> AxumResult<impl IntoResponse> {
    fetch_and_render_metrics(async move { metrics.gather(&pool) }).await
}

/// Request recorder middleware
///
/// Looks similar, but *is not* a usable middleware / layer
/// since we need the route-name.
///
/// Can be used like:
/// ```text,ignore
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

    let otel_metrics = request
        .extensions()
        .get::<Arc<WebMetrics>>()
        .expect("otel metrics missing in request extensions")
        .clone();

    let start = Instant::now();
    let result = next.run(request).await;
    let resp_time = start.elapsed().as_secs_f64();

    let attrs = [KeyValue::new("route", route_name.to_string())];

    metrics
        .routes_visited
        .with_label_values(&[&route_name])
        .inc();

    otel_metrics.routes_visited.add(1, &attrs);

    metrics
        .response_time
        .with_label_values(&[&route_name])
        .observe(resp_time);

    otel_metrics.response_time.record(resp_time, &attrs);

    result
}

#[cfg(test)]
mod tests {
    use crate::test::{AxumResponseTestExt, AxumRouterTestExt, async_wrapper};
    use opentelemetry_sdk::metrics::data::{AggregatedMetrics, MetricData};
    use pretty_assertions::assert_eq;
    use std::collections::HashMap;

    #[test]
    fn test_response_times_count_being_collected() {
        const ROUTES: &[(&str, &str)] = &[
            ("/", "/"),
            ("/crate/hexponent/0.2.0", "/crate/{name}/{version}"),
            ("/crate/rcc/0.0.0", "/crate/{name}/{version}"),
            (
                "/crate/rcc/0.0.0/status.json",
                "/crate/{name}/{version}/status.json",
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
                "/releases/recent-failures/{page}",
            ),
            ("/releases/recent/1", "/releases/recent/{page}"),
            ("/-/static/robots.txt", "static resource"),
            ("/sitemap.xml", "/sitemap.xml"),
            (
                "/-/sitemap/a/sitemap.xml",
                "/-/sitemap/{letter}/sitemap.xml",
            ),
            ("/-/static/style.css", "static resource"),
            ("/-/static/vendored.css", "static resource"),
            ("/rustdoc/rcc/0.0.0/rcc/index.html", "rustdoc page"),
            ("/rustdoc/gcc/0.0.0/gcc/index.html", "rustdoc page"),
        ];

        async_wrapper(|env| async move {
            env.fake_release()
                .await
                .name("rcc")
                .version("0.0.0")
                .repo("https://github.com/jyn514/rcc")
                .create()
                .await?;
            env.fake_release()
                .await
                .name("rcc")
                .version("1.0.0")
                .build_result_failed()
                .create()
                .await?;
            env.fake_release()
                .await
                .name("hexponent")
                .version("0.2.0")
                .create()
                .await?;

            let frontend = env.web_app().await;
            let metrics = env.instance_metrics();

            for (route, _) in ROUTES.iter() {
                frontend.get(route).await?;
                frontend.get(route).await?;
            }

            let mut expected = HashMap::new();
            for (_, correct) in ROUTES.iter() {
                let entry = expected.entry(*correct).or_insert(0);
                *entry += 2;
            }

            let collected = dbg!(env.collected_metrics());
            let AggregatedMetrics::U64(MetricData::Sum(routes_visited)) = collected
                .get_metric("web", "docsrs.web.routes_visited")?
                .data()
            else {
                panic!("Expected Sum<U64> metric data");
            };

            dbg!(&routes_visited);

            let routes_visited: HashMap<String, u64> = routes_visited
                .data_points()
                .map(|dp| {
                    let route = dp
                        .attributes()
                        .find(|kv| kv.key.as_str() == "route")
                        .unwrap()
                        .clone()
                        .value;

                    (route.to_string(), dp.value())
                })
                .collect();

            assert_eq!(
                routes_visited,
                HashMap::from_iter(
                    vec![
                        ("/", 2),
                        ("/-/sitemap/{letter}/sitemap.xml", 2),
                        ("/crate/{name}/{version}", 4),
                        ("/crate/{name}/{version}/status.json", 2),
                        ("/releases", 2),
                        ("/releases/feed", 2),
                        ("/releases/queue", 2),
                        ("/releases/recent-failures", 2),
                        ("/releases/recent-failures/{page}", 2),
                        ("/releases/recent/{page}", 2),
                        ("/sitemap.xml", 2),
                        ("rustdoc page", 4),
                        ("static resource", 16),
                    ]
                    .into_iter()
                    .map(|(k, v)| (k.to_string(), v))
                )
            );

            let AggregatedMetrics::F64(MetricData::Histogram(response_time)) = collected
                .get_metric("web", "docsrs.web.response_time")?
                .data()
            else {
                panic!("Expected Histogram<F64> metric data");
            };

            dbg!(&response_time);

            let response_time_sample_counts: HashMap<String, u64> = response_time
                .data_points()
                .map(|dp| {
                    let route = dp
                        .attributes()
                        .find(|kv| kv.key.as_str() == "route")
                        .unwrap()
                        .clone()
                        .value;

                    (route.to_string(), dp.count())
                })
                .collect();

            assert_eq!(
                response_time_sample_counts,
                HashMap::from_iter(
                    vec![
                        ("/", 2),
                        ("/-/sitemap/{letter}/sitemap.xml", 2),
                        ("/crate/{name}/{version}", 4),
                        ("/crate/{name}/{version}/status.json", 2),
                        ("/releases", 2),
                        ("/releases/feed", 2),
                        ("/releases/queue", 2),
                        ("/releases/recent-failures", 2),
                        ("/releases/recent-failures/{page}", 2),
                        ("/releases/recent/{page}", 2),
                        ("/sitemap.xml", 2),
                        ("rustdoc page", 4),
                        ("static resource", 16),
                    ]
                    .into_iter()
                    .map(|(k, v)| (k.to_string(), v))
                )
            );

            // this shows what the routes were *actually* recorded as, making it easier to update ROUTES if the name changes.
            let metrics_serialized = metrics.gather(&env.context.pool)?;
            let all_routes_visited = metrics_serialized
                .iter()
                .find(|x| x.name() == "docsrs_routes_visited")
                .unwrap();
            let routes_visited_pretty: Vec<_> = all_routes_visited
                .get_metric()
                .iter()
                .map(|metric| {
                    let labels = metric.get_label();
                    assert_eq!(labels.len(), 1); // not sure when this would be false
                    let route = labels[0].value();
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
        async_wrapper(|env| async move {
            let response = env.web_app().await.get("/about/metrics").await?;
            assert!(response.status().is_success());

            let body = response.text().await?;
            assert!(body.contains("docsrs_failed_builds"), "{}", body);
            assert!(body.contains("queued_crates_count"), "{}", body);
            Ok(())
        })
    }

    #[test]
    fn test_service_metrics_page_success() {
        async_wrapper(|env| async move {
            let response = env.web_app().await.get("/about/metrics/service").await?;
            assert!(response.status().is_success());

            let body = response.text().await?;
            assert!(!body.contains("docsrs_failed_builds"), "{}", body);
            assert!(body.contains("queued_crates_count"), "{}", body);
            Ok(())
        })
    }

    #[test]
    fn test_instance_metrics_page_success() {
        async_wrapper(|env| async move {
            let response = env.web_app().await.get("/about/metrics/instance").await?;
            assert!(response.status().is_success());

            let body = response.text().await?;
            assert!(body.contains("docsrs_failed_builds"), "{}", body);
            assert!(!body.contains("queued_crates_count"), "{}", body);
            Ok(())
        })
    }
}
