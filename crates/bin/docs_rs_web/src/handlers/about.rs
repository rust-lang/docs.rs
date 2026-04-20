use crate::{
    cache::{CachePolicy, SURROGATE_KEY_DOCSRS_STATIC},
    error::{AxumErrorPage, AxumResult},
    extractors::{DbConnection, Path},
    impl_axum_webpage,
    page::templates::{AlertSeverityRender, RenderBrands, RenderSolid, filters},
    page::warnings::WarningsCache,
};
use askama::Template;
use axum::{extract::Extension, http::StatusCode, response::IntoResponse};
use docs_rs_build_limits::Limits;
use docs_rs_context::Context;
use docs_rs_database::service_config::{Abnormality, ConfigName, get_config};
use std::sync::Arc;

#[derive(Template)]
#[template(path = "core/about/builds.html")]
#[derive(Debug, Clone, PartialEq, Eq)]
struct AboutBuilds {
    /// The current version of rustc that docs.rs is using to build crates
    rustc_version: Option<String>,
    /// The default crate build limits
    limits: Limits,
    /// Just for the template, since this isn't shared with AboutPage
    active_tab: &'static str,
}

impl_axum_webpage!(
    AboutBuilds,
    // NOTE: potential future improvement: serve a special surrogate key, and
    // purge that after we updated the local toolchain.
    cache_policy = |_| CachePolicy::ShortInCdnAndBrowser,
);

#[derive(Debug, Clone, PartialEq, Template)]
#[template(path = "core/about/status.html")]
struct AboutStatus {
    abnormalities: Vec<Abnormality>,
}

impl_axum_webpage!(
    AboutStatus,
    // NOTE: potential future improvement: serve a special surrogate key, and
    // purge that after we updated the local toolchain.
    cache_policy = |_| CachePolicy::ShortInCdnAndBrowser,
);

pub(crate) async fn about_builds_handler(
    mut conn: DbConnection,
    Extension(context): Extension<Arc<Context>>,
) -> AxumResult<impl IntoResponse> {
    Ok(AboutBuilds {
        rustc_version: get_config::<String>(&mut conn, ConfigName::RustcVersion).await?,
        limits: Limits::new(context.config().build_limits()?),
        active_tab: "builds",
    })
}

pub(crate) async fn status_handler(
    Extension(warnings_cache): Extension<Arc<WarningsCache>>,
) -> AxumResult<impl IntoResponse> {
    Ok(AboutStatus {
        abnormalities: warnings_cache.get().await.abnormalities,
    })
}

macro_rules! about_page {
    ($ty:ident, $template:literal) => {
        #[derive(Template)]
        #[template(path = $template)]
        struct $ty;

        impl_axum_webpage! {
            $ty,
            cache_policy = |_| CachePolicy::ForeverInCdn(SURROGATE_KEY_DOCSRS_STATIC.into())
        }
    };
}

about_page!(AboutPage, "core/about/index.html");
about_page!(AboutPageBadges, "core/about/badges.html");
about_page!(AboutPageMetadata, "core/about/metadata.html");
about_page!(AboutPageRedirection, "core/about/redirections.html");
about_page!(AboutPageDownload, "core/about/download.html");
about_page!(AboutPageRustdocJson, "core/about/rustdoc-json.html");

pub(crate) async fn about_handler(subpage: Option<Path<String>>) -> AxumResult<impl IntoResponse> {
    let subpage = match subpage {
        Some(subpage) => subpage.0,
        None => "index".to_string(),
    };

    let response = match &subpage[..] {
        "about" | "index" => AboutPage.into_response(),
        "badges" => AboutPageBadges.into_response(),
        "metadata" => AboutPageMetadata.into_response(),
        "redirections" => AboutPageRedirection.into_response(),
        "download" => AboutPageDownload.into_response(),
        "rustdoc-json" => AboutPageRustdocJson.into_response(),
        _ => {
            let msg = "This /about page does not exist. \
                Perhaps you are interested in <a href=\"https://github.com/rust-lang/docs.rs/tree/master/templates/core/about\">creating</a> it?";
            let page = AxumErrorPage {
                title: "The requested page does not exist",
                message: msg.into(),
                status: StatusCode::NOT_FOUND,
            };
            page.into_response()
        }
    };
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{
        AxumResponseTestExt as _, AxumRouterTestExt, TestEnvironment, TestEnvironmentExt as _,
    };
    use anyhow::Result;
    use chrono::{TimeZone, Utc};
    use docs_rs_database::service_config::{AlertSeverity, AnchorId, set_config};
    use docs_rs_uri::EscapedURI;
    use kuchikiki::traits::TendrilSink;

    #[tokio::test(flavor = "multi_thread")]
    async fn about_page() -> Result<()> {
        let env = TestEnvironment::new().await?;
        let web = env.web_app().await;
        for file in std::fs::read_dir("templates/core/about")? {
            use std::ffi::OsStr;

            let file_path = file?.path();
            if file_path.extension() != Some(OsStr::new("html"))
                || file_path.file_stem() == Some(OsStr::new("index"))
                || file_path.file_stem() == Some(OsStr::new("status"))
            {
                continue;
            }
            let filename = file_path.file_stem().unwrap().to_str().unwrap();
            let path = format!("/about/{filename}");
            let response = web.assert_success(&path).await?;

            if filename == "builds" {
                response.assert_cache_control(CachePolicy::ShortInCdnAndBrowser, env.config());
            } else {
                response.assert_cache_control(
                    CachePolicy::ForeverInCdn(SURROGATE_KEY_DOCSRS_STATIC.into()),
                    env.config(),
                );
            }
        }
        web.assert_success("/about").await?;
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
                anchor_id: AnchorId::Manual,
                url: "https://example.com/maintenance"
                    .parse::<EscapedURI>()
                    .unwrap(),
                text: "Scheduled maintenance".into(),
                explanation: Some("Planned maintenance is in progress.".into()),
                start_time: Some(Utc.with_ymd_and_hms(2023, 1, 30, 19, 32, 33).unwrap()),
                severity: AlertSeverity::Warn,
            },
        )
        .await?;
        drop(conn);

        let web = env.web_app().await;
        let page =
            kuchikiki::parse_html().one(web.assert_success("/-/status/").await?.text().await?);

        let body_text = page.text_contents();
        assert!(body_text.contains("Scheduled maintenance"));
        assert!(body_text.contains("Planned maintenance is in progress."));
        assert!(body_text.contains("2023-01-30 19:32:33 UTC"));

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn about_status_page_shows_no_abnormalities_when_clean() -> Result<()> {
        let env = TestEnvironment::new().await?;
        let web = env.web_app().await;

        let page =
            kuchikiki::parse_html().one(web.assert_success("/-/status/").await?.text().await?);

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
                anchor_id: AnchorId::Manual,
                url: "https://example.com/maintenance"
                    .parse::<EscapedURI>()
                    .unwrap(),
                text: "Scheduled maintenance".into(),
                explanation: Some(
                    "Planned maintenance is <em>in progress</em>. See <a href=\"/details\">details</a>.".into(),
                ),
                start_time: None,
                severity: AlertSeverity::Warn,
            },
        )
        .await?;
        drop(conn);

        let web = env.web_app().await;
        let html = web.assert_success("/-/status/").await?.text().await?;
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
