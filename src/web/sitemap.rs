use crate::{db::Pool, docbuilder::Limits, impl_webpage, web::error::Nope, web::page::WebPage};
use chrono::{DateTime, Utc};
use iron::{
    headers::ContentType,
    mime::{Mime, SubLevel, TopLevel},
    IronResult, Request, Response,
};
use router::Router;
use serde::Serialize;
use serde_json::Value;

/// sitemap index
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct SitemapIndexXml {
    sitemaps: Vec<char>,
}

impl_webpage! {
    SitemapIndexXml   = "core/sitemapindex.xml",
    content_type = ContentType(Mime(TopLevel::Application, SubLevel::Xml, vec![])),
}

pub fn sitemapindex_handler(req: &mut Request) -> IronResult<Response> {
    let sitemaps: Vec<char> = ('a'..='z').collect();

    SitemapIndexXml { sitemaps }.into_response(req)
}

/// The sitemap
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct SitemapXml {
    /// The release's names and RFC 3339 timestamp to be displayed on the sitemap
    releases: Vec<(String, String)>,
}

impl_webpage! {
    SitemapXml   = "core/sitemap.xml",
    content_type = ContentType(Mime(TopLevel::Application, SubLevel::Xml, vec![])),
}

pub fn sitemap_handler(req: &mut Request) -> IronResult<Response> {
    let router = extension!(req, Router);
    let letter = cexpect!(req, router.find("letter"));

    if letter.len() != 1 {
        return Err(Nope::ResourceNotFound.into());
    } else if let Some(ch) = letter.chars().next() {
        if !(ch.is_ascii_lowercase()) {
            return Err(Nope::ResourceNotFound.into());
        }
    }

    let mut conn = extension!(req, Pool).get()?;
    let query = conn
        .query(
            "SELECT crates.name,
                    MAX(releases.release_time) as release_time
             FROM crates
             INNER JOIN releases ON releases.crate_id = crates.id
             WHERE 
                rustdoc_status = true AND 
                crates.name ILIKE $1 
             GROUP BY crates.name
             ",
            &[&format!("{}%", letter)],
        )
        .unwrap();

    let releases = query
        .into_iter()
        .map(|row| {
            let time = row.get::<_, DateTime<Utc>>(1).format("%+").to_string();

            (row.get(0), time)
        })
        .collect::<Vec<(String, String)>>();

    SitemapXml { releases }.into_response(req)
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

impl_webpage!(AboutBuilds = "core/about/builds.html");

pub fn about_builds_handler(req: &mut Request) -> IronResult<Response> {
    let mut conn = extension!(req, Pool).get()?;
    let res = ctry!(
        req,
        conn.query("SELECT value FROM config WHERE name = 'rustc_version'", &[]),
    );

    let rustc_version = res.get(0).and_then(|row| {
        if let Ok(Some(Value::String(version))) = row.try_get(0) {
            Some(version)
        } else {
            None
        }
    });

    AboutBuilds {
        rustc_version,
        limits: Limits::default(),
        active_tab: "builds",
    }
    .into_response(req)
}

#[derive(Serialize)]
struct AboutPage<'a> {
    #[serde(skip)]
    template: String,
    active_tab: &'a str,
}

impl_webpage!(AboutPage<'_> = |this: &AboutPage| this.template.clone().into());

pub fn about_handler(req: &mut Request) -> IronResult<Response> {
    use super::ErrorPage;
    use iron::status::Status;

    let name = match *req.url.path().last().expect("iron is broken") {
        "about" | "index" => "index",
        x @ "badges" | x @ "metadata" | x @ "redirections" => x,
        _ => {
            let msg = "This /about page does not exist. \
                Perhaps you are interested in <a href=\"https://github.com/rust-lang/docs.rs/tree/master/templates/core/about\">creating</a> it?";
            let page = ErrorPage {
                title: "The requested page does not exist",
                message: Some(msg.into()),
                status: Status::NotFound,
            };
            return page.into_response(req);
        }
    };
    let template = format!("core/about/{}.html", name);
    AboutPage {
        template,
        active_tab: name,
    }
    .into_response(req)
}

// This authenticates @jsha (https://github.com/jsha) to use the Google Search Console
// https://support.google.com/webmasters/answer/9128668?visit_id=637926083810327397-791277710&rd=2
// for docs.rs. Removing this handler will de-authenticate. Additional handlers can be
// added for additional users.
pub fn google_search_console_handler(_req: &mut Request) -> IronResult<Response> {
    Ok(Response::with(
        "google-site-verification: googleb54494ba4d52c200.html".to_string(),
    ))
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
                println!("trying to fail letter {}", invalid_letter);
                assert_eq!(
                    web.get(&format!("/-/sitemap/{}/sitemap.xml", invalid_letter))
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
                assert_success(&format!("/-/sitemap/{}/sitemap.xml", letter), web)?;
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
            assert!(content.contains(&"some_random_crate"));
            assert!(!(content.contains(&"some_random_crate_that_failed")));

            // and not in the others
            for letter in ('a'..='z').filter(|&c| c != 's') {
                let response = web
                    .get(&format!("/-/sitemap/{}/sitemap.xml", letter))
                    .send()?;

                assert!(response.status().is_success());
                assert!(!(response.text()?.contains("some_random_crate")));
            }

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
                let path = format!("/about/{}", filename);
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
