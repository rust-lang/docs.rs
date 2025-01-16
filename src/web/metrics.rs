use crate::{
    db::Pool, metrics::duration_to_seconds, web::error::AxumResult, AsyncBuildQueue, Config,
    InstanceMetrics, ServiceMetrics,
};
use anyhow::{Context as _, Result};
use axum::{
    extract::{Extension, MatchedPath, Request as AxumRequest},
    http::{header::CONTENT_TYPE, StatusCode},
    middleware::Next,
    response::IntoResponse,
};
use prometheus::{proto::MetricFamily, Encoder, TextEncoder};
use std::{borrow::Cow, future::Future, sync::Arc, time::Instant};

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

#[cfg(test)]
mod tests {
    use crate::test::{async_wrapper, AxumResponseTestExt, AxumRouterTestExt};
    use crate::Context;
    use std::collections::HashMap;

    #[test]
    fn test_response_times_count_being_collected() {
        const ROUTES: &[(&str, &str)] = &[
            ("/", "/"),
            ("/crate/hexponent/0.2.0", "/crate/{name}/{version}"),
            ("/crate/rcc/0.0.0", "/crate/{name}/{version}"),
            (
                "/crate/rcc/0.0.0/builds.json",
                "/crate/{name}/{version}/builds.json",
            ),
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

            // this shows what the routes were *actually* recorded as, making it easier to update ROUTES if the name changes.
            let metrics_serialized = metrics.gather(&env.async_pool().await?)?;
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
