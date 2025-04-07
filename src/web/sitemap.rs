use crate::{
    Config,
    docbuilder::Limits,
    impl_axum_webpage,
    utils::{ConfigName, get_config},
    web::{
        AxumErrorPage,
        error::{AxumNope, AxumResult},
        extractors::{DbConnection, Path},
        page::templates::{RenderBrands, RenderSolid, filters},
    },
};
use askama::Template;
use axum::{extract::Extension, http::StatusCode, response::IntoResponse};
use chrono::{TimeZone, Utc};
use futures_util::stream::TryStreamExt;
use std::sync::Arc;

/// sitemap index
#[derive(Template)]
#[template(path = "core/sitemapindex.xml")]
#[derive(Debug, Clone, PartialEq, Eq)]
struct SitemapIndexXml {
    sitemaps: Vec<char>,
    csp_nonce: String,
}

impl_axum_webpage! {
    SitemapIndexXml,
    content_type = "application/xml",
}

pub(crate) async fn sitemapindex_handler() -> impl IntoResponse {
    let sitemaps: Vec<char> = ('a'..='z').collect();

    SitemapIndexXml {
        sitemaps,
        csp_nonce: String::new(),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SitemapRow {
    crate_name: String,
    last_modified: String,
    target_name: String,
}

/// The sitemap
#[derive(Template)]
#[template(path = "core/sitemap.xml")]
#[derive(Debug, Clone, PartialEq, Eq)]
struct SitemapXml {
    releases: Vec<SitemapRow>,
    csp_nonce: String,
}

impl_axum_webpage! {
    SitemapXml,
    content_type = "application/xml",
}

pub(crate) async fn sitemap_handler(
    Path(letter): Path<String>,
    mut conn: DbConnection,
) -> AxumResult<impl IntoResponse> {
    if letter.len() != 1 {
        return Err(AxumNope::ResourceNotFound);
    } else if let Some(ch) = letter.chars().next() {
        if !(ch.is_ascii_lowercase()) {
            return Err(AxumNope::ResourceNotFound);
        }
    }

    let releases: Vec<_> = sqlx::query!(
        r#"SELECT crates.name,
                releases.target_name,
                MAX(releases.release_time) as "release_time!"
         FROM crates
         INNER JOIN releases ON releases.crate_id = crates.id
         WHERE
            rustdoc_status = true AND
            crates.name ILIKE $1
         GROUP BY crates.name, releases.target_name
         "#,
        format!("{letter}%"),
    )
    .fetch(&mut *conn)
    .map_ok(|row| SitemapRow {
        crate_name: row.name,
        target_name: row
            .target_name
            .expect("when we have rustdoc_status=true, this field is filled"),
        last_modified: row
            .release_time
            // On Aug 27 2022 we added `<link rel="canonical">` to all pages,
            // so they should all get recrawled if they haven't been since then.
            .max(Utc.with_ymd_and_hms(2022, 8, 28, 0, 0, 0).unwrap())
            .format("%+")
            .to_string(),
    })
    .try_collect()
    .await?;

    Ok(SitemapXml {
        releases,
        csp_nonce: String::new(),
    })
}

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
    csp_nonce: String,
}

impl_axum_webpage!(AboutBuilds);

pub(crate) async fn about_builds_handler(
    mut conn: DbConnection,
    Extension(config): Extension<Arc<Config>>,
) -> AxumResult<impl IntoResponse> {
    Ok(AboutBuilds {
        rustc_version: get_config::<String>(&mut conn, ConfigName::RustcVersion).await?,
        limits: Limits::new(&config),
        active_tab: "builds",
        csp_nonce: String::new(),
    })
}

macro_rules! about_page {
    ($ty:ident, $template:literal) => {
        #[derive(Template)]
        #[template(path = $template)]
        struct $ty {
            active_tab: &'static str,
            csp_nonce: String,
        }

        impl_axum_webpage! { $ty }
    };
}

about_page!(AboutPage, "core/about/index.html");
about_page!(AboutPageBadges, "core/about/badges.html");
about_page!(AboutPageMetadata, "core/about/metadata.html");
about_page!(AboutPageRedirection, "core/about/redirections.html");
about_page!(AboutPageDownload, "core/about/download.html");

pub(crate) async fn about_handler(subpage: Option<Path<String>>) -> AxumResult<impl IntoResponse> {
    let subpage = match subpage {
        Some(subpage) => subpage.0,
        None => "index".to_string(),
    };

    let response = match &subpage[..] {
        "about" | "index" => AboutPage {
            active_tab: "index",
            csp_nonce: String::new(),
        }
        .into_response(),
        "badges" => AboutPageBadges {
            active_tab: "badges",
            csp_nonce: String::new(),
        }
        .into_response(),
        "metadata" => AboutPageMetadata {
            active_tab: "metadata",
            csp_nonce: String::new(),
        }
        .into_response(),
        "redirections" => AboutPageRedirection {
            active_tab: "redirections",
            csp_nonce: String::new(),
        }
        .into_response(),
        "download" => AboutPageDownload {
            active_tab: "download",
            csp_nonce: String::new(),
        }
        .into_response(),
        _ => {
            let msg = "This /about page does not exist. \
                Perhaps you are interested in <a href=\"https://github.com/rust-lang/docs.rs/tree/master/templates/core/about\">creating</a> it?";
            let page = AxumErrorPage {
                title: "The requested page does not exist",
                message: msg.into(),
                status: StatusCode::NOT_FOUND,
                csp_nonce: String::new(),
            };
            page.into_response()
        }
    };
    Ok(response)
}

#[cfg(test)]
mod tests {
    use crate::test::{AxumResponseTestExt, AxumRouterTestExt, async_wrapper};
    use axum::http::StatusCode;

    #[test]
    fn sitemap_index() {
        async_wrapper(|env| async move {
            let app = env.web_app().await;
            app.assert_success("/sitemap.xml").await?;
            Ok(())
        })
    }

    #[test]
    fn sitemap_invalid_letters() {
        async_wrapper(|env| async move {
            let web = env.web_app().await;

            // everything not length=1 and ascii-lowercase should fail
            for invalid_letter in &["1", "aa", "A", ""] {
                println!("trying to fail letter {invalid_letter}");
                assert_eq!(
                    web.get(&format!("/-/sitemap/{invalid_letter}/sitemap.xml"))
                        .await?
                        .status(),
                    StatusCode::NOT_FOUND
                );
            }
            Ok(())
        })
    }

    #[test]
    fn sitemap_letter() {
        async_wrapper(|env| async move {
            let web = env.web_app().await;

            // letter-sitemaps always work, even without crates & releases
            for letter in 'a'..='z' {
                web.assert_success(&format!("/-/sitemap/{letter}/sitemap.xml"))
                    .await?;
            }

            env.fake_release()
                .await
                .name("some_random_crate")
                .create()
                .await?;
            env.fake_release()
                .await
                .name("some_random_crate_that_failed")
                .build_result_failed()
                .create()
                .await?;

            // these fake crates appear only in the `s` sitemap
            let response = web.get("/-/sitemap/s/sitemap.xml").await?;
            assert!(response.status().is_success());

            let content = response.text().await?;
            assert!(content.contains("some_random_crate"));
            assert!(!(content.contains("some_random_crate_that_failed")));

            // and not in the others
            for letter in ('a'..='z').filter(|&c| c != 's') {
                let response = web.get(&format!("/-/sitemap/{letter}/sitemap.xml")).await?;

                assert!(response.status().is_success());
                assert!(!(response.text().await?.contains("some_random_crate")));
            }

            Ok(())
        })
    }

    #[test]
    fn sitemap_max_age() {
        async_wrapper(|env| async move {
            let web = env.web_app().await;

            use chrono::{TimeZone, Utc};
            env.fake_release()
                .await
                .name("some_random_crate")
                .release_time(Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap())
                .create()
                .await?;

            let response = web.get("/-/sitemap/s/sitemap.xml").await?;
            assert!(response.status().is_success());

            let content = response.text().await?;
            assert!(content.contains("2022-08-28T00:00:00+00:00"));
            Ok(())
        })
    }

    #[test]
    fn about_page() {
        async_wrapper(|env| async move {
            let web = env.web_app().await;
            for file in std::fs::read_dir("templates/core/about")? {
                use std::ffi::OsStr;

                let file_path = file?.path();
                if file_path.extension() != Some(OsStr::new("html"))
                    || file_path.file_stem() == Some(OsStr::new("index"))
                {
                    continue;
                }
                let filename = file_path.file_stem().unwrap().to_str().unwrap();
                let path = format!("/about/{filename}");
                web.assert_success(&path).await?;
            }
            web.assert_success("/about").await?;
            Ok(())
        })
    }

    #[test]
    fn robots_txt() {
        async_wrapper(|env| async move {
            let web = env.web_app().await;
            web.assert_redirect("/robots.txt", "/-/static/robots.txt")
                .await?;
            Ok(())
        })
    }
}
