use axum::{
    extract::{MatchedPath, Request as AxumRequest},
    http::StatusCode,
    middleware::Next,
    response::IntoResponse,
};
use docs_rs_opentelemetry::AnyMeterProvider;
use opentelemetry::{
    KeyValue,
    metrics::{Counter, Histogram},
};
use std::{borrow::Cow, sync::Arc, time::Instant};

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
                .with_boundaries(RESPONSE_TIME_HISTOGRAM_BUCKETS.to_vec())
                .with_unit("s")
                .build(),
        }
    }
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

    let otel_metrics = request
        .extensions()
        .get::<Arc<WebMetrics>>()
        .expect("otel metrics missing in request extensions")
        .clone();

    let start = Instant::now();
    let result = next.run(request).await;
    let resp_time = start.elapsed().as_secs_f64();

    // to be able to differentiate between kinds of responses (e.g., 2xx vs 4xx vs 5xx)
    // in response times, or RPM.
    // Special case for 304 Not Modified since it's about caching and not just redirecting.
    let status_kind = match result.status() {
        StatusCode::NOT_MODIFIED => "not_modified",
        s if s.is_informational() => "informational",
        s if s.is_success() => "success",
        s if s.is_redirection() => "redirection",
        s if s.is_client_error() => "client_error",
        s if s.is_server_error() => "server_error",
        _ => "other",
    };

    let attrs = [
        KeyValue::new("route", route_name.to_string()),
        KeyValue::new("status_kind", status_kind),
    ];

    otel_metrics.routes_visited.add(1, &attrs);
    otel_metrics.response_time.record(resp_time, &attrs);

    result
}

#[cfg(test)]
mod tests {
    use crate::test::{AxumRouterTestExt, async_wrapper};
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

            Ok(())
        })
    }
}
