use crate::{
    cache::CachePolicy,
    error::AxumResult,
    extractors::{DbConnection, Path},
    impl_axum_webpage,
    page::{
        templates::{RenderBrands, RenderSolid},
        warnings::{self, ActiveAbnormalities},
    },
};
use askama::Template;
use axum::{
    extract::Extension,
    response::{IntoResponse, Response as AxumResponse},
};
use docs_rs_build_queue::AsyncBuildQueue;
use docs_rs_database::service_config::Abnormality;
use docs_rs_rustsec::{OsvAdvisory, RustsecClient};
use docs_rs_std_replacements::{ReplacementDetails, StdReplacements};
use docs_rs_types::KrateName;
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use tracing::error;

#[derive(Debug, Clone, PartialEq, Template)]
#[template(path = "core/about/status.html")]
struct AboutStatus {
    abnormalities: Vec<Abnormality>,
}

impl_axum_webpage!(
    AboutStatus,
    cache_policy = |_| CachePolicy::ShortInCdnAndBrowser
);

#[derive(Template)]
#[template(path = "header/abnormalities.html")]
#[derive(Debug, Clone)]
struct Abnormalities {
    abnormalities: ActiveAbnormalities,
}

impl_axum_webpage! {
    Abnormalities,
    cache_policy = |_| CachePolicy::LongerInCdnAndBrowser
}

pub(crate) async fn status_handler(
    Extension(build_queue): Extension<Arc<AsyncBuildQueue>>,
    mut conn: DbConnection,
) -> AxumResult<impl IntoResponse> {
    Ok(AboutStatus {
        abnormalities: warnings::load_abnormalities(&mut conn, &build_queue).await?,
    })
}

pub(crate) async fn abnormalities(
    Extension(build_queue): Extension<Arc<AsyncBuildQueue>>,
    mut conn: DbConnection,
) -> AxumResult<AxumResponse> {
    Ok(Abnormalities {
        abnormalities: warnings::load_abnormalities(&mut conn, &build_queue).await?,
    }
    .into_response())
}

#[derive(Template)]
#[template(path = "header/crate_warnings.html")]
struct CrateWarnings {
    replacement: Option<Arc<ReplacementDetails>>,
    unmaintained: Option<Arc<OsvAdvisory>>,
    ttl: Duration,
}

impl_axum_webpage! {
    CrateWarnings,
    cache_policy = |page| CachePolicy::InCdnAndBrowser(page.ttl)
}

/// Render crate warnings for insertion into the documentation topbar.
pub(crate) async fn crate_warnings(
    Extension(rustsec): Extension<Arc<RustsecClient>>,
    Extension(std_replacements): Extension<Arc<StdReplacements>>,
    Path(name): Path<KrateName>,
) -> AxumResult<impl IntoResponse> {
    let started_at = Instant::now();

    // Failed lookups return an empty result with zero TTL; disabled clients return None.
    let (std_replacement, unmaintained) = tokio::join!(
        async {
            std_replacements
                .get(&name)
                .await
                .inspect_err(
                    |err| error!(?err, %name, "failed to fetch standard-library replacements"),
                )
                .unwrap_or_default()
        },
        async {
            rustsec
                .find_unmaintained(&name)
                .await
                .inspect_err(|err| error!(?err, %name, "failed to fetch RustSec advisories"))
                .unwrap_or_default()
        }
    );

    // NOTE: both std replacements and rustsec advisories are fetched from github pages.
    // They have a TTL when we fetch them, and based on that we locally cache the responses.
    // What happens here, based on that:
    // we take the shorter of these TTLs, and use it to cache the partial in Fastly as long as it's
    // allowed.
    let ttl = std_replacement
        .ttl
        .min(unmaintained.ttl)
        // Conservatively account for time spent waiting for the slower lookup.
        .saturating_sub(started_at.elapsed());

    Ok(CrateWarnings {
        replacement: std_replacement.value,
        unmaintained: unmaintained.value,
        ttl,
    })
}

