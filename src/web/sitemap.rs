use crate::{
    db::Pool,
    docbuilder::Limits,
    impl_axum_webpage,
    utils::{get_config, spawn_blocking, ConfigName},
    web::{
        error::{AxumNope, AxumResult},
        AxumErrorPage,
    },
    Config,
};
use axum::{
    extract::{Extension, Path},
    http::StatusCode,
    response::IntoResponse,
};
use chrono::{DateTime, TimeZone, Utc};
use serde::Serialize;
use std::sync::Arc;

/// sitemap index
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct SitemapIndexXml {
    sitemaps: Vec<char>,
}

impl_axum_webpage! {
    SitemapIndexXml   = "core/sitemapindex.xml",
    content_type = "application/xml",
}

pub(crate) async fn sitemapindex_handler() -> impl IntoResponse {
    let sitemaps: Vec<char> = ('a'..='z').collect();

    SitemapIndexXml { sitemaps }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct SitemapRow {
    crate_name: String,
    last_modified: String,
    target_name: String,
}

/// The sitemap
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct SitemapXml {
    releases: Vec<SitemapRow>,
}

impl_axum_webpage! {
    SitemapXml   = "core/sitemap.xml",
    content_type = "application/xml",
}

pub(crate) async fn sitemap_handler(
    Path(letter): Path<String>,
    Extension(pool): Extension<Pool>,
) -> AxumResult<impl IntoResponse> {
    if letter.len() != 1 {
        return Err(AxumNope::ResourceNotFound);
    } else if let Some(ch) = letter.chars().next() {
        if !(ch.is_ascii_lowercase()) {
            return Err(AxumNope::ResourceNotFound);
        }
    }
    let releases = spawn_blocking(move || {
        let mut conn = pool.get()?;
        let query = conn.query(
            "SELECT crates.name,
                    releases.target_name,
                    MAX(releases.release_time) as release_time
             FROM crates
             INNER JOIN releases ON releases.crate_id = crates.id
             WHERE
                rustdoc_status = true AND
                crates.name ILIKE $1
             GROUP BY crates.name, releases.target_name
             ",
            &[&format!("{letter}%")],
        )?;

        Ok(query
            .into_iter()
            .map(|row| SitemapRow {
                crate_name: row.get("name"),
                target_name: row.get("target_name"),
                last_modified: row
                    .get::<_, DateTime<Utc>>("release_time")
                    // On Aug 27 2022 we added `<link rel="canonical">` to all pages,
                    // so they should all get recrawled if they haven't been since then.
                    .max(Utc.with_ymd_and_hms(2022, 8, 28, 0, 0, 0).unwrap())
                    .format("%+")
                    .to_string(),
            })
            .collect())
    })
    .await?;

    Ok(SitemapXml { releases })
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct AboutBuilds {
    /// The current version of rustc that docs.rs is using to build crates
    rustc_version: Option<String>,
    /// The default crate build limits
    limits: Limits,
    /// Just for the template, since this isn't shared with AboutPage
    active_tab: &'static str,
}

impl_axum_webpage!(AboutBuilds = "core/about/builds.html");

pub(crate) async fn about_builds_handler(
    Extension(pool): Extension<Pool>,
    Extension(config): Extension<Arc<Config>>,
) -> AxumResult<impl IntoResponse> {
    let rustc_version = spawn_blocking(move || {
        let mut conn = pool.get()?;
        get_config::<String>(&mut conn, ConfigName::RustcVersion)
    })
    .await?;

    Ok(AboutBuilds {
        rustc_version,
        limits: Limits::new(&config),
        active_tab: "builds",
    })
}

#[derive(Serialize)]
struct AboutPage<'a> {
    #[serde(skip)]
    template: String,
    active_tab: &'a str,
}

impl_axum_webpage!(AboutPage<'_> = |this: &AboutPage| this.template.clone().into());

pub(crate) async fn about_handler(subpage: Option<Path<String>>) -> AxumResult<impl IntoResponse> {
    let subpage = match subpage {
        Some(subpage) => subpage.0,
        None => "index".to_string(),
    };

    let name = match &subpage[..] {
        "about" | "index" => "index",
        x @ "badges" | x @ "metadata" | x @ "redirections" | x @ "download" => x,
        _ => {
            let msg = "This /about page does not exist. \
                Perhaps you are interested in <a href=\"https://github.com/rust-lang/docs.rs/tree/master/templates/core/about\">creating</a> it?";
            let page = AxumErrorPage {
                title: "The requested page does not exist",
                message: msg.into(),
                status: StatusCode::NOT_FOUND,
            };
            return Ok(page.into_response());
        }
    };
    let template = format!("core/about/{name}.html");
    Ok(AboutPage {
        template,
        active_tab: name,
    }
    .into_response())
}

#[cfg(test)]
mod tests {
    use crate::test::{assert_success, wrapper};
    use reqwest::StatusCode;

    #[test]
    fn sitemap_index() {
        wrapper(|env| {
            let web = env.frontend();
            assert_success("/sitemap.xml", web)
        })
    }

    #[test]
    fn sitemap_invalid_letters() {
        wrapper(|env| {
            let web = env.frontend();

            // everything not length=1 and ascii-lowercase should fail
            for invalid_letter in &["1", "aa", "A", ""] {
                println!("trying to fail letter {invalid_letter}");
                assert_eq!(
                    web.get(&format!("/-/sitemap/{invalid_letter}/sitemap.xml"))
                        .send()?
                        .status(),
                    StatusCode::NOT_FOUND
                );
            }
            Ok(())
        })
    }

    #[test]
    fn sitemap_letter() {
        wrapper(|env| {
            let web = env.frontend();

            // letter-sitemaps always work, even without crates & releases
            for letter in 'a'..='z' {
                assert_success(&format!("/-/sitemap/{letter}/sitemap.xml"), web)?;
            }

            env.fake_release().name("some_random_crate").create()?;
            env.fake_release()
                .name("some_random_crate_that_failed")
                .build_result_failed()
                .create()?;

            // these fake crates appear only in the `s` sitemap
            let response = web.get("/-/sitemap/s/sitemap.xml").send()?;
            assert!(response.status().is_success());

            let content = response.text()?;
            assert!(content.contains("some_random_crate"));
            assert!(!(content.contains("some_random_crate_that_failed")));

            // and not in the others
            for letter in ('a'..='z').filter(|&c| c != 's') {
                let response = web
                    .get(&format!("/-/sitemap/{letter}/sitemap.xml"))
                    .send()?;

                assert!(response.status().is_success());
                assert!(!(response.text()?.contains("some_random_crate")));
            }

            Ok(())
        })
    }

    #[test]
    fn sitemap_max_age() {
        wrapper(|env| {
            let web = env.frontend();

            use chrono::{TimeZone, Utc};
            env.fake_release()
                .name("some_random_crate")
                .release_time(Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap())
                .create()?;

            let response = web.get("/-/sitemap/s/sitemap.xml").send()?;
            assert!(response.status().is_success());

            let content = response.text()?;
            assert!(content.contains("2022-08-28T00:00:00+00:00"));
            Ok(())
        })
    }

    #[test]
    fn about_page() {
        wrapper(|env| {
            let web = env.frontend();
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
                assert_success(&path, web)?;
            }
            assert_success("/about", web)
        })
    }

    #[test]
    fn robots_txt() {
        wrapper(|env| {
            let web = env.frontend();
            assert_success("/robots.txt", web)
        })
    }
}
