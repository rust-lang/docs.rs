use super::duration_to_str;
use super::page::Page;
use super::pool::Pool;
use super::MetaData;
use crate::docbuilder::Limits;
use iron::prelude::*;
use router::Router;
use rustc_serialize::json::{Json, ToJson};
use std::collections::BTreeMap;

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
        use iron::headers::{
            AccessControlAllowOrigin, CacheControl, CacheDirective, ContentType, Expires, HttpDate,
        };
        use iron::status;

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

#[cfg(test)]
mod tests {
    use super::*;
    use rustc_serialize::json::Json;

    #[test]
    fn serialize_build() {
        let time = time::get_time();
        let mut build = Build {
            id: 22,
            rustc_version: "rustc 1.43.0 (4fb7144ed 2020-04-20)".to_string(),
            cratesfyi_version: "docsrs 0.6.0 (3dd32ec 2020-05-01)".to_string(),
            build_status: true,
            build_time: time,
            output: None,
        };

        let correct_json = format!(
            r#"{{
                "id": 22,
                "rustc_version": "rustc 1.43.0 (4fb7144ed 2020-04-20)",
                "cratesfyi_version": "docsrs 0.6.0 (3dd32ec 2020-05-01)",
                "build_time": "{}",
                "build_time_relative": "{}",
                "output": null,
                "build_status": true
            }}"#,
            time::at(time).rfc3339().to_string(),
            duration_to_str(time),
        );

        // Have to call `.to_string()` here because for some reason rustc_serialize defaults to
        // u64s for `Json::from_str`, which makes the `id`s unequal
        assert_eq!(
            Json::from_str(&correct_json).unwrap().to_string(),
            build.to_json().to_string()
        );

        build.output = Some("some random stuff".to_string());
        let correct_json = format!(
            r#"{{
                "id": 22,
                "rustc_version": "rustc 1.43.0 (4fb7144ed 2020-04-20)",
                "cratesfyi_version": "docsrs 0.6.0 (3dd32ec 2020-05-01)",
                "build_time": "{}",
                "build_time_relative": "{}",
                "output": "some random stuff",
                "build_status": true
            }}"#,
            time::at(time).rfc3339().to_string(),
            duration_to_str(time),
        );

        // Have to call `.to_string()` here because for some reason rustc_serialize defaults to
        // u64s for `Json::from_str`, which makes the `id`s unequal
        assert_eq!(
            Json::from_str(&correct_json).unwrap().to_string(),
            build.to_json().to_string()
        );
    }

    #[test]
    fn serialize_build_page() {
        let time = time::get_time();
        let build = Build {
            id: 22,
            rustc_version: "rustc 1.43.0 (4fb7144ed 2020-04-20)".to_string(),
            cratesfyi_version: "docsrs 0.6.0 (3dd32ec 2020-05-01)".to_string(),
            build_status: true,
            build_time: time,
            output: None,
        };
        let limits = Limits::default();
        let mut builds = BuildsPage {
            metadata: Some(MetaData {
                name: "serde".to_string(),
                version: "1.0.0".to_string(),
                description: Some("serde does stuff".to_string()),
                target_name: None,
                rustdoc_status: true,
                default_target: "x86_64-unknown-linux-gnu".to_string(),
            }),
            builds: vec![build.clone()],
            build_details: Some(build.clone()),
            limits: limits.clone(),
        };

        let correct_json = format!(
            r#"{{
                "metadata": {{
                    "name": "serde",
                    "version": "1.0.0",
                    "description": "serde does stuff",
                    "target_name": null,
                    "rustdoc_status": true,
                    "default_target": "x86_64-unknown-linux-gnu"
                }},
                "builds": [{{
                    "id": 22,
                    "rustc_version": "rustc 1.43.0 (4fb7144ed 2020-04-20)",
                    "cratesfyi_version": "docsrs 0.6.0 (3dd32ec 2020-05-01)",
                    "build_time": "{time}",
                    "build_time_relative": "{time_rel}",
                    "output": null,
                    "build_status": true
                }}],
                "build_details": {{
                    "id": 22,
                    "rustc_version": "rustc 1.43.0 (4fb7144ed 2020-04-20)",
                    "cratesfyi_version": "docsrs 0.6.0 (3dd32ec 2020-05-01)",
                    "build_time": "{time}",
                    "build_time_relative": "{time_rel}",
                    "output": null,
                    "build_status": true
                }},
                "limits": {}
            }}"#,
            limits.for_website().to_json().to_string(),
            time = time::at(time).rfc3339().to_string(),
            time_rel = duration_to_str(time),
        );

        // Have to call `.to_string()` here because for some reason rustc_serialize defaults to
        // u64s for `Json::from_str`, which makes the `id`s unequal
        assert_eq!(
            Json::from_str(&correct_json).unwrap().to_string(),
            builds.to_json().to_string()
        );

        builds.metadata = None;
        let correct_json = format!(
            r#"{{
                "metadata": null,
                "builds": [{{
                    "id": 22,
                    "rustc_version": "rustc 1.43.0 (4fb7144ed 2020-04-20)",
                    "cratesfyi_version": "docsrs 0.6.0 (3dd32ec 2020-05-01)",
                    "build_time": "{time}",
                    "build_time_relative": "{time_rel}",
                    "output": null,
                    "build_status": true
                }}],
                "build_details": {{
                    "id": 22,
                    "rustc_version": "rustc 1.43.0 (4fb7144ed 2020-04-20)",
                    "cratesfyi_version": "docsrs 0.6.0 (3dd32ec 2020-05-01)",
                    "build_time": "{time}",
                    "build_time_relative": "{time_rel}",
                    "output": null,
                    "build_status": true
                }},
                "limits": {}
            }}"#,
            limits.for_website().to_json().to_string(),
            time = time::at(time).rfc3339().to_string(),
            time_rel = duration_to_str(time),
        );

        // Have to call `.to_string()` here because for some reason rustc_serialize defaults to
        // u64s for `Json::from_str`, which makes the `id`s unequal
        assert_eq!(
            Json::from_str(&correct_json).unwrap().to_string(),
            builds.to_json().to_string()
        );

        builds.builds = Vec::new();
        let correct_json = format!(
            r#"{{
                "metadata": null,
                "builds": [],
                "build_details": {{
                    "id": 22,
                    "rustc_version": "rustc 1.43.0 (4fb7144ed 2020-04-20)",
                    "cratesfyi_version": "docsrs 0.6.0 (3dd32ec 2020-05-01)",
                    "build_time": "{time}",
                    "build_time_relative": "{time_rel}",
                    "output": null,
                    "build_status": true
                }},
                "limits": {}
            }}"#,
            limits.for_website().to_json().to_string(),
            time = time::at(time).rfc3339().to_string(),
            time_rel = duration_to_str(time),
        );

        // Have to call `.to_string()` here because for some reason rustc_serialize defaults to
        // u64s for `Json::from_str`, which makes the `id`s unequal
        assert_eq!(
            Json::from_str(&correct_json).unwrap().to_string(),
            builds.to_json().to_string()
        );

        builds.build_details = None;
        let correct_json = format!(
            r#"{{
                "metadata": null,
                "builds": [],
                "build_details": null,
                "limits": {}
            }}"#,
            limits.for_website().to_json().to_string(),
        );

        // Have to call `.to_string()` here because for some reason rustc_serialize defaults to
        // u64s for `Json::from_str`, which makes the `id`s unequal
        assert_eq!(
            Json::from_str(&correct_json).unwrap().to_string(),
            builds.to_json().to_string()
        );
    }
}
