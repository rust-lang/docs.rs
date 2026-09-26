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
use docs_rs_rustsec::RustsecClient;
use docs_rs_std_replacements::{ReplacementDetails, StdReplacements};
use docs_rs_types::KrateName;
use std::sync::Arc;

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

/// subset of `rustsec::Advisory` that we
/// currently show in the crate-warnings.
///
/// So we don't have to clone the whole advisory.
struct UnmaintainedWarning {
    id: String,
    title: String,
}

#[derive(Template)]
#[template(path = "header/crate_warnings.html")]
struct CrateWarnings {
    replacement: Option<Arc<ReplacementDetails>>,
    unmaintained: Option<UnmaintainedWarning>,
    cache_policy: CachePolicy,
}

impl_axum_webpage! {
    CrateWarnings,
    cache_policy = |page| page.cache_policy.clone()
}

/// Render crate warnings for insertion into the documentation topbar.
pub(crate) async fn crate_warnings(
    Extension(rustsec): Extension<Arc<RustsecClient>>,
    Extension(std_replacements): Extension<Arc<StdReplacements>>,
    Path(name): Path<KrateName>,
) -> AxumResult<impl IntoResponse> {
    // default cache policy for this endpoint.
    // Should be used:
    // - when we have results
    // - when we don't have results for that crate, but the database was loaded.
    let mut cache_policy = CachePolicy::LongerInCdnAndBrowser;

    let unmaintained = if let Some(database) = rustsec.database() {
        database
            .find_unmaintained(&name)
            .map(|advisory| UnmaintainedWarning {
                id: advisory.id().to_string(),
                title: advisory.title().to_string(),
            })
    } else {
        // when the database wasn't available, don't cache for long.
        cache_policy = CachePolicy::ShortInCdnAndBrowser;
        None
    };

    let replacement = if let Some(database) = std_replacements.database() {
        database.get(&name).cloned()
    } else {
        // The first background refresh has not completed yet.
        cache_policy = CachePolicy::ShortInCdnAndBrowser;
        None
    };

    Ok(CrateWarnings {
        replacement,
        unmaintained,
        cache_policy,
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
    use docs_rs_config::AppConfig as _;
    use docs_rs_database::service_config::{Abnormality, ConfigName, set_config};
    use docs_rs_rustsec::{RustsecClient, testing};
    use docs_rs_std_replacements::testing::{StdReplacementMockServer, std_replacement};
    use docs_rs_types::{KrateName, testing::V1};
    use docs_rs_uri::EscapedURI;
    use http::StatusCode;
    use kuchikiki::traits::TendrilSink;
    use std::{str::FromStr, sync::Arc};

    const OWNED_ALLOC: KrateName = KrateName::from_static("owned-alloc");

    fn rustsec_client() -> Result<Arc<RustsecClient>> {
        Ok(Arc::new(testing::client([testing::unmaintained(
            &OWNED_ALLOC,
        )])?))
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn crate_warnings_renders_separate_menu_items() -> Result<()> {
        let std_server = StdReplacementMockServer::new()
            .await
            .mock()
            .replacement(OWNED_ALLOC, std_replacement("Use std"))
            .start()
            .await;

        let rustsec = rustsec_client()?;

        let env = TestEnvironment::builder()
            .std_replacements_config(std_server.config().build())
            .rustsec(rustsec)
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
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn crate_warnings_caches_replacement_404() -> Result<()> {
        let std_server = StdReplacementMockServer::new()
            .await
            .mock()
            .status_code(StatusCode::NOT_FOUND)
            .start()
            .await;

        let env = TestEnvironment::builder()
            .std_replacements_config(std_server.config().build())
            .rustsec(rustsec_client()?)
            .build()
            .await?;

        let response = env
            .web_app()
            .await
            .assert_success("/-/partial/crate-warnings/owned-alloc/")
            .await?;

        response.assert_cache_control(CachePolicy::ShortInCdnAndBrowser, env.config());

        let html = response.text().await?;
        assert!(html.contains("Unmaintained"));
        assert!(!html.contains("Std alternative"));

        std_server.assert_async().await;
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn crate_warnings_renders_rustsec_warning_when_replacements_are_unavailable() -> Result<()>
    {
        let std_server = StdReplacementMockServer::new()
            .await
            .mock()
            .replacement(OWNED_ALLOC, std_replacement("Use std"))
            .status_code(StatusCode::SERVICE_UNAVAILABLE)
            .start()
            .await;

        let env = TestEnvironment::builder()
            .std_replacements_config(std_server.config().build())
            .rustsec(rustsec_client()?)
            .build()
            .await?;

        let response = env
            .web_app()
            .await
            .assert_success("/-/partial/crate-warnings/owned-alloc/")
            .await?;

        response.assert_cache_control(CachePolicy::ShortInCdnAndBrowser, env.config());

        let html = response.text().await?;
        assert!(html.contains("Unmaintained"));
        assert!(!html.contains("Std alternative"));

        std_server.assert_async().await;
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
