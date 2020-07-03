use crate::{
    db::Pool,
    docbuilder::Limits,
    impl_webpage,
    web::{page::WebPage, MetaData},
};
use chrono::{DateTime, NaiveDateTime, Utc};
use iron::{
    headers::{
        AccessControlAllowOrigin, CacheControl, CacheDirective, ContentType, Expires, HttpDate,
    },
    status, IronResult, Request, Response,
};
use router::Router;
use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct Build {
    id: i32,
    rustc_version: String,
    docsrs_version: String,
    build_status: bool,
    build_time: DateTime<Utc>,
    output: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct BuildsPage {
    metadata: Option<MetaData>,
    builds: Vec<Build>,
    build_log: Option<Build>,
    limits: Limits,
}

impl_webpage! {
    BuildsPage = "crate/builds.html",
}

pub fn build_list_handler(req: &mut Request) -> IronResult<Response> {
    let router = extension!(req, Router);
    let name = cexpect!(req, router.find("name"));
    let version = cexpect!(req, router.find("version"));
    let req_build_id: i32 = router.find("id").unwrap_or("0").parse().unwrap_or(0);

    let conn = extension!(req, Pool).get()?;
    let limits = ctry!(req, Limits::for_crate(&conn, name));

    let query = ctry!(
        req,
        conn.query(
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
        )
    );

    let mut build_log = None;
    // FIXME: getting builds.output may cause performance issues when release have tons of builds
    let mut builds = query
        .into_iter()
        .map(|row| {
            let id: i32 = row.get("id");

            let build = Build {
                id,
                rustc_version: row.get("rustc_version"),
                docsrs_version: row.get("cratesfyi_version"),
                build_status: row.get("build_status"),
                build_time: DateTime::from_utc(row.get::<_, NaiveDateTime>("build_time"), Utc),
                output: row.get("output"),
            };

            if id == req_build_id {
                build_log = Some(build.clone());
            }

            build
        })
        .collect::<Vec<Build>>();

    if req.url.path().join("/").ends_with(".json") {
        // Remove build output from build list for json output
        for build in builds.iter_mut() {
            build.output = None;
        }

        let mut resp = Response::with((status::Ok, serde_json::to_string(&builds).unwrap()));
        resp.headers.set(ContentType::json());
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
            builds,
            build_log,
            limits,
        }
        .into_response(req)
    }
}
