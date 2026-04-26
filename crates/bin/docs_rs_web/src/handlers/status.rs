use crate::{
    cache::CachePolicy,
    error::AxumResult,
    impl_axum_webpage,
    page::{
        templates::{AlertSeverityRender, RenderBrands, RenderSolid},
        warnings::{ActiveAbnormalities, WarningsCache},
    },
};
use askama::Template;
use axum::{
    extract::Extension,
    response::{IntoResponse, Response as AxumResponse},
};
use docs_rs_database::service_config::Abnormality;
use docs_rs_headers::SURROGATE_KEY_WARNINGS;
use std::sync::Arc;

#[derive(Debug, Clone, PartialEq, Template)]
#[template(path = "core/about/status.html")]
struct AboutStatus {
    abnormalities: Vec<Abnormality>,
}

impl_axum_webpage!(
    AboutStatus,
    cache_policy = |_| CachePolicy::ForeverInCdn(SURROGATE_KEY_WARNINGS.into()),
);

#[derive(Template)]
#[template(path = "header/abnormalities.html")]
#[derive(Debug, Clone)]
struct Abnormalities {
    abnormalities: ActiveAbnormalities,
}

impl_axum_webpage! {
    Abnormalities,
    cache_policy = |_| CachePolicy::ForeverInCdn(
        SURROGATE_KEY_WARNINGS.into()
    ),
}

pub(crate) async fn status_handler(
    Extension(warnings_cache): Extension<Arc<WarningsCache>>,
) -> AxumResult<impl IntoResponse> {
    Ok(AboutStatus {
        abnormalities: warnings_cache.get().await.abnormalities,
    })
}

pub(crate) async fn abnormalities(
    Extension(warnings): Extension<Arc<WarningsCache>>,
) -> AxumResult<AxumResponse> {
    Ok(Abnormalities {
        abnormalities: warnings.get().await.abnormalities,
    }
    .into_response())
}

#[cfg(test)]
mod tests {
    use crate::testing::{
        AxumResponseTestExt, AxumRouterTestExt, TestEnvironment, TestEnvironmentExt as _,
    };
    use anyhow::Result;
    use chrono::{TimeZone as _, Utc};
    use docs_rs_config::AppConfig as _;
    use docs_rs_database::service_config::{
        Abnormality, AlertSeverity, AnchorId, ConfigName, set_config,
    };
    use docs_rs_types::{KrateName, testing::V1};
    use docs_rs_uri::EscapedURI;
    use kuchikiki::traits::TendrilSink;
    use std::str::FromStr;

    #[tokio::test(flavor = "multi_thread")]
    async fn abnormalities_partial_renders_configured_link() -> Result<()> {
        let env = TestEnvironment::new().await?;

        let mut conn = env.async_conn().await?;
        // NOTE: abnormalities are cached inside the web-app, so set them
        // before we fetch the web-app from the test-environments.
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
                start_time: None,
                severity: AlertSeverity::Warn,
            },
        )
        .await?;

        let web = env.web_app().await;
        let page = kuchikiki::parse_html().one(
            web.assert_success("/-/partial/abnormalities/")
                .await?
                .text()
                .await?,
        );
        let alert = page
            .select("a.pure-menu-link.warn")
            .unwrap()
            .next()
            .expect("missing abnormality");

        assert_eq!(
            alert.attributes.borrow().get("href"),
            Some("/-/status/#manual")
        );
        assert!(alert.text_contents().contains("Scheduled maintenance"));
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
            queue.add_crate(&name, &V1, 0, None).await?;
        }

        let web = env.web_app().await;
        let page = kuchikiki::parse_html().one(
            web.assert_success("/-/partial/abnormalities/")
                .await?
                .text()
                .await?,
        );
        let alert = page
            .select("a.pure-menu-link.warn")
            .unwrap()
            .next()
            .expect("missing queue alert");

        assert_eq!(
            alert.attributes.borrow().get("href"),
            Some("/-/status/#queue-length")
        );
        assert!(alert.text_contents().contains("long build queue"));
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn manual_abnormality_wins_when_multiple_abnormalities_are_active() -> Result<()> {
        let mut queue_config = docs_rs_build_queue::Config::test_config()?;
        queue_config.length_warning_threshold = 1;
        let env = TestEnvironment::builder()
            .build_queue_config(queue_config)
            .build()
            .await?;

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
                start_time: None,
                severity: AlertSeverity::Error,
            },
        )
        .await?;
        drop(conn);

        let queue = env.build_queue()?.clone();
        for idx in 0..2 {
            let name = KrateName::from_str(&format!("queued-crate-{idx}"))?;
            queue.add_crate(&name, &V1, 0, None).await?;
        }

        let web = env.web_app().await;
        let page = kuchikiki::parse_html().one(
            web.assert_success("/-/partial/abnormalities/")
                .await?
                .text()
                .await?,
        );
        let alert = page
            .select("a.pure-menu-link.error")
            .unwrap()
            .next()
            .expect("missing manual alert");

        assert_eq!(alert.attributes.borrow().get("href"), Some("#"));
        assert!(alert.text_contents().contains("Scheduled maintenance"));
        let dropdown_links = page
            .select("ul.pure-menu-children a.pure-menu-link")
            .unwrap()
            .map(|link| {
                (
                    link.attributes.borrow().get("href").unwrap().to_string(),
                    link.text_contents(),
                )
            })
            .collect::<Vec<_>>();

        assert!(dropdown_links.iter().any(|(href, text)| {
            href == "/-/status/#manual" && text.contains("Scheduled maintenance")
        }));
        assert!(dropdown_links.iter().any(|(href, text)| {
            href == "/-/status/#queue-length" && text.contains("long build queue")
        }));
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