#[cfg(test)]
mod tests {
    use crate::{
        cache::CachePolicy,
        testing::{
            AxumResponseTestExt, AxumRouterTestExt, TestEnvironment, TestEnvironmentExt as _,
        },
    };
    use anyhow::Result;
    use axum_extra::headers::{CacheControl, HeaderMapExt as _};
    use docs_rs_config::AppConfig as _;
    use docs_rs_database::service_config::{Abnormality, ConfigName, set_config};
    use docs_rs_rustsec::testing::RustsecMockServer;
    use docs_rs_std_replacements::testing::{StdReplacementMockServer, std_replacement};
    use docs_rs_types::{Duration, KrateName, testing::V1};
    use docs_rs_uri::EscapedURI;
    use http::StatusCode;
    use kuchikiki::traits::TendrilSink;
    use std::str::FromStr;
    use test_case::test_case;

    const OWNED_ALLOC: KrateName = KrateName::from_static("owned-alloc");

    fn assert_ttl(response: &axum::response::Response, expected: Duration) {
        let header = response
            .headers()
            .typed_get::<CacheControl>()
            .expect("valid Cache-Control header");

        let ttl = header.max_age().expect("max-age directive");
        if expected == Duration::ZERO {
            assert!(!header.public());
            assert_eq!(ttl, expected);
        } else {
            let expected = expected.as_secs();
            assert!(header.public());
            assert!(
                (expected.saturating_sub(10)..=expected).contains(&ttl.as_secs()),
                "{header:?}"
            );
        }
    }

