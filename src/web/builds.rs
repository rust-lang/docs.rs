use crate::docbuilder::Limits;
use std::collections::BTreeMap;
use super::MetaData;
use super::pool::Pool;
use super::duration_to_str;
use super::page::Page;
use iron::prelude::*;
use time;
use router::Router;
use rustc_serialize::json::{Json, ToJson};

#[derive(Clone)]
struct Build {
    id: i32,
    rustc_version: String,
    cratesfyi_version: String,
    build_status: bool,
    build_time: time::Timespec,
    output: Option<String>,
}

struct BuildsPage {
    metadata: Option<MetaData>,
    builds: Vec<Build>,
    build_details: Option<Build>,
    limits: Limits,
}

impl ToJson for Build {
    fn to_json(&self) -> Json {
        let mut m: BTreeMap<String, Json> = BTreeMap::new();
        m.insert("id".to_owned(), self.id.to_json());
        m.insert("rustc_version".to_owned(), self.rustc_version.to_json());
        m.insert(
            "cratesfyi_version".to_owned(),
            self.cratesfyi_version.to_json(),
        );
        m.insert("build_status".to_owned(), self.build_status.to_json());
        m.insert(
            "build_time".to_owned(),
            format!("{}", time::at(self.build_time).rfc3339()).to_json(),
        );
        m.insert(
            "build_time_relative".to_owned(),
            duration_to_str(self.build_time).to_json(),
        );
        m.insert("output".to_owned(), self.output.to_json());
        m.to_json()
    }
}

impl ToJson for BuildsPage {
    fn to_json(&self) -> Json {
        let mut m: BTreeMap<String, Json> = BTreeMap::new();
        m.insert("metadata".to_owned(), self.metadata.to_json());
        m.insert("builds".to_owned(), self.builds.to_json());
        m.insert("build_details".to_owned(), self.build_details.to_json());
        m.insert("limits".into(), self.limits.for_website().to_json());
        m.to_json()
    }
}

pub fn build_list_handler(req: &mut Request) -> IronResult<Response> {
    let router = extension!(req, Router);
    let name = cexpect!(router.find("name"));
    let version = cexpect!(router.find("version"));
    let req_build_id: i32 = router.find("id").unwrap_or("0").parse().unwrap_or(0);

    let conn = extension!(req, Pool).get();
    let limits = ctry!(Limits::for_crate(&conn, name));

    let mut build_list: Vec<Build> = Vec::new();
    let mut build_details = None;

    // FIXME: getting builds.output may cause performance issues when release have tons of builds
    for row in &ctry!(conn.query(
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
    )) {
        let id: i32 = row.get(5);

        let build = Build {
            id,
            rustc_version: row.get(6),
            cratesfyi_version: row.get(7),
            build_status: row.get(8),
            build_time: row.get(9),
            output: row.get(10),
        };

        if id == req_build_id {
            build_details = Some(build.clone());
        }

        build_list.push(build);
    }

    if req.url.path().join("/").ends_with(".json") {
        use iron::status;
        use iron::headers::{
            Expires, HttpDate, CacheControl, CacheDirective, ContentType, AccessControlAllowOrigin,
        };

        // Remove build output from build list for json output
        for build in build_list.as_mut_slice() {
            build.output = None;
        }

        let mut resp = Response::with((status::Ok, build_list.to_json().to_string()));
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
        let builds_page = BuildsPage {
            metadata: MetaData::from_crate(&conn, &name, &version),
            builds: build_list,
            build_details,
            limits,
        };
        Page::new(builds_page)
            .set_true("show_package_navigation")
            .set_true("package_navigation_builds_tab")
            .to_resp("builds")
    }
}
