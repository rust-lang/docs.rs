use crate::{
    db::Pool,
    docbuilder::Limits,
    impl_axum_webpage,
    utils::{get_config, spawn_blocking, ConfigName},
    web::{
        error::{AxumNope, AxumResult},
        extractors::{DbConnection, Path},
        page::templates::filters,
        AxumErrorPage, MetaData,
    },
    Config,
};
use axum::{extract::Extension, http::StatusCode, response::IntoResponse};
use chrono::{TimeZone, Utc};
use futures_util::stream::TryStreamExt;
use rinja::Template;
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

impl AboutBuilds {
    pub(crate) fn get_metadata(&self) -> Option<&MetaData> {
        None
    }
}

impl_axum_webpage!(AboutBuilds);

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

        impl $ty {
            pub(crate) fn get_metadata(&self) -> Option<&MetaData> {
                None
            }
        }
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
