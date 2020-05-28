use super::page::{BuildsPage, WebPage};
use super::pool::Pool;
use super::MetaData;
use crate::docbuilder::Limits;
use iron::prelude::*;
use router::Router;
use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Build {
    id: i32,
    rustc_version: String,
    docsrs_version: String,
    build_status: bool,
    #[serde(serialize_with = "super::rfc3339")]
    build_time: time::Timespec,
    output: Option<String>,
}

pub fn build_list_handler(req: &mut Request) -> IronResult<Response> {
    let router = extension!(req, Router);
    let name = cexpect!(router.find("name"));
    let version = cexpect!(router.find("version"));
    let req_build_id: i32 = router.find("id").unwrap_or("0").parse().unwrap_or(0);

    let conn = extension!(req, Pool).get()?;
    let limits = ctry!(Limits::for_crate(&conn, name));

    let query = ctry!(conn.query(
        "SELECT crates.name,
                releases.version,
                releases.description,
                releases.rustdoc_status,
                releases.target_name,
                builds.id,
                builds.rustc_version,
                builds.cratesfyi_version,
                builds.build_status,
                builds.build_time,
                builds.output
         FROM builds
         INNER JOIN releases ON releases.id = builds.rid
         INNER JOIN crates ON releases.crate_id = crates.id
         WHERE crates.name = $1 AND releases.version = $2
         ORDER BY id DESC",
        &[&name, &version]
    ));

    let mut build_log = None;
    // FIXME: getting builds.output may cause performance issues when release have tons of builds
    let mut build_list = query
        .into_iter()
        .map(|row| {
            let id: i32 = row.get(5);

            let build = Build {
                id,
                rustc_version: row.get(6),
                docsrs_version: row.get(7),
                build_status: row.get(8),
                build_time: row.get(9),
                output: row.get(10),
            };

            if id == req_build_id {
                build_log = Some(build.clone());
            }

            build
        })
        .collect::<Vec<Build>>();

    if req.url.path().join("/").ends_with(".json") {
        use iron::headers::{
            AccessControlAllowOrigin, CacheControl, CacheDirective, ContentType, Expires, HttpDate,
        };
        use iron::status;

        // Remove build output from build list for json output
        for build in build_list.as_mut_slice() {
            build.output = None;
        }

        let mut resp = Response::with((status::Ok, serde_json::to_string(&build_list).unwrap()));
        resp.headers
            .set(ContentType("application/json".parse().unwrap()));
        resp.headers.set(Expires(HttpDate(time::now())));
        resp.headers.set(CacheControl(vec![
            CacheDirective::NoCache,
            CacheDirective::NoStore,
            CacheDirective::MustRevalidate,
        ]));
        resp.headers.set(AccessControlAllowOrigin::Any);
        Ok(resp)
    } else {
        BuildsPage {
            metadata: MetaData::from_crate(&conn, &name, &version),
            builds: build_list,
            build_log,
            limits,
        }
        .into_response()
    }
}
