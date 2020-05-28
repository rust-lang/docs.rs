use super::{
    page::{About, SitemapXml, WebPage},
    pool::Pool,
};
use crate::docbuilder::Limits;
use iron::{headers::ContentType, prelude::*};
use serde_json::Value;

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
            let time = format!("{}", time::at(row.get(1)).rfc3339());

            (row.get(0), time)
        })
        .collect::<Vec<(String, String)>>();

    // TODO: Only the second element of each tuple (the release time) is actually used
    //       in the template
    SitemapXml { releases }.into_response()
}

pub fn robots_txt_handler(_: &mut Request) -> IronResult<Response> {
    let mut resp = Response::with("Sitemap: https://docs.rs/sitemap.xml");
    resp.headers.set(ContentType("text/plain".parse().unwrap()));
    Ok(resp)
}

pub fn about_handler(req: &mut Request) -> IronResult<Response> {
    let conn = extension!(req, Pool).get()?;
    let res = ctry!(conn.query("SELECT value FROM config WHERE name = 'rustc_version'", &[]));

    let mut rustc_version = None;
    if let Some(row) = res.iter().next() {
        if let Some(Ok::<Value, _>(res)) = row.get_opt(0) {
            if let Some(vers) = res.as_str() {
                rustc_version = Some(vers.to_owned());
            }
        }
    }

    About {
        rustc_version,
        limits: Limits::default(),
    }
    .into_response()
}
