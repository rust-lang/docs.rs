use crate::db::Pool;
use crate::{docbuilder::Limits, impl_webpage, web::page::WebPage};
use chrono::{DateTime, NaiveDateTime, Utc};
use iron::{
    headers::ContentType,
    mime::{Mime, SubLevel, TopLevel},
    IronResult, Request, Response,
};
use serde::Serialize;
use serde_json::Value;

/// The sitemap
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct SitemapXml {
    /// The release's names and RFC 3339 timestamp to be displayed on the sitemap
    pub releases: Vec<(String, String)>,
}

impl_webpage! {
    SitemapXml   = "core/sitemap.xml",
    content_type = ContentType(Mime(TopLevel::Application, SubLevel::Xml, vec![])),
}

pub fn sitemap_handler(req: &mut Request) -> IronResult<Response> {
    let conn = extension!(req, Pool).get()?;
    let query = conn
        .query(
            "SELECT DISTINCT ON (crates.name)
                    crates.name,
                    releases.release_time
             FROM crates
             INNER JOIN releases ON releases.crate_id = crates.id
             WHERE rustdoc_status = true",
            &[],
        )
        .unwrap();

    let releases = query
        .into_iter()
        .map(|row| {
            let time = DateTime::<Utc>::from_utc(row.get::<_, NaiveDateTime>(1), Utc)
                .format("%+")
                .to_string();

            (row.get(0), time)
        })
        .collect::<Vec<(String, String)>>();

    SitemapXml { releases }.into_response(req)
}

pub fn robots_txt_handler(_: &mut Request) -> IronResult<Response> {
    let mut resp = Response::with("Sitemap: https://docs.rs/sitemap.xml");
    resp.headers.set(ContentType::plaintext());

    Ok(resp)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct About {
    /// The current version of rustc that docs.rs is using to build crates
    pub rustc_version: Option<String>,
    /// The default crate build limits
    pub limits: Limits,
}

impl_webpage!(About = "core/about.html");

pub fn about_handler(req: &mut Request) -> IronResult<Response> {
    let conn = extension!(req, Pool).get()?;
    let res = ctry!(conn.query("SELECT value FROM config WHERE name = 'rustc_version'", &[]));

    let mut rustc_version = None;

    if let Some(Ok(Value::String(version))) = res.iter().next().and_then(|row| row.get_opt(0)) {
        rustc_version = Some(version);
    }

    About {
        rustc_version,
        limits: Limits::default(),
    }
    .into_response(req)
}