    #[test_case(Duration::from_secs(1200), Duration::from_secs(1800), false, Duration::from_secs(1200); "replacement shorter")]
    #[test_case(Duration::from_mins(2), Duration::from_mins(1), false, Duration::from_mins(1); "rustsec shorter")]
    #[test_case(Duration::ZERO, Duration::from_mins(2), false, Duration::ZERO; "uncacheable")]
    #[test_case(Duration::from_mins(1), Duration::from_mins(2), true, Duration::from_mins(1); "empty HTML")]
    #[tokio::test(flavor = "multi_thread")]
    async fn crate_warnings_uses_remaining_ttl(
        std_ttl: Duration,
        rustsec_ttl: Duration,
        empty: bool,
        expected: Duration,
    ) -> Result<()> {
        let replacement = std_replacement("Use std");

        let mut std_server = StdReplacementMockServer::new().await;
        let std_cache = CacheControl::new().with_max_age(std_ttl.into());
        std_server = if empty {
            std_server.mock().cache_control(std_cache).start().await
        } else {
            std_server
                .mock()
                .replacement(OWNED_ALLOC, replacement.clone())
                .cache_control(std_cache)
                .start()
                .await
        };

        let rustsec_cache = CacheControl::new().with_max_age(rustsec_ttl.into());
        let mut rustsec_server = RustsecMockServer::new().await;
        rustsec_server = if empty {
            rustsec_server
                .mock(OWNED_ALLOC)
                .status_code(StatusCode::NOT_FOUND)
                .cache_control(rustsec_cache)
                .start()
                .await
        } else {
            rustsec_server
                .mock(OWNED_ALLOC)
                .empty(false)
                .cache_control(rustsec_cache)
                .start()
                .await
        };

        let env = TestEnvironment::builder()
            .std_replacements_config(std_server.config().build())
            .rustsec_config(
                rustsec_server
                    .config()
                    .cache_default_ttl(rustsec_ttl)
                    .build(),
            )
            .build()
            .await?;

        let response = env
            .web_app()
            .await
            .assert_success("/-/partial/crate-warnings/owned-alloc/")
            .await?;

        if expected == Duration::ZERO {
            response.assert_cache_control(CachePolicy::NoCaching, env.config());
        } else {
            assert_ttl(&response, expected);
        }

        if empty {
            assert!(response.text().await?.is_empty());
        }

        std_server.assert_async().await;
        rustsec_server.assert_async().await;
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn crate_warnings_renders_separate_menu_items() -> Result<()> {
        let std_server = StdReplacementMockServer::new()
            .await
            .mock()
            .replacement(OWNED_ALLOC, std_replacement("Use std"))
            .start()
            .await;

        let rustsec_server = RustsecMockServer::new()
            .await
            .mock(OWNED_ALLOC)
            .start()
            .await;

        let env = TestEnvironment::builder()
            .std_replacements_config(std_server.config().build())
            .rustsec_config(rustsec_server.config().build())
            .build()
            .await?;

        let html = env
            .web_app()
            .await
            .assert_success("/-/partial/crate-warnings/owned-alloc/")
            .await?
            .text()
            .await?;

        let page = kuchikiki::parse_html().one(format!("<ul>{html}</ul>"));
        let labels: Vec<_> = page
            .select("ul > li.crate-warning > a.warn")
            .unwrap()
            .map(|link| link.text_contents().trim().to_owned())
            .collect();

        assert_eq!(labels, ["Std alternative", "Unmaintained"]);
        assert_eq!(
            page.select("li.crate-warning + li.crate-warning")
                .unwrap()
                .count(),
            1
        );

        std_server.assert_async().await;
        rustsec_server.assert_async().await;
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn crate_warnings_caches_replacement_404() -> Result<()> {
        let rustsec_server = RustsecMockServer::new()
            .await
            .mock(OWNED_ALLOC)
            .cache_control(CacheControl::new().with_max_age(std::time::Duration::from_secs(600)))
            .start()
            .await;
        let std_server = StdReplacementMockServer::new()
            .await
            .mock()
            .status_code(StatusCode::NOT_FOUND)
            .start()
            .await;

        let env = TestEnvironment::builder()
            .std_replacements_config(std_server.config().build())
            .rustsec_config(rustsec_server.config().build())
            .build()
            .await?;

        let response = env
            .web_app()
            .await
            .assert_success("/-/partial/crate-warnings/owned-alloc/")
            .await?;

        assert_ttl(&response, Duration::from_secs(600));

        let html = response.text().await?;
        assert!(html.contains("Unmaintained"));
        assert!(!html.contains("Std alternative"));

        std_server.assert_async().await;
        rustsec_server.assert_async().await;
        Ok(())
    }

    #[test_case(true; "replacement unavailable")]
    #[test_case(false; "rustsec unavailable")]
    #[tokio::test(flavor = "multi_thread")]
    async fn crate_warnings_preserves_healthy_source(replacement_fails: bool) -> Result<()> {
        let std_server = StdReplacementMockServer::new()
            .await
            .mock()
            .replacement(OWNED_ALLOC, std_replacement("Use std"))
            .status_code(if replacement_fails {
                StatusCode::SERVICE_UNAVAILABLE
            } else {
                StatusCode::OK
            })
            .start()
            .await;
        let rustsec_server = RustsecMockServer::new()
            .await
            .mock(OWNED_ALLOC)
            .status_code(if replacement_fails {
                StatusCode::OK
            } else {
                StatusCode::SERVICE_UNAVAILABLE
            })
            .start()
            .await;

        let env = TestEnvironment::builder()
            .std_replacements_config(std_server.config().build())
            .rustsec_config(rustsec_server.config().build())
            .build()
            .await?;

        let response = env
            .web_app()
            .await
            .assert_success("/-/partial/crate-warnings/owned-alloc/")
            .await?;

        response.assert_cache_control(CachePolicy::NoCaching, env.config());

        let html = response.text().await?;
        assert_eq!(html.contains("Unmaintained"), replacement_fails);
        assert_eq!(html.contains("Std alternative"), !replacement_fails);

        std_server.assert_async().await;
        rustsec_server.assert_async().await;
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn abnormalities_partial_renders_configured_link() -> Result<()> {
        let env = TestEnvironment::new().await?;

        let mut conn = env.async_conn().await?;
        set_config(
            &mut conn,
            ConfigName::Abnormality,
            Abnormality {
                url: "https://example.com/maintenance"
                    .parse::<EscapedURI>()
                    .unwrap(),
                text: "Scheduled maintenance".into(),
                explanation: Some("Planned maintenance is in progress.".into()),
            },
        )
        .await?;

        let web = env.web_app().await;
        let page = kuchikiki::parse_html().one(
            web.assert_success_cached(
                "/-/partial/abnormalities/",
                CachePolicy::LongerInCdnAndBrowser,
                env.config(),
            )
            .await?
            .text()
            .await?,
        );
        let alert = page
            .select("a.pure-menu-link.warn")
            .unwrap()
            .next()
            .expect("missing abnormality");

        assert_eq!(alert.attributes.borrow().get("href"), Some("/-/status/"));
        assert!(alert.text_contents().trim().is_empty());
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn abnormalities_partial_renders_queue_alert() -> Result<()> {
        let mut queue_config = docs_rs_build_queue::Config::test_config()?;
        queue_config.length_warning_threshold = 1;
        let env = TestEnvironment::builder()
            .build_queue_config(queue_config)
            .build()
            .await?;
        let queue = env.build_queue()?.clone();

        for idx in 0..2 {
            let name = KrateName::from_str(&format!("queued-crate-{idx}"))?;
            queue.add_crate(&name, &V1, 0).await?;
        }

        let web = env.web_app().await;
        let page = kuchikiki::parse_html().one(
            web.assert_success_cached(
                "/-/partial/abnormalities/",
                CachePolicy::LongerInCdnAndBrowser,
                env.config(),
            )
            .await?
            .text()
            .await?,
        );
        let alert = page
            .select("a.pure-menu-link.warn")
            .unwrap()
            .next()
            .expect("missing queue alert");

        assert_eq!(alert.attributes.borrow().get("href"), Some("/-/status/"));
        assert!(alert.text_contents().trim().is_empty());
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn about_status_page_renders_abnormality_details() -> Result<()> {
        let env = TestEnvironment::new().await?;

        let mut conn = env.async_conn().await?;
        set_config(
            &mut conn,
            ConfigName::Abnormality,
            Abnormality {
                url: "https://example.com/maintenance"
                    .parse::<EscapedURI>()
                    .unwrap(),
                text: "Scheduled maintenance".into(),
                explanation: Some("Planned maintenance is in progress.".into()),
            },
        )
        .await?;
        drop(conn);

        let web = env.web_app().await;
        let page = kuchikiki::parse_html().one(
            web.assert_success_cached(
                "/-/status/",
                CachePolicy::ShortInCdnAndBrowser,
                env.config(),
            )
            .await?
            .text()
            .await?,
        );

        let body_text = page.text_contents();
        assert!(body_text.contains("Scheduled maintenance"));
        assert!(body_text.contains("Planned maintenance is in progress."));

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn about_status_page_shows_no_abnormalities_when_clean() -> Result<()> {
        let env = TestEnvironment::new().await?;
        let web = env.web_app().await;

        let page = kuchikiki::parse_html().one(
            web.assert_success_cached(
                "/-/status/",
                CachePolicy::ShortInCdnAndBrowser,
                env.config(),
            )
            .await?
            .text()
            .await?,
        );

        let body_text = page.text_contents();
        assert!(body_text.contains("No abnormalities detected currently."));
        assert_eq!(
            page.select(".about h3").unwrap().count(),
            0,
            "should not render any abnormality headings"
        );

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn about_status_page_renders_html_explanation() -> Result<()> {
        let env = TestEnvironment::new().await?;

        let mut conn = env.async_conn().await?;
        set_config(
            &mut conn,
            ConfigName::Abnormality,
            Abnormality {
                url: "https://example.com/maintenance"
                    .parse::<EscapedURI>()
                    .unwrap(),
                text: "Scheduled maintenance".into(),
                explanation: Some(
                    "Planned maintenance is <em>in progress</em>. See <a href=\"/details\">details</a>.".into(),
                ),
            },
        )
        .await?;
        drop(conn);

        let web = env.web_app().await;
        let html = web
            .assert_success_cached(
                "/-/status/",
                CachePolicy::ShortInCdnAndBrowser,
                env.config(),
            )
            .await?
            .text()
            .await?;
        let page = kuchikiki::parse_html().one(html.clone());

        // The <em> tag should be rendered as an actual HTML element, not escaped.
        assert!(
            html.contains("<em>in progress</em>"),
            "HTML in explanation should be rendered unescaped"
        );

        // The <a> tag should be rendered as an actual link.
        let link = page
            .select(".about p a[href='/details']")
            .unwrap()
            .next()
            .expect("explanation should contain a rendered <a> link");
        assert!(link.text_contents().contains("details"));

        Ok(())
    }
}
